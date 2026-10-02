//! One actor owns SQLite and all pending changes; no UI work runs on this thread.
use crate::{
    model::*,
    repository::{Mutation, PendingItem, Repository},
    search,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct QueuedCommand {
    command: Result<Command, String>,
    bytes: usize,
}
struct Commands {
    sender: Option<Sender<QueuedCommand>>,
    exiting: bool,
    queued_bytes: Arc<AtomicUsize>,
    queue_limit: usize,
    max_item_bytes: usize,
}
pub struct Engine {
    commands: Mutex<Commands>,
    events: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
}
impl Engine {
    pub fn start(
        options: EngineOptions,
        notifier: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let repo = Repository::open(&options.database_path)?;
        let settings = repo.settings()?;
        let (sender, commands) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let worker_queue_bytes = queued_bytes.clone();
        let queue_limit = options.max_pending_bytes;
        let max_item_bytes = options.max_item_bytes;
        let worker = thread::Builder::new()
            .name("quick-panel-storage".into())
            .spawn(move || {
                let mut actor = Actor {
                    repo,
                    options,
                    settings,
                    pending: HashMap::new(),
                    deleted: HashSet::new(),
                    settings_dirty: false,
                    deadline: None,
                    checkpoint_at: None,
                    commits: 0,
                    retry_delay_ms: 1_000,
                    retry_not_before: None,
                    queued_bytes: worker_queue_bytes,
                    exiting: false,
                    events: events_tx,
                    notifier,
                };
                actor.emit(Event::Ready(actor.settings.clone()));
                let exit_ready = actor.run(commands);
                let events = actor.events.clone();
                let notifier = actor.notifier.clone();
                drop(actor);
                if exit_ready && events.send(Event::ExitReady).is_ok() {
                    notifier();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands: Mutex::new(Commands {
                sender: Some(sender),
                exiting: false,
                queued_bytes,
                queue_limit,
                max_item_bytes,
            }),
            events,
            worker: Some(worker),
        })
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        let mut state = self
            .commands
            .lock()
            .map_err(|_| "command queue poisoned".to_string())?;
        if state.exiting
            && !matches!(
                command,
                Command::Shutdown
                    | Command::CancelShutdown
                    | Command::Search(_)
                    | Command::Load { .. }
                    | Command::Flush
            )
        {
            return Err("正在保存退出；取消退出后可继续修改".into());
        }
        let exiting = match &command {
            Command::Shutdown => Some(true),
            Command::CancelShutdown => Some(false),
            _ => None,
        };
        let bytes = match &command {
            Command::Capture(snapshot) => snapshot.text.len().saturating_add(
                snapshot
                    .representations
                    .iter()
                    .map(|r| r.data.len())
                    .sum::<usize>(),
            ),
            _ => 0,
        };
        let (command, bytes) = if bytes > state.max_item_bytes {
            (
                Err(format!(
                    "剪贴板内容超过单条 {} 字节上限，未记录",
                    state.max_item_bytes
                )),
                0,
            )
        } else {
            (Ok(command), bytes)
        };
        if bytes > 0 {
            state
                .queued_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    let total = used.checked_add(bytes)?;
                    (used == 0 || total <= state.queue_limit).then_some(total)
                })
                .map_err(|_| "采集任务队列已满，本条未记录".to_string())?;
        }
        let result = state
            .sender
            .as_ref()
            .ok_or("engine stopped")?
            .send(QueuedCommand { command, bytes });
        if result.is_err() {
            state.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err("engine stopped".into());
        }
        if let Some(exiting) = exiting {
            state.exiting = exiting;
        }
        Ok(())
    }
    pub fn try_recv(&self) -> Result<Event, TryRecvError> {
        self.events.try_recv()
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        if let Ok(state) = self.commands.get_mut()
            && let Some(sender) = state.sender.take()
        {
            let _ = sender.send(QueuedCommand {
                command: Ok(Command::Shutdown),
                bytes: 0,
            });
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct Actor {
    repo: Repository,
    options: EngineOptions,
    settings: Settings,
    pending: HashMap<String, PendingItem>,
    deleted: HashSet<String>,
    settings_dirty: bool,
    deadline: Option<Instant>,
    checkpoint_at: Option<Instant>,
    commits: u64,
    retry_delay_ms: u64,
    retry_not_before: Option<Instant>,
    queued_bytes: Arc<AtomicUsize>,
    exiting: bool,
    events: Sender<Event>,
    notifier: Arc<dyn Fn() + Send + Sync>,
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn item_bytes(item: &StoredItem) -> usize {
    item.text.len().saturating_add(
        item.representations
            .iter()
            .map(|r| r.data.len())
            .sum::<usize>(),
    )
}
fn content_id(snapshot: &ClipboardSnapshot) -> String {
    let mut hash = Sha256::new();
    hash.update((snapshot.text.len() as u64).to_le_bytes());
    hash.update(snapshot.text.as_bytes());
    let mut reprs: Vec<_> = snapshot.representations.iter().collect();
    reprs.sort_by(|a, b| a.format.cmp(&b.format).then(a.data.cmp(&b.data)));
    for r in reprs {
        hash.update((r.format.len() as u64).to_le_bytes());
        hash.update(r.format.as_bytes());
        hash.update((r.data.len() as u64).to_le_bytes());
        hash.update(&r.data);
    }
    format!("clip:{:x}", hash.finalize())
}
fn result_rank(summary: &ItemSummary, query: &str) -> i64 {
    search::rank(summary, query)
        + if query.trim().is_empty() && summary.kind != ItemKind::Clipboard {
            5_000
        } else {
            0
        }
}
fn scope_matches(kind: ItemKind, scope: Scope) -> bool {
    matches!(
        (kind, scope),
        (_, Scope::All)
            | (ItemKind::Clipboard, Scope::Clipboard)
            | (ItemKind::Folder, Scope::Folder)
            | (ItemKind::Document, Scope::Document)
    )
}
impl Actor {
    fn emit(&self, event: Event) {
        if self.events.send(event).is_ok() {
            (self.notifier)();
        }
    }
    fn error(&self, message: String) {
        self.emit(Event::Error {
            message,
            exiting: self.exiting,
        });
    }
    fn dirty(&self) -> bool {
        !self.pending.is_empty() || !self.deleted.is_empty() || self.settings_dirty
    }
    fn mark_dirty(&mut self) {
        if self.deadline.is_none() {
            self.deadline =
                Some(Instant::now() + Duration::from_millis(self.options.save_interval_ms));
        }
    }
    fn pending_bytes(&self) -> usize {
        self.pending.values().map(|p| item_bytes(&p.item)).sum()
    }
    fn lookup(&self, id: &str, blobs: bool) -> Result<Option<StoredItem>, String> {
        if self.deleted.contains(id) {
            return Ok(None);
        }
        if let Some(pending) = self.pending.get(id) {
            let mut item = pending.item.clone();
            if blobs
                && pending.mutation == Mutation::Metadata
                && let Some(saved) = self.repo.load(id, true)?
            {
                item.text = saved.text;
                item.representations = saved.representations;
            }
            return Ok(Some(item));
        }
        if blobs {
            self.repo.load(id, true)
        } else {
            self.repo.metadata(id)
        }
    }
    fn pressure_flush(&mut self) -> bool {
        if self
            .retry_not_before
            .is_some_and(|when| Instant::now() < when)
        {
            return false;
        }
        self.flush()
    }
    fn flush(&mut self) -> bool {
        if !self.dirty() {
            return true;
        }
        let now = now_ms();
        match self.repo.commit(
            &self.pending,
            &self.deleted,
            self.settings_dirty.then_some(&self.settings),
            &self.settings,
            now,
        ) {
            Ok(()) => {
                self.pending.clear();
                self.deleted.clear();
                self.settings_dirty = false;
                self.deadline = None;
                self.commits += 1;
                self.retry_delay_ms = 1_000;
                self.retry_not_before = None;
                self.checkpoint_at
                    .get_or_insert(Instant::now() + Duration::from_secs(300));
                if self.repo.wal_bytes() >= 16 * 1024 * 1024 {
                    let pressure = Instant::now() + Duration::from_secs(30);
                    self.checkpoint_at = Some(self.checkpoint_at.unwrap().min(pressure));
                }
                self.emit(Event::Saved(SaveStats {
                    pending_items: 0,
                    pending_bytes: 0,
                    commits: self.commits,
                    last_saved_at_ms: now,
                }));
                true
            }
            Err(message) => {
                self.deadline = Some(Instant::now() + Duration::from_millis(self.retry_delay_ms));
                self.retry_not_before = self.deadline;
                self.retry_delay_ms = self.retry_delay_ms.saturating_mul(2).min(60_000);
                self.error(message);
                false
            }
        }
    }
    fn capture(&mut self, snapshot: ClipboardSnapshot) -> Result<(), String> {
        if self.settings.capture_paused
            || self
                .settings
                .excluded_apps
                .iter()
                .any(|app| app.eq_ignore_ascii_case(&snapshot.source_app))
        {
            return Ok(());
        }
        let bytes = snapshot.text.len().saturating_add(
            snapshot
                .representations
                .iter()
                .map(|r| r.data.len())
                .sum::<usize>(),
        );
        if bytes > self.options.max_item_bytes {
            return Err(format!(
                "剪贴板内容超过单条 {} 字节上限，未记录",
                self.options.max_item_bytes
            ));
        }
        let id = content_id(&snapshot);
        if let Some(mut old) = self.lookup(&id, false)? {
            old.summary.last_used_ms = old.summary.last_used_ms.max(snapshot.captured_at_ms);
            let mutation = self
                .pending
                .get(&id)
                .map_or(Mutation::Metadata, |p| p.mutation);
            self.pending.insert(
                id,
                PendingItem {
                    item: old,
                    mutation,
                },
            );
            self.mark_dirty();
            self.emit(Event::Changed);
            return Ok(());
        }
        if self.pending_bytes().saturating_add(bytes) > self.options.max_pending_bytes
            && self.dirty()
            && !self.pressure_flush()
        {
            return Err("待保存缓存已满，保存失败，本条未记录".into());
        }
        let old = self.lookup(&id, false)?;
        let mut summary = old.map(|i| i.summary).unwrap_or_else(|| ItemSummary {
            id: id.clone(),
            kind: ItemKind::Clipboard,
            title: if snapshot.text.is_empty() {
                snapshot
                    .representations
                    .first()
                    .map(|r| format!("{} 剪贴板内容", r.format))
                    .unwrap_or_else(|| "剪贴板内容".into())
            } else {
                snapshot
                    .text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(160)
                    .collect()
            },
            subtitle: snapshot.source_app.clone(),
            alias: String::new(),
            tags: vec![],
            pinned: false,
            created_at_ms: snapshot.captured_at_ms,
            last_used_ms: snapshot.captured_at_ms,
            use_count: 0,
        });
        summary.last_used_ms = summary.last_used_ms.max(snapshot.captured_at_ms);
        self.deleted.remove(&id);
        self.pending.insert(
            id,
            PendingItem {
                item: StoredItem {
                    summary,
                    text: snapshot.text,
                    representations: snapshot.representations,
                    target: None,
                    open_with: None,
                },
                mutation: Mutation::Content,
            },
        );
        self.mark_dirty();
        self.emit(Event::Changed);
        if self.pending_bytes() >= self.options.max_pending_bytes {
            self.pressure_flush();
        }
        Ok(())
    }
    fn search(&self, query: SearchQuery) -> Result<(), String> {
        let mut items: HashMap<String, ItemSummary> = self
            .repo
            .summaries(&query)?
            .into_iter()
            .filter(|s| {
                !self.deleted.contains(&s.id)
                    && !self
                        .pending
                        .get(&s.id)
                        .is_some_and(|p| p.mutation == Mutation::Content)
            })
            .map(|s| {
                let s = self
                    .pending
                    .get(&s.id)
                    .map_or(s.clone(), |p| p.item.summary.clone());
                (s.id.clone(), s)
            })
            .collect();
        for p in self.pending.values() {
            if p.mutation == Mutation::Metadata && items.contains_key(&p.item.summary.id) {
                continue;
            }
            let body;
            let item = if p.mutation == Mutation::Metadata && !search::matches(&p.item, &query.text)
            {
                body = self.repo.load(&p.item.summary.id, false)?;
                let Some(body) = body.as_ref() else {
                    continue;
                };
                body
            } else {
                &p.item
            };
            if scope_matches(p.item.summary.kind, query.scope) && search::matches(item, &query.text)
            {
                items.insert(p.item.summary.id.clone(), p.item.summary.clone());
            }
        }
        let mut items: Vec<_> = items.into_values().collect();
        items.sort_by(|a, b| {
            result_rank(b, &query.text)
                .cmp(&result_rank(a, &query.text))
                .then(b.pinned.cmp(&a.pinned))
                .then(b.last_used_ms.cmp(&a.last_used_ms))
                .then(a.id.cmp(&b.id))
        });
        items.truncate(50);
        self.emit(Event::Results {
            request_id: query.request_id,
            items,
        });
        Ok(())
    }
    fn favorite(&mut self, favorite: FavoriteInput) -> Result<(), String> {
        if favorite.kind == ItemKind::Clipboard {
            return Err("收藏需要文件夹或文档类型".into());
        }
        let id = favorite.id.unwrap_or_else(|| {
            let mut hash = Sha256::new();
            hash.update(format!("{:?}:{}", favorite.kind, favorite.target));
            format!("favorite:{:x}", hash.finalize())
        });
        let old = self.lookup(&id, false)?;
        let now = now_ms();
        let summary = ItemSummary {
            id: id.clone(),
            kind: favorite.kind,
            title: favorite.title,
            subtitle: favorite.target.clone(),
            alias: favorite.alias,
            tags: favorite.tags,
            pinned: old.as_ref().is_some_and(|i| i.summary.pinned),
            created_at_ms: old.as_ref().map_or(now, |i| i.summary.created_at_ms),
            last_used_ms: old.as_ref().map_or(now, |i| i.summary.last_used_ms),
            use_count: old.as_ref().map_or(0, |i| i.summary.use_count),
        };
        self.deleted.remove(&id);
        self.pending.insert(
            id,
            PendingItem {
                item: StoredItem {
                    summary,
                    text: String::new(),
                    representations: vec![],
                    target: Some(favorite.target),
                    open_with: favorite.open_with,
                },
                mutation: Mutation::Content,
            },
        );
        self.mark_dirty();
        self.emit(Event::Changed);
        self.flush();
        Ok(())
    }
    fn modify(&mut self, id: String, pinned: Option<bool>) -> Result<(), String> {
        let Some(mut item) = self.lookup(&id, false)? else {
            return Ok(());
        };
        if let Some(value) = pinned {
            item.summary.pinned = value;
        } else {
            item.summary.use_count = item.summary.use_count.saturating_add(1);
            item.summary.last_used_ms = now_ms();
        }
        let mutation = self
            .pending
            .get(&id)
            .map_or(Mutation::Metadata, |p| p.mutation);
        self.pending.insert(id, PendingItem { item, mutation });
        self.mark_dirty();
        self.emit(Event::Changed);
        if pinned.is_some() {
            self.flush();
        }
        Ok(())
    }
    fn run(&mut self, commands: Receiver<QueuedCommand>) -> bool {
        loop {
            // recv_timeout returns queued commands even with a zero timeout; explicitly
            // service the deadline so sustained input cannot postpone a dirty batch.
            if self
                .deadline
                .is_some_and(|deadline| deadline <= Instant::now())
            {
                self.flush();
            }
            let next = match (self.deadline, self.checkpoint_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let received = if let Some(next) = next {
                commands.recv_timeout(next.saturating_duration_since(Instant::now()))
            } else {
                commands.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            let command = match received {
                Ok(queued) => {
                    self.queued_bytes.fetch_sub(queued.bytes, Ordering::AcqRel);
                    match queued.command {
                        Ok(command) => command,
                        Err(message) => {
                            self.error(message);
                            continue;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.deadline.is_some_and(|d| d <= Instant::now()) {
                        self.flush();
                    }
                    if !self.dirty() && self.checkpoint_at.is_some_and(|d| d <= Instant::now()) {
                        self.repo.checkpoint();
                        self.checkpoint_at = None;
                    }
                    // Defer maintenance during a failed dirty batch to avoid a zero-timeout loop.
                    if self.dirty() && self.checkpoint_at.is_some_and(|d| d <= Instant::now()) {
                        self.checkpoint_at = None;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.flush();
                    break;
                }
            };
            let result = match command {
                Command::Capture(snapshot) => self.capture(snapshot),
                Command::Search(query) => self.search(query),
                Command::Load { id, request_id } => self
                    .lookup(&id, true)
                    .map(|item| self.emit(Event::Loaded { request_id, item })),
                Command::SaveFavorite(favorite) => self.favorite(favorite),
                Command::SaveSettings(settings) => {
                    self.settings = settings;
                    self.settings_dirty = true;
                    self.mark_dirty();
                    self.emit(Event::Changed);
                    self.flush();
                    Ok(())
                }
                Command::Delete(id) => {
                    self.pending.remove(&id);
                    self.deleted.insert(id);
                    self.mark_dirty();
                    self.emit(Event::Changed);
                    Ok(())
                }
                Command::Pin { id, pinned } => self.modify(id, Some(pinned)),
                Command::Used(id) => self.modify(id, None),
                Command::Flush => {
                    self.flush();
                    Ok(())
                }
                Command::CancelShutdown => {
                    self.exiting = false;
                    Ok(())
                }
                Command::Shutdown => {
                    self.exiting = true;
                    if self.flush() {
                        // Drop SQLite before the final acknowledgement.
                        self.repo.checkpoint();
                        return true;
                    }
                    Ok(())
                }
            };
            if let Err(message) = result {
                self.error(message);
            }
        }
        false
    }
}
