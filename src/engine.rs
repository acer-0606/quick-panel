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
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct QueuedCommand {
    command: Option<Result<Command, String>>,
    bytes: usize,
}
// A single replaceable slot, shared across all three result-query modes.
#[derive(Default)]
struct Queries {
    latest: Mutex<Option<(u64, Command)>>,
    generation: AtomicU64,
    revision: AtomicU64,
    semantic_revision: AtomicU64,
    reliable_pending: AtomicUsize,
}
impl Queries {
    fn note_reliable(&self, command: &Command) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        if matches!(
            command,
            Command::Capture(_)
                | Command::StoreEmbeddings { .. }
                | Command::SaveFavorite(_)
                | Command::EditClipboardMetadata(_)
                | Command::SaveSettings(_)
                | Command::Delete(_)
                | Command::Pin { .. }
                | Command::Used(_)
        ) {
            self.semantic_revision.fetch_add(1, Ordering::AcqRel);
        }
    }
}
const QUERY_YIELDED: &str = "internal: query yielded";
struct Commands {
    sender: Option<Sender<QueuedCommand>>,
    queries: Arc<Queries>,
    exiting: bool,
    queued_bytes: Arc<AtomicUsize>,
    queued_path_bytes: Arc<AtomicUsize>,
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
        let queued_path_bytes = Arc::new(AtomicUsize::new(0));
        let worker_path_bytes = queued_path_bytes.clone();
        let queries = Arc::new(Queries::default());
        let worker_queries = queries.clone();
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
                    vectors: crate::semantic_store::PendingVectors::default(),
                    semantic_scan: None,
                    status_work: None,
                    source_generation: 0,
                    queries: worker_queries,
                    query_guard: None,
                    deleted: HashSet::new(),
                    settings_dirty: false,
                    deadline: None,
                    checkpoint_at: None,
                    emergency_checkpoint_retry: None,
                    commits: 0,
                    retry_delay_ms: 1_000,
                    retry_not_before: None,
                    queued_bytes: worker_queue_bytes,
                    queued_path_bytes: worker_path_bytes,
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
                queries,
                exiting: false,
                queued_bytes,
                queued_path_bytes,
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
        if matches!(
            &command,
            Command::Search(_) | Command::AiSearch(_) | Command::SemanticSearch(_)
        ) {
            let generation = state.queries.generation.fetch_add(1, Ordering::AcqRel) + 1;
            let mut latest = state
                .queries
                .latest
                .lock()
                .map_err(|_| "query slot poisoned")?;
            let wake = latest.is_none();
            *latest = Some((generation, command));
            if wake {
                state
                    .sender
                    .as_ref()
                    .ok_or("engine stopped")?
                    .send(QueuedCommand {
                        command: None,
                        bytes: 0,
                    })
                    .map_err(|_| "engine stopped")?;
            }
            return Ok(());
        }
        let path_bytes = if let Command::ObservePaths(observations) = &command {
            if observations.is_empty() {
                return Ok(());
            }
            if observations.len() > crate::path_activity::MAX_BATCH
                || observations.iter().any(|o| {
                    o.path
                        .encode_utf16()
                        .take(crate::path_activity::MAX_PATH_UNITS + 1)
                        .count()
                        > crate::path_activity::MAX_PATH_UNITS
                })
            {
                return Err("路径观察批次或路径长度超过上限".into());
            }
            let bytes = observation_bytes(observations, observations.capacity());
            state
                .queued_path_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                    used.checked_add(bytes)
                        .filter(|total| *total <= crate::path_activity::MAX_QUEUE_BYTES)
                })
                .map_err(|_| "路径观察队列已满".to_string())?;
            bytes
        } else {
            0
        };
        state.queries.note_reliable(&command);
        if matches!(command, Command::Shutdown) {
            state.queries.generation.fetch_add(1, Ordering::AcqRel);
            *state
                .queries
                .latest
                .lock()
                .map_err(|_| "query slot poisoned")? = None;
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
            Command::StoreEmbeddings { items, .. } => items
                .iter()
                .map(crate::semantic_store::embedding_bytes)
                .sum(),
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
        state
            .queries
            .reliable_pending
            .fetch_add(1, Ordering::AcqRel);
        let result = state
            .sender
            .as_ref()
            .ok_or("engine stopped")?
            .send(QueuedCommand {
                command: Some(command),
                bytes,
            });
        if result.is_err() {
            state
                .queries
                .reliable_pending
                .fetch_sub(1, Ordering::AcqRel);
            state.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
            state
                .queued_path_bytes
                .fetch_sub(path_bytes, Ordering::AcqRel);
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
            state.queries.generation.fetch_add(1, Ordering::AcqRel);
            *state.queries.latest.lock().unwrap() = None;
            state
                .queries
                .reliable_pending
                .fetch_add(1, Ordering::AcqRel);
            let _ = sender.send(QueuedCommand {
                command: Some(Ok(Command::Shutdown)),
                bytes: 0,
            });
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
// At most one explicit status request is active. This retains only counters,
// cursors and pending IDs, never source bodies or an open SQLite statement.
struct SemanticStatusWork {
    request_id: u64,
    revision: u64,

    source_generation: u64,
    profile: String,
    scope: crate::semantic::IndexScope,
    after_rowid: i64,
    disk_done: bool,
    pending_ids: Vec<String>,
    indexed: usize,
    needs_update: bool,
}

struct Actor {
    repo: Repository,
    options: EngineOptions,
    settings: Settings,
    pending: HashMap<String, PendingItem>,
    vectors: crate::semantic_store::PendingVectors,
    // Latest returned cursor and commit generation. Source commits can move
    // unscanned RAM content into disk rows, including updates to old rowids.
    semantic_scan: Option<(crate::semantic::IndexCursor, u64, String)>,
    source_generation: u64,
    status_work: Option<SemanticStatusWork>,
    queries: Arc<Queries>,
    query_guard: Option<(u64, u64)>,
    deleted: HashSet<String>,
    settings_dirty: bool,
    deadline: Option<Instant>,
    checkpoint_at: Option<Instant>,
    emergency_checkpoint_retry: Option<Instant>,
    commits: u64,
    retry_delay_ms: u64,
    retry_not_before: Option<Instant>,
    queued_bytes: Arc<AtomicUsize>,
    queued_path_bytes: Arc<AtomicUsize>,
    exiting: bool,
    events: Sender<Event>,
    notifier: Arc<dyn Fn() + Send + Sync>,
}
fn observation_bytes(
    observations: &[crate::path_activity::PathObservation],
    capacity: usize,
) -> usize {
    observations.iter().fold(
        capacity.saturating_mul(std::mem::size_of::<crate::path_activity::PathObservation>()),
        |bytes, observation| bytes.saturating_add(observation.path.capacity()),
    )
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
    search::rank_normalized(summary, query)
        + if summary.kind != ItemKind::Clipboard {
            if query.trim().is_empty() {
                5_000
            } else {
                2_000
            }
        } else {
            0
        }
}
fn semantic_result_rank(item: &StoredItem, query: &str) -> i64 {
    let rank = result_rank(&item.summary, query);
    let needle = query;
    if !needle.is_empty()
        && item
            .target
            .as_ref()
            .is_some_and(|target| search::normalize(target).contains(needle))
    {
        rank.max(600_000)
    } else {
        rank
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
    fn query_checkpoint(&self) -> Result<(), String> {
        if let Some((generation, revision)) = self.query_guard
            && (self.queries.generation.load(Ordering::Acquire) != generation
                || self.queries.revision.load(Ordering::Acquire) != revision
                || self.queries.reliable_pending.load(Ordering::Acquire) > 0
                || self.deadline.is_some_and(|d| Instant::now() >= d))
        {
            return Err(QUERY_YIELDED.into());
        }
        Ok(())
    }
    fn emit(&self, event: Event) {
        if matches!(event, Event::Results { .. } | Event::PathSuggestions { .. })
            && self.query_checkpoint().is_err()
        {
            return;
        }
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
        !self.pending.is_empty()
            || self.repo.paths.is_dirty()
            || !self.deleted.is_empty()
            || self.settings_dirty
            || !self.vectors.items.is_empty()
    }
    fn mark_dirty(&mut self) {
        if !self.exiting && self.deadline.is_none() {
            self.deadline =
                Some(Instant::now() + Duration::from_millis(self.options.save_start_delay_ms()));
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
                && pending.mutation != Mutation::Content
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
        if self.exiting {
            return false;
        }
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
        let semantic_status_changed = !self.pending.is_empty()
            || !self.deleted.is_empty()
            || self.settings_dirty
            || !self.vectors.items.is_empty();
        match self.repo.commit(
            &self.pending,
            &self.deleted,
            self.settings_dirty.then_some(&self.settings),
            &self.settings,
            now,
            &self.vectors,
        ) {
            Ok(effects) => {
                let semantic_status_changed = semantic_status_changed || effects.sources_removed;
                if semantic_status_changed {
                    self.queries
                        .semantic_revision
                        .fetch_add(1, Ordering::AcqRel);
                }
                if self
                    .pending
                    .values()
                    .any(|p| p.mutation == Mutation::Content)
                    || !self.deleted.is_empty()
                    || effects.sources_removed
                {
                    self.source_generation += 1;
                }
                self.pending.clear();
                self.vectors = crate::semantic_store::PendingVectors::default();
                self.deleted.clear();
                self.settings_dirty = false;
                self.deadline = None;
                self.commits += 1;
                self.retry_delay_ms = 1_000;
                self.retry_not_before = None;
                self.checkpoint_at
                    .get_or_insert(Instant::now() + Duration::from_secs(300));
                if self.repo.wal_backlog_bytes() >= 16 * 1024 * 1024 {
                    let pressure = Instant::now() + Duration::from_secs(30);
                    self.checkpoint_at = Some(self.checkpoint_at.unwrap().min(pressure));
                }
                if self.repo.wal_backlog_bytes() >= 64 * 1024 * 1024
                    && self
                        .emergency_checkpoint_retry
                        .is_none_or(|when| Instant::now() >= when)
                {
                    let complete = self.repo.checkpoint();
                    if !complete {
                        // A long reader may prevent PASSIVE progress. Do not spin
                        // a checkpoint for every tiny subsequent dirty batch.
                        let retry = Instant::now() + Duration::from_secs(1);
                        self.emergency_checkpoint_retry = Some(retry);
                        self.checkpoint_at = Some(self.checkpoint_at.unwrap().min(retry));
                    } else {
                        self.emergency_checkpoint_retry = None;
                        self.checkpoint_at = None;
                    }
                }
                self.emit(Event::Saved(SaveStats {
                    semantic_status_changed,
                    pending_items: 0,
                    pending_bytes: 0,
                    commits: self.commits,
                    last_saved_at_ms: now,
                }));
                true
            }
            Err(message) => {
                self.retry_not_before =
                    Some(Instant::now() + Duration::from_millis(self.retry_delay_ms));
                // A failed exit waits for the explicit modal decision. Background
                // retry must not save/exit or accumulate prompts behind that dialog.
                self.deadline = if self.exiting {
                    None
                } else {
                    self.retry_not_before
                };
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
        self.source_generation += 1;
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
    fn collect_search(
        &self,
        query: &SearchQuery,
        from_ms: Option<i64>,
        until_ms: Option<i64>,
    ) -> Result<Vec<ItemSummary>, String> {
        self.query_checkpoint()?;
        let needle = search::normalize(query.text.trim());
        let saved = self
            .repo
            .summaries_in_period(query, from_ms, until_ms, || self.query_checkpoint())?;
        let mut items: HashMap<String, ItemSummary> = saved
            .into_iter()
            .filter(|s| {
                !self.deleted.contains(&s.id)
                    && !self
                        .pending
                        .get(&s.id)
                        .is_some_and(|p| p.mutation != Mutation::Metadata)
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
            self.query_checkpoint()?;
            let created = p.item.summary.created_at_ms;
            if from_ms.is_some_and(|from| created < from)
                || until_ms.is_some_and(|until| created >= until)
            {
                continue;
            }
            if p.mutation != Mutation::Content && items.contains_key(&p.item.summary.id) {
                continue;
            }
            let body;
            let item = if p.mutation != Mutation::Content
                && !search::matches_normalized(&p.item, &needle)
            {
                body = self.semantic_item(&p.item.summary.id)?;
                let Some(body) = body.as_ref() else {
                    continue;
                };
                body
            } else {
                &p.item
            };
            if scope_matches(p.item.summary.kind, query.scope)
                && search::matches_normalized(item, &needle)
            {
                items.insert(p.item.summary.id.clone(), p.item.summary.clone());
            }
        }
        let mut items: Vec<_> = items.into_values().collect();
        items.sort_by(|a, b| {
            result_rank(b, &needle)
                .cmp(&result_rank(a, &needle))
                .then(b.pinned.cmp(&a.pinned))
                .then(b.last_used_ms.cmp(&a.last_used_ms))
                .then(a.id.cmp(&b.id))
        });
        items.truncate(50);
        for summary in items.iter_mut().filter(|_| !needle.is_empty()) {
            self.query_checkpoint()?;
            if let Some(item) = self.semantic_item(&summary.id)? {
                summary.subtitle = search::body_subtitle(&item, &needle);
            }
        }
        Ok(items)
    }
    fn emit_path_suggestions(
        &self,
        request_id: u64,
        terms: &[String],
        scope: Scope,
        period: (Option<i64>, Option<i64>),
    ) -> Result<(), String> {
        self.query_checkpoint()?;
        let items = self.repo.paths.suggestions(
            crate::path_activity_store::PathSearch {
                terms,
                scope,
                now: now_ms(),
                from_ms: period.0,
                until_ms: period.1,
            },
            &self.settings.path_activity,
            || self.query_checkpoint(),
        )?;
        self.query_checkpoint()?;
        self.emit(Event::PathSuggestions { request_id, items });
        Ok(())
    }
    fn search(&self, query: SearchQuery) -> Result<(), String> {
        let items = self.collect_search(&query, None, None)?;
        self.emit_path_suggestions(
            query.request_id,
            std::slice::from_ref(&query.text),
            query.scope,
            (None, None),
        )?;
        self.emit(Event::Results {
            request_id: query.request_id,
            items,
        });
        Ok(())
    }
    fn ai_search(&self, query: AiSearchQuery) -> Result<(), String> {
        if query.queries.len() > 6
            || query
                .queries
                .iter()
                .any(|term| term.chars().take(65).count() > 64)
        {
            return Err("AI 检索最多接受 6 个关键词，每个不超过 64 个字符".into());
        }
        if matches!((query.from_ms, query.until_ms), (Some(from), Some(until)) if from >= until) {
            return Err("AI 检索时间范围无效".into());
        }
        let mut seen = HashSet::new();
        let mut terms: Vec<_> = query
            .queries
            .iter()
            .map(|term| term.trim())
            .filter(|term| !term.is_empty() && seen.insert(term.to_lowercase()))
            .map(str::to_owned)
            .collect();
        if terms.is_empty() {
            if query.scope == Scope::All && query.from_ms.is_none() && query.until_ms.is_none() {
                return Err("AI 检索需要关键词、内容范围或时间条件".into());
            }
            terms.push(String::new());
        }
        // Every term uses the same ordered top 50 as ordinary search. An entry
        // outside all six top-50 lists cannot enter the global top 50 under the
        // same rank/tie ordering, so the merge holds at most 300 real IDs.
        let mut merged: HashMap<String, (ItemSummary, i64)> = HashMap::new();
        for text in &terms {
            let local = SearchQuery {
                text: text.clone(),
                scope: query.scope,
                request_id: query.request_id,
            };
            let needle = search::normalize(local.text.trim());
            for summary in self.collect_search(&local, query.from_ms, query.until_ms)? {
                let rank = result_rank(&summary, &needle);
                merged
                    .entry(summary.id.clone())
                    .and_modify(|entry| entry.1 = entry.1.max(rank))
                    .or_insert((summary, rank));
            }
        }
        let mut items: Vec<_> = merged.into_values().collect();
        items.sort_by(|(a, a_rank), (b, b_rank)| {
            b_rank
                .cmp(a_rank)
                .then(b.pinned.cmp(&a.pinned))
                .then(b.last_used_ms.cmp(&a.last_used_ms))
                .then(a.id.cmp(&b.id))
        });
        items.truncate(50);
        self.emit_path_suggestions(
            query.request_id,
            &terms,
            query.scope,
            (query.from_ms, query.until_ms),
        )?;
        self.emit(Event::Results {
            request_id: query.request_id,
            items: items.into_iter().map(|(summary, _)| summary).collect(),
        });
        Ok(())
    }
    fn new_status_work(&self, request_id: u64) -> Result<SemanticStatusWork, String> {
        Ok(SemanticStatusWork {
            request_id,
            revision: self.queries.semantic_revision.load(Ordering::Acquire),
            source_generation: self.source_generation,
            profile: crate::semantic::profile_id(&self.settings.embedding)?,
            scope: self.settings.embedding.index_scope,
            after_rowid: 0,
            disk_done: false,
            pending_ids: Vec::new(),
            indexed: 0,
            needs_update: false,
        })
    }
    fn semantic_status(&mut self, request_id: u64) -> Result<(), String> {
        self.status_work = None;
        self.status_work = Some(self.new_status_work(request_id)?);
        Ok(())
    }
    fn status_should_yield(&self, work: &SemanticStatusWork) -> bool {
        self.exiting
            || self.queries.reliable_pending.load(Ordering::Acquire) > 0
            || self.queries.semantic_revision.load(Ordering::Acquire) != work.revision
            || self
                .deadline
                .is_some_and(|deadline| deadline <= Instant::now())
    }
    fn inspect_status_item(
        &self,
        work: &mut SemanticStatusWork,
        item: StoredItem,
    ) -> Result<(), String> {
        if let Some(candidate) = crate::semantic::candidate(&item, work.scope) {
            if self.needs_embedding(&work.profile, &candidate)? {
                work.needs_update = true;
            } else {
                work.indexed += 1;
            }
        }
        Ok(())
    }
    fn status_step(&mut self) -> Result<(), String> {
        let Some(mut work) = self.status_work.take() else {
            return Ok(());
        };
        if self.exiting {
            return Ok(());
        }
        // RAM source/vector changes and commits with semantic effects invalidate
        // counts. Path-only reliable work yields scheduling without losing pages.
        if work.revision != self.queries.semantic_revision.load(Ordering::Acquire)
            || work.source_generation != self.source_generation
            || work.scope != self.settings.embedding.index_scope
            || work.profile != crate::semantic::profile_id(&self.settings.embedding)?
        {
            work = self.new_status_work(work.request_id)?;
        }
        if self.status_should_yield(&work) {
            self.status_work = Some(work);
            return Ok(());
        }
        if !work.disk_done {
            let mut visited = 0;
            let (last, done) =
                crate::semantic_store::page(&self.repo.connection, work.after_rowid, |item| {
                    if !self.deleted.contains(&item.summary.id)
                        && !self.pending.contains_key(&item.summary.id)
                    {
                        self.inspect_status_item(&mut work, item)?;
                    }
                    visited += 1;
                    // The callback has consumed this row, so retaining its rowid
                    // resumes without skipping or double-counting a source.
                    Ok(visited < 16 && !self.status_should_yield(&work))
                })?;
            work.after_rowid = last;
            work.disk_done = done;
            if done {
                work.pending_ids = self.pending.keys().cloned().collect();
            }
        } else {
            for _ in 0..16 {
                if self.status_should_yield(&work) {
                    break;
                }
                let Some(id) = work.pending_ids.pop() else {
                    break;
                };
                if let Some(item) = self.semantic_item(&id)? {
                    self.inspect_status_item(&mut work, item)?;
                }
            }
        }
        if work.disk_done && work.pending_ids.is_empty() && !self.status_should_yield(&work) {
            self.emit(Event::SemanticStatus {
                request_id: work.request_id,
                status: SemanticIndexStatus {
                    profile: work.profile,
                    indexed: work.indexed,
                    needs_update: work.needs_update,
                },
            });
        } else {
            self.status_work = Some(work);
        }
        Ok(())
    }
    fn edit_clipboard_metadata(&mut self, input: ClipboardMetadataInput) -> Result<(), String> {
        let input = crate::smart_search::normalize_metadata(input)?;
        let Some(mut item) = self.lookup(&input.id, false)? else {
            return Err("剪贴板条目不存在".into());
        };
        if item.summary.kind != ItemKind::Clipboard || !item.summary.pinned {
            return Err("仅可编辑已固定的剪贴板条目".into());
        }
        item.summary.title = input.title;
        item.summary.alias = input.alias;
        item.summary.tags = input.tags;
        let mutation = if self
            .pending
            .get(&input.id)
            .is_some_and(|p| p.mutation == Mutation::Content)
        {
            Mutation::Content
        } else {
            Mutation::SearchMetadata
        };
        self.pending
            .insert(input.id, PendingItem { item, mutation });
        self.source_generation += 1;
        self.mark_dirty();
        self.emit(Event::Changed);
        self.flush();
        Ok(())
    }
    fn semantic_profile(&self) -> Result<String, String> {
        if !self.settings.embedding.enabled {
            return Err("语义检索未启用".into());
        }
        crate::semantic::profile_id(&self.settings.embedding)
    }
    // Metadata-only actor changes intentionally omit text. Restore only text,
    // never representations, and keep the current metadata overlay.
    fn semantic_item(&self, id: &str) -> Result<Option<StoredItem>, String> {
        if self.deleted.contains(id) {
            return Ok(None);
        }
        if let Some(change) = self.pending.get(id) {
            let mut item = StoredItem {
                summary: change.item.summary.clone(),
                text: change.item.text.clone(),
                target: change.item.target.clone(),
                open_with: change.item.open_with.clone(),
                representations: vec![],
            };
            if change.mutation != Mutation::Content {
                let Some(saved) = self.repo.load(id, false)? else {
                    return Ok(None);
                };
                item.text = saved.text;
            }
            return Ok(Some(item));
        }
        self.repo.load(id, false)
    }
    fn needs_embedding(
        &self,
        profile: &str,
        candidate: &crate::semantic::EmbeddingCandidate,
    ) -> Result<bool, String> {
        if self.vectors.profile == profile
            && let Some(vector) = self.vectors.items.get(&candidate.id)
        {
            return Ok(vector.text_hash != candidate.text_hash);
        }
        Ok(!crate::semantic_store::indexed(
            &self.repo.connection,
            profile,
            &candidate.id,
            &candidate.text_hash,
        )?)
    }
    fn prepare_semantic_index(
        &mut self,
        request_id: u64,
        mut cursor: crate::semantic::IndexCursor,
    ) -> Result<(), String> {
        let profile = self.semantic_profile()?;
        let continuation = self
            .semantic_scan
            .as_ref()
            .filter(|(previous, _, scanned_profile)| {
                previous == &cursor && scanned_profile == &profile
            });
        if continuation.is_some_and(|(_, generation, _)| *generation != self.source_generation)
            || (cursor.disk_done && continuation.is_none())
        {
            // A successful commit may have cleared unseen pending sources or
            // updated earlier disk rowids. Restart the bounded disk scan; hashes
            // skip completed vectors, so no completed inputs are sent again.
            cursor = crate::semantic::IndexCursor::default();
        }
        let mut candidates = Vec::new();
        if !cursor.disk_done {
            let (last, done) = crate::semantic_store::page(
                &self.repo.connection,
                cursor.after_rowid,
                |mut item| {
                    // Pending content is scanned once in the memory stage instead.
                    if self.deleted.contains(&item.summary.id)
                        || self
                            .pending
                            .get(&item.summary.id)
                            .is_some_and(|p| p.mutation == Mutation::Content)
                    {
                        return Ok(true);
                    }
                    if let Some(p) = self.pending.get(&item.summary.id) {
                        item.summary = p.item.summary.clone()
                    }
                    if let Some(candidate) =
                        crate::semantic::candidate(&item, self.settings.embedding.index_scope)
                        && self.needs_embedding(&profile, &candidate)?
                    {
                        candidates.push(candidate)
                    }
                    Ok(candidates.len() < 16)
                },
            )?;
            cursor.after_rowid = last;
            cursor.disk_done = done;
            // Disk pages inspect at most 128 sources per command, even if none
            // need embeddings. The controller follows the returned cursor.
            self.semantic_scan = Some((cursor.clone(), self.source_generation, profile.clone()));
            self.emit(Event::SemanticBatch(crate::semantic::IndexBatch {
                request_id,
                profile,
                candidates,
                cursor,
                done: false,
            }));
            return Ok(());
        }
        let mut ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(id, p)| {
                p.mutation == Mutation::Content
                    && cursor
                        .pending_after_id
                        .as_ref()
                        .is_none_or(|after| *id > after)
            })
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        let total = ids.len();
        let mut visited = 0;
        for id in ids.into_iter().take(128) {
            cursor.pending_after_id = Some(id.clone());
            visited += 1;
            if let Some(item) = self.semantic_item(&id)?
                && let Some(candidate) =
                    crate::semantic::candidate(&item, self.settings.embedding.index_scope)
                && self.needs_embedding(&profile, &candidate)?
            {
                candidates.push(candidate);
                if candidates.len() == 16 {
                    break;
                }
            }
        }
        let done = visited == total;
        self.semantic_scan = Some((cursor.clone(), self.source_generation, profile.clone()));
        self.emit(Event::SemanticBatch(crate::semantic::IndexBatch {
            request_id,
            profile,
            candidates,
            cursor,
            done,
        }));
        Ok(())
    }
    fn store_embeddings(
        &mut self,
        request_id: u64,
        profile: String,
        items: Vec<crate::semantic::IndexedEmbedding>,
    ) -> Result<(), String> {
        if self.exiting || self.semantic_profile()? != profile {
            return Err("语义索引配置已变化".into());
        }
        if items.is_empty() {
            self.emit(Event::EmbeddingsStored {
                request_id,
                accepted: 0,
            });
            return Ok(());
        }
        if items.len() > 16 {
            return Err("语义索引批次过大".into());
        }
        // Validate every vector before even attempting a pressure commit, so a
        // malformed later entry cannot admit or persist an earlier entry.
        let dimensions = crate::semantic::validate_vectors(
            &items.iter().map(|i| i.vector.clone()).collect::<Vec<_>>(),
        )?;
        let mut seen = HashSet::new();
        if items.iter().any(|i| !seen.insert(&i.id)) {
            return Err("语义索引批次包含重复条目".into());
        }
        if self.vectors.profile == profile
            && !self.vectors.items.is_empty()
            && self.vectors.dimensions != dimensions
        {
            return Err("语义向量维度不一致".into());
        }
        if crate::semantic_store::active(&self.repo.connection)?
            .is_some_and(|(p, d)| p == profile && d != dimensions)
        {
            return Err("语义向量维度不一致".into());
        }
        let mut accepted = Vec::new();
        for mut item in items {
            if let Some(source) = self.semantic_item(&item.id)?
                && crate::semantic::candidate(&source, self.settings.embedding.index_scope)
                    .is_some_and(|c| c.text_hash == item.text_hash)
            {
                item.vector = crate::semantic_store::normalize(&item.vector);
                accepted.push(item);
            }
        }
        if !accepted.is_empty()
            && crate::semantic_store::PendingVectors::default()
                .projected_bytes(profile.capacity(), &accepted)
                > crate::semantic_store::MAX_PENDING_BYTES
        {
            return Err("待保存语义缓存已满".into());
        }
        let pressure = !accepted.is_empty()
            && (self.vectors.projected_bytes(profile.capacity(), &accepted)
                > crate::semantic_store::MAX_PENDING_BYTES
                || (!self.vectors.items.is_empty() && self.vectors.profile != profile));
        if pressure {
            if self
                .retry_not_before
                .is_some_and(|when| Instant::now() < when)
            {
                return Err("待保存语义缓存已满，保存失败".into());
            }
            if !self.pressure_flush() {
                // flush already emitted the storage failure. No receipt and no
                // second error: the controller stops this indexing session.
                return Ok(());
            }
            // Retention is part of a pressure transaction and may have removed
            // sources that were valid before it. Recheck before the RAM change.
            let mut survivors = Vec::new();
            for item in accepted {
                if let Some(source) = self.semantic_item(&item.id)?
                    && crate::semantic::candidate(&source, self.settings.embedding.index_scope)
                        .is_some_and(|c| c.text_hash == item.text_hash)
                {
                    survivors.push(item)
                }
            }
            accepted = survivors;
        }
        let count = accepted.len();
        if count > 0 {
            self.vectors.profile = profile;
            self.vectors.dimensions = dimensions;
            for item in accepted {
                self.vectors.items.insert(item.id.clone(), item);
            }
            debug_assert!(self.vectors.bytes() <= crate::semantic_store::MAX_PENDING_BYTES);
            self.mark_dirty();
        }
        self.emit(Event::EmbeddingsStored {
            request_id,
            accepted: count,
        });
        Ok(())
    }
    fn semantic_path_candidates(&self, query: &SearchQuery) -> Result<Vec<StoredItem>, String> {
        let needle = search::normalize(query.text.trim());
        if needle.is_empty() {
            return Ok(vec![]);
        }
        let mut items = Vec::new();
        let mut cursor = i64::MAX;
        loop {
            self.query_checkpoint()?;
            let page = self.repo.path_page(query.scope, cursor)?;
            if page.is_empty() {
                break;
            }
            cursor = page.last().unwrap().0;
            let exhausted = page.len() < 128;
            for (_, mut item) in page {
                self.query_checkpoint()?;
                if self.deleted.contains(&item.summary.id)
                    || self
                        .pending
                        .get(&item.summary.id)
                        .is_some_and(|p| p.mutation == Mutation::Content)
                {
                    continue;
                }
                if let Some(pending) = self.pending.get(&item.summary.id) {
                    item.summary = pending.item.summary.clone();
                }
                if item
                    .target
                    .as_ref()
                    .is_some_and(|target| search::normalize(target).contains(&needle))
                {
                    items.push(item);
                }
            }
            Self::trim_semantic_paths(&mut items, &needle);
            if exhausted {
                break;
            }
        }
        for pending in self.pending.values() {
            self.query_checkpoint()?;
            let item = &pending.item;
            if pending.mutation == Mutation::Content
                && !self.deleted.contains(&item.summary.id)
                && scope_matches(item.summary.kind, query.scope)
                && item
                    .target
                    .as_ref()
                    .is_some_and(|target| search::normalize(target).contains(&needle))
            {
                items.push(StoredItem {
                    summary: item.summary.clone(),
                    target: item.target.clone(),
                    text: String::new(),
                    representations: vec![],
                    open_with: None,
                });
                Self::trim_semantic_paths(&mut items, &needle);
            }
        }
        Self::trim_semantic_paths(&mut items, &needle);
        Ok(items)
    }
    fn trim_semantic_paths(items: &mut Vec<StoredItem>, query: &str) {
        items.sort_by(|a, b| {
            semantic_result_rank(b, query)
                .cmp(&semantic_result_rank(a, query))
                .then(b.summary.pinned.cmp(&a.summary.pinned))
                .then(b.summary.last_used_ms.cmp(&a.summary.last_used_ms))
                .then(a.summary.id.cmp(&b.summary.id))
        });
        items.truncate(50);
    }
    fn semantic_search(&self, query: crate::semantic::SemanticSearchQuery) -> Result<(), String> {
        if self.semantic_profile()? != query.profile {
            return Err("语义检索配置已变化".into());
        }
        crate::semantic::validate_vectors(std::slice::from_ref(&query.vector))?;
        let needle = search::normalize(query.text.trim());
        let lexical = SearchQuery {
            text: query.text.clone(),
            scope: query.scope,
            request_id: query.request_id,
        };
        let mut merged: HashMap<String, (ItemSummary, i64, Option<f32>)> = HashMap::new();
        for summary in self.collect_search(&lexical, None, None)? {
            if let Some(item) = self.semantic_item(&summary.id)? {
                let rank = semantic_result_rank(&item, &needle);
                merged.insert(summary.id.clone(), (summary, rank, None));
            }
        }
        for item in self.semantic_path_candidates(&lexical)? {
            let rank = semantic_result_rank(&item, &needle);
            merged.insert(item.summary.id.clone(), (item.summary, rank, None));
        }
        let mut excluded = self.deleted.clone();
        excluded.extend(
            self.pending
                .iter()
                .filter(|(_, p)| p.mutation != Mutation::Metadata)
                .map(|(id, _)| id.clone()),
        );
        // RAM coordinates replace disk coordinates even when subsequently stale.
        excluded.extend(self.vectors.items.keys().cloned());
        let pins = self
            .pending
            .iter()
            .map(|(id, p)| (id.clone(), p.item.summary.pinned))
            .collect();
        let mut nearest = crate::semantic_store::nearest(
            &self.repo.connection,
            &query.profile,
            &query.vector,
            query.scope,
            self.settings.embedding.index_scope,
            &excluded,
            &pins,
        )?;
        // Content changes with the same embedding hash keep their disk vector.
        // Read those coordinates individually and rank the current source shape;
        // this also handles a changed kind/scope before the global result limit.
        for (id, change) in &self.pending {
            self.query_checkpoint()?;
            if change.mutation == Mutation::Metadata || self.vectors.items.contains_key(id) {
                continue;
            }
            if let Some(item) = self.semantic_item(id)?
                && scope_matches(item.summary.kind, query.scope)
                && let Some(candidate) =
                    crate::semantic::candidate(&item, self.settings.embedding.index_scope)
                && let Some(vector) = crate::semantic_store::matching_vector(
                    &self.repo.connection,
                    &query.profile,
                    id,
                    &candidate.text_hash,
                    query.vector.len(),
                )?
            {
                nearest.push((
                    id.clone(),
                    crate::semantic::cosine_distance(&vector, &query.vector)?,
                ));
            }
        }
        if self.vectors.profile == query.profile && self.vectors.dimensions == query.vector.len() {
            for vector in self.vectors.items.values() {
                self.query_checkpoint()?;
                if let Some(item) = self.semantic_item(&vector.id)?
                    && scope_matches(item.summary.kind, query.scope)
                    && crate::semantic::candidate(&item, self.settings.embedding.index_scope)
                        .is_some_and(|c| c.text_hash == vector.text_hash)
                {
                    nearest.push((
                        vector.id.clone(),
                        crate::semantic::cosine_distance(&vector.vector, &query.vector)?,
                    ));
                }
            }
        }
        for (id, distance) in nearest {
            let Some(item) = self.semantic_item(&id)? else {
                continue;
            };
            let rank = semantic_result_rank(&item, &needle);
            merged
                .entry(id)
                .and_modify(|entry| entry.2 = Some(distance))
                .or_insert((item.summary, rank, Some(distance)));
        }
        let mut items: Vec<_> = merged.into_values().collect();
        items.sort_by(|(a, ar, ad), (b, br, bd)| {
            // Explicit heading/path matches retain ordinary search precedence.
            let ae = *ar >= 600_000;
            let be = *br >= 600_000;
            be.cmp(&ae)
                .then_with(|| {
                    if ae && be {
                        br.cmp(ar)
                    } else {
                        ad.unwrap_or(2.).total_cmp(&bd.unwrap_or(2.))
                    }
                })
                .then(br.cmp(ar))
                .then(b.pinned.cmp(&a.pinned))
                .then(b.last_used_ms.cmp(&a.last_used_ms))
                .then(a.id.cmp(&b.id))
        });
        items.truncate(50);
        self.emit_path_suggestions(
            query.request_id,
            std::slice::from_ref(&query.text),
            query.scope,
            (None, None),
        )?;
        self.emit(Event::Results {
            request_id: query.request_id,
            items: items.into_iter().map(|(s, _, _)| s).collect(),
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
        self.source_generation += 1;
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
            if item.summary.pinned != value {
                self.source_generation += 1;
            }
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
            // No SQLite transaction is held between commands, even when RAM is
            // dirty. Service due maintenance here so continuous input cannot starve it.
            if self
                .checkpoint_at
                .is_some_and(|when| when <= Instant::now())
            {
                let complete = self.repo.checkpoint();
                if !complete {
                    let seconds = if self.repo.wal_backlog_bytes() >= 64 * 1024 * 1024 {
                        1
                    } else {
                        5
                    };
                    self.checkpoint_at = Some(Instant::now() + Duration::from_secs(seconds));
                } else {
                    self.checkpoint_at = None;
                    self.emergency_checkpoint_retry = None;
                }
            }
            let next = match (self.deadline, self.checkpoint_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            let queued = match commands.try_recv() {
                Ok(queued) => Some(queued),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    self.flush();
                    break;
                }
            };
            if queued.is_none() {
                let query = self.queries.latest.lock().unwrap().take();
                if let Some((generation, command)) = query {
                    self.query_guard =
                        Some((generation, self.queries.revision.load(Ordering::Acquire)));
                    let result = match command.clone() {
                        Command::Search(query) => self.search(query),
                        Command::AiSearch(query) => self.ai_search(query),
                        Command::SemanticSearch(query) => self.semantic_search(query),
                        _ => unreachable!(),
                    };
                    let interrupted = result.as_ref().is_err_and(|e| e == QUERY_YIELDED)
                        || self.query_checkpoint().is_err();
                    self.query_guard = None;
                    if interrupted {
                        let mut latest = self.queries.latest.lock().unwrap();
                        if self.queries.generation.load(Ordering::Acquire) == generation
                            && latest.is_none()
                        {
                            *latest = Some((generation, command));
                        }
                    } else if let Err(message) = result {
                        self.error(message);
                    }
                    continue;
                }
                if let Some(work) = &self.status_work {
                    let request_id = work.request_id;
                    if let Err(message) = self.status_step() {
                        self.emit(Event::SemanticStatusFailed {
                            request_id,
                            message,
                        });
                    }
                    continue;
                }
            }
            let received = if let Some(queued) = queued {
                Ok(queued)
            } else if let Some(next) = next {
                commands.recv_timeout(next.saturating_duration_since(Instant::now()))
            } else {
                commands.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            let command = match received {
                Ok(queued) => {
                    self.queued_bytes.fetch_sub(queued.bytes, Ordering::AcqRel);
                    if let Some(Ok(Command::ObservePaths(observations))) = &queued.command {
                        self.queued_path_bytes.fetch_sub(
                            observation_bytes(observations, observations.capacity()),
                            Ordering::AcqRel,
                        );
                    }
                    if queued.command.is_some() {
                        self.queries.reliable_pending.fetch_sub(1, Ordering::AcqRel);
                    }
                    match queued.command {
                        None => continue,
                        Some(Ok(command)) => command,
                        Some(Err(message)) => {
                            self.error(message);
                            continue;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if self.deadline.is_some_and(|d| d <= Instant::now()) {
                        self.flush();
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.flush();
                    break;
                }
            };
            let result = match command {
                Command::ObservePaths(observations) => {
                    if self
                        .repo
                        .paths
                        .observe(observations, &self.settings.path_activity, now_ms())
                    {
                        self.mark_dirty();
                        self.emit(Event::PathActivityChanged);
                    }
                    Ok(())
                }
                Command::SuppressPath(path) => match self.repo.paths.suppress(&path) {
                    Ok(true) => {
                        self.mark_dirty();
                        self.emit(Event::PathActivityChanged);
                        self.flush();
                        Ok(())
                    }
                    Ok(false) => Ok(()),
                    Err(message) => Err(message),
                },
                Command::ClearPathActivity => {
                    if self.repo.paths.clear() {
                        self.mark_dirty();
                        self.emit(Event::PathActivityChanged);
                        self.flush();
                    }
                    Ok(())
                }
                Command::Capture(snapshot) => self.capture(snapshot),
                Command::Search(query) => self.search(query),
                Command::AiSearch(query) => self.ai_search(query),
                Command::PrepareSemanticIndex { request_id, cursor } => {
                    self.prepare_semantic_index(request_id, cursor)
                }
                Command::StoreEmbeddings {
                    request_id,
                    profile,
                    items,
                } => self.store_embeddings(request_id, profile, items),
                Command::SemanticSearch(query) => self.semantic_search(query),
                Command::Load { id, request_id } => {
                    match self.lookup(&id, true) {
                        Ok(item) => self.emit(Event::Loaded { request_id, item }),
                        Err(message) => self.emit(Event::LoadFailed {
                            request_id,
                            message,
                        }),
                    }
                    Ok(())
                }
                Command::SaveFavorite(favorite) => self.favorite(favorite),
                Command::EditClipboardMetadata(input) => self.edit_clipboard_metadata(input),
                Command::SemanticStatus { request_id } => {
                    if let Err(message) = self.semantic_status(request_id) {
                        self.emit(Event::SemanticStatusFailed {
                            request_id,
                            message,
                        });
                    }
                    Ok(())
                }
                Command::SaveSettings(settings) => {
                    if self.settings.embedding != settings.embedding {
                        self.source_generation += 1;
                    }
                    let paths_changed = self.settings.path_activity != settings.path_activity;
                    self.settings = settings;
                    if paths_changed {
                        self.emit(Event::PathActivityChanged);
                    }
                    self.settings_dirty = true;
                    self.mark_dirty();
                    self.emit(Event::Changed);
                    self.flush();
                    Ok(())
                }
                Command::Delete(id) => {
                    let item = self.lookup(&id, false);
                    let immediate = match item {
                        Ok(Some(item)) => {
                            item.summary.kind != ItemKind::Clipboard || item.summary.pinned
                        }
                        Ok(None) => continue,
                        Err(message) => {
                            self.error(message);
                            continue;
                        }
                    };
                    self.pending.remove(&id);
                    self.deleted.insert(id);
                    self.mark_dirty();
                    self.emit(Event::Changed);
                    if immediate {
                        self.flush();
                    }
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
                    if self.dirty() {
                        self.deadline = Some(self.retry_not_before.unwrap_or_else(Instant::now));
                    }
                    Ok(())
                }
                Command::Shutdown => {
                    if let Some(work) = self.status_work.take() {
                        self.emit(Event::SemanticStatusFailed {
                            request_id: work.request_id,
                            message: "本地索引状态检查已取消".into(),
                        });
                    }
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

#[cfg(test)]
mod status_scheduling_tests {
    use super::*;

    fn actor_fixture() -> (Actor, Receiver<Event>) {
        let (events, received) = mpsc::channel();
        let mut settings = Settings::default();
        settings.embedding.enabled = true;
        settings.embedding.model = "status-fixture".into();
        settings.embedding.index_scope = crate::semantic::IndexScope::AllText;
        let actor = Actor {
            repo: Repository::open(std::path::Path::new(":memory:")).unwrap(),
            options: EngineOptions::new(":memory:".into()),
            settings,
            pending: HashMap::new(),
            vectors: crate::semantic_store::PendingVectors::default(),
            semantic_scan: None,
            status_work: None,
            source_generation: 0,
            queries: Arc::new(Queries::default()),
            query_guard: None,
            deleted: HashSet::new(),
            settings_dirty: false,
            deadline: None,
            checkpoint_at: None,
            emergency_checkpoint_retry: None,
            commits: 0,
            retry_delay_ms: 1000,
            retry_not_before: None,
            queued_bytes: Arc::new(AtomicUsize::new(0)),
            queued_path_bytes: Arc::new(AtomicUsize::new(0)),
            exiting: false,
            events,
            notifier: Arc::new(|| {}),
        };
        (actor, received)
    }

    #[test]
    fn status_request_defers_work_to_scheduler_without_publishing_inline() {
        let (mut actor, events) = actor_fixture();
        actor.semantic_status(91).unwrap();
        assert!(
            matches!(events.try_recv(), Err(TryRecvError::Empty)),
            "a status request must schedule bounded work, not finish its scan inline"
        );
    }
    fn seed_sources(
        actor: &mut Actor,
        events: &Receiver<Event>,
        count: usize,
        indexed: bool,
        persist: bool,
    ) {
        for i in 0..count {
            actor
                .capture(ClipboardSnapshot {
                    text: format!("状态来源 {i}"),
                    representations: vec![],
                    source_app: "fixture".into(),
                    captured_at_ms: now_ms(),
                })
                .unwrap();
        }
        if indexed {
            let profile = actor.semantic_profile().unwrap();
            for items in ram_vectors(actor).chunks(16) {
                actor
                    .store_embeddings(1, profile.clone(), items.to_vec())
                    .unwrap();
            }
        }
        if persist {
            assert!(actor.flush());
        }
        while events.try_recv().is_ok() {}
    }
    fn ram_vectors(actor: &Actor) -> Vec<crate::semantic::IndexedEmbedding> {
        actor
            .pending
            .values()
            .map(|p| {
                let candidate =
                    crate::semantic::candidate(&p.item, actor.settings.embedding.index_scope)
                        .unwrap();
                crate::semantic::IndexedEmbedding {
                    id: candidate.id,
                    text_hash: candidate.text_hash,
                    vector: vec![1., 0.],
                }
            })
            .collect()
    }
    fn queue(sender: &Sender<QueuedCommand>, queries: &Queries, command: Command) {
        queries.note_reliable(&command);
        queries.reliable_pending.fetch_add(1, Ordering::AcqRel);
        sender
            .send(QueuedCommand {
                command: Some(Ok(command)),
                bytes: 0,
            })
            .unwrap();
    }
    fn drive_until_status(
        actor: &mut Actor,
        events: &Receiver<Event>,
        commands: Vec<Command>,
    ) -> (SemanticIndexStatus, usize) {
        let queries = actor.queries.clone();
        let (sender, received) = mpsc::channel();
        for command in commands {
            queue(&sender, &queries, command);
        }
        thread::scope(|scope| {
            // Local ownership drops the live channel before scoped joining on unwind.
            let sender = sender;
            let worker = scope.spawn(|| actor.run(received));
            let mut saved = 0;
            let status = loop {
                match events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    Event::Saved(_) => saved += 1,
                    Event::SemanticStatus { status, .. } => break status,
                    Event::Error { message, .. } => panic!("{message}"),
                    _ => (),
                }
            };
            queue(&sender, &queries, Command::Shutdown);
            assert!(worker.join().unwrap());
            (status, saved)
        })
    }
    #[test]
    fn partial_disk_status_yields_to_due_commit_and_immediate_saves_then_restarts() {
        let (mut actor, events) = actor_fixture();
        seed_sources(&mut actor, &events, 40, true, true);
        actor.semantic_status(92).unwrap();
        actor.status_step().unwrap();
        assert_eq!(actor.status_work.as_ref().unwrap().indexed, 16);
        assert!(events.try_recv().is_err());
        actor
            .capture(ClipboardSnapshot {
                text: "待到期提交".into(),
                representations: vec![],
                source_app: "fixture".into(),
                captured_at_ms: now_ms(),
            })
            .unwrap();
        while events.try_recv().is_ok() {}
        actor.deadline = Some(Instant::now());
        let mut settings = actor.settings.clone();
        settings.embedding.model = "new-profile".into();
        let expected_profile = crate::semantic::profile_id(&settings.embedding).unwrap();
        let (status, saved) = drive_until_status(
            &mut actor,
            &events,
            vec![
                Command::SaveFavorite(FavoriteInput {
                    id: Some("favorite".into()),
                    kind: ItemKind::Document,
                    title: "收藏".into(),
                    target: "C:/favorite.txt".into(),
                    alias: String::new(),
                    tags: vec![],
                    open_with: None,
                }),
                Command::SaveSettings(settings),
            ],
        );
        assert_eq!(
            saved, 3,
            "due commit and both immediate saves must finish before status"
        );
        assert_eq!(status.profile, expected_profile);
        assert_eq!(status.indexed, 0);
        assert!(status.needs_update);
        assert_eq!(actor.commits, 4, "status scanning must not add commits");
        assert_eq!(
            actor
                .repo
                .connection
                .query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            42
        );
    }
    #[test]
    fn partial_pending_status_restarts_for_ram_vectors_and_deleted_overlays() {
        let (mut actor, events) = actor_fixture();
        seed_sources(&mut actor, &events, 40, false, false);
        actor.semantic_status(93).unwrap();
        actor.status_step().unwrap(); // Empty disk, snapshot pending IDs.
        actor.status_step().unwrap(); // Only 16 of the 40 RAM bodies.
        let work = actor.status_work.as_ref().unwrap();
        assert_eq!(work.pending_ids.len(), 24);
        assert!(work.needs_update);
        let profile = actor.semantic_profile().unwrap();
        let vectors = ram_vectors(&actor);
        let mut commands: Vec<_> = vectors
            .chunks(16)
            .map(|items| Command::StoreEmbeddings {
                request_id: 1,
                profile: profile.clone(),
                items: items.to_vec(),
            })
            .collect();
        commands.push(Command::Delete(vectors[0].id.clone()));
        let (status, saved) = drive_until_status(&mut actor, &events, commands);
        assert_eq!(status.indexed, 39);
        assert!(!status.needs_update);
        assert_eq!(
            saved, 0,
            "status plus RAM vector admission must remain read-only"
        );
        assert_eq!(
            actor.commits, 1,
            "only the final Shutdown may persist this test's data"
        );
    }
    #[test]
    fn path_commands_and_commits_keep_partial_status_progress() {
        use crate::path_activity::{ObservationSource, PathObservation};
        let (mut actor, events) = actor_fixture();
        actor.settings.path_activity.enabled = true;
        seed_sources(&mut actor, &events, 96, true, true);
        actor.semantic_status(95).unwrap();
        actor.status_step().unwrap();
        assert_eq!(actor.status_work.as_ref().unwrap().indexed, 16);
        let (sender, received) = mpsc::channel();
        for command in [
            Command::ObservePaths(vec![PathObservation {
                path: "C:/StatusFixture".into(),
                kind: ItemKind::Folder,
                observed_at_ms: now_ms(),
                source: ObservationSource::Explorer,
            }]),
            Command::SuppressPath("C:/StatusFixture".into()),
            Command::ClearPathActivity,
        ] {
            let indexed = actor.status_work.as_ref().unwrap().indexed;
            queue(&sender, &actor.queries, command);
            actor.status_step().unwrap();
            assert_eq!(
                actor.status_work.as_ref().unwrap().indexed,
                indexed,
                "pending path commands must yield without resetting completed pages"
            );
            let queued = received.recv().unwrap();
            actor
                .queries
                .reliable_pending
                .fetch_sub(1, Ordering::AcqRel);
            match queued.command.unwrap().unwrap() {
                Command::ObservePaths(paths) => {
                    assert!(actor.repo.paths.observe(
                        paths,
                        &actor.settings.path_activity,
                        now_ms()
                    ));
                }
                Command::SuppressPath(path) => {
                    assert!(actor.repo.paths.suppress(&path).unwrap());
                }
                Command::ClearPathActivity => {
                    assert!(actor.repo.paths.clear());
                }
                _ => unreachable!(),
            }
            assert!(actor.flush());
            assert!(
                matches!(events.recv().unwrap(), Event::Saved(stats) if !stats.semantic_status_changed)
            );
            actor.status_step().unwrap();
            assert_eq!(
                actor.status_work.as_ref().unwrap().indexed,
                indexed + 16,
                "path-only commits must resume the next status page"
            );
        }
        while actor.status_work.is_some() {
            actor.status_step().unwrap();
        }
        assert!(
            matches!(events.recv().unwrap(), Event::SemanticStatus { status, .. } if status.indexed == 96 && !status.needs_update)
        );
    }
    #[test]
    fn shutdown_discards_a_partial_status_without_finishing_the_scan() {
        let (mut actor, events) = actor_fixture();
        seed_sources(&mut actor, &events, 40, true, false);
        actor.semantic_status(94).unwrap();
        actor.status_step().unwrap();
        actor.status_step().unwrap();
        assert_eq!(actor.status_work.as_ref().unwrap().indexed, 16);
        let (sender, received) = mpsc::channel();
        queue(&sender, &actor.queries, Command::Shutdown);
        assert!(actor.run(received));
        assert!(actor.status_work.is_none());
        let events: Vec<_> = events.try_iter().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::SemanticStatusFailed { request_id: 94, .. })),
            "shutdown must release the front-end state slot even if saving later fails"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::Saved(_)))
                .count(),
            1
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::SemanticStatus { .. } | Event::Error { .. }))
        );
    }
}
