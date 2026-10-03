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
                    vectors: crate::semantic_store::PendingVectors::default(),
                    deleted: HashSet::new(),
                    settings_dirty: false,
                    deadline: None,
                    checkpoint_at: None,
                    emergency_checkpoint_retry: None,
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
    vectors: crate::semantic_store::PendingVectors,
    deleted: HashSet<String>,
    settings_dirty: bool,
    deadline: Option<Instant>,
    checkpoint_at: Option<Instant>,
    emergency_checkpoint_retry: Option<Instant>,
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
        !self.pending.is_empty()
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
        match self.repo.commit(
            &self.pending,
            &self.deleted,
            self.settings_dirty.then_some(&self.settings),
            &self.settings,
            now,
            &self.vectors,
        ) {
            Ok(()) => {
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
        let saved = if from_ms.is_none() && until_ms.is_none() {
            self.repo.summaries(query)?
        } else {
            self.repo.summaries_in_period(query, from_ms, until_ms)?
        };
        let mut items: HashMap<String, ItemSummary> = saved
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
            let created = p.item.summary.created_at_ms;
            if from_ms.is_some_and(|from| created < from)
                || until_ms.is_some_and(|until| created >= until)
            {
                continue;
            }
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
        Ok(items)
    }
    fn search(&self, query: SearchQuery) -> Result<(), String> {
        let items = self.collect_search(&query, None, None)?;
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
        for text in terms {
            let local = SearchQuery {
                text,
                scope: query.scope,
                request_id: query.request_id,
            };
            for summary in self.collect_search(&local, query.from_ms, query.until_ms)? {
                let rank = result_rank(&summary, &local.text);
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
        self.emit(Event::Results {
            request_id: query.request_id,
            items: items.into_iter().map(|(summary, _)| summary).collect(),
        });
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
            if change.mutation == Mutation::Metadata {
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
            && self
                .vectors
                .items
                .get(&candidate.id)
                .is_some_and(|v| v.text_hash == candidate.text_hash)
        {
            return Ok(false);
        }
        Ok(!crate::semantic_store::indexed(
            &self.repo.connection,
            profile,
            &candidate.id,
            &candidate.text_hash,
        )?)
    }
    fn prepare_semantic_index(
        &self,
        request_id: u64,
        mut cursor: crate::semantic::IndexCursor,
    ) -> Result<(), String> {
        let profile = self.semantic_profile()?;
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
        let bytes: usize = accepted
            .iter()
            .map(crate::semantic_store::embedding_bytes)
            .sum();
        if bytes > crate::semantic_store::MAX_PENDING_BYTES {
            return Err("待保存语义缓存已满".into());
        }
        let replacing: usize = accepted
            .iter()
            .filter_map(|v| self.vectors.items.get(&v.id))
            .map(crate::semantic_store::embedding_bytes)
            .sum();
        let pressure = !accepted.is_empty()
            && (self
                .vectors
                .bytes()
                .saturating_sub(replacing)
                .saturating_add(bytes)
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
            self.mark_dirty();
        }
        self.emit(Event::EmbeddingsStored {
            request_id,
            accepted: count,
        });
        Ok(())
    }
    fn semantic_search(&self, query: crate::semantic::SemanticSearchQuery) -> Result<(), String> {
        if self.semantic_profile()? != query.profile {
            return Err("语义检索配置已变化".into());
        }
        crate::semantic::validate_vectors(std::slice::from_ref(&query.vector))?;
        let lexical = SearchQuery {
            text: query.text.clone(),
            scope: query.scope,
            request_id: query.request_id,
        };
        let needle = query.text.trim().to_lowercase();
        let mut merged: HashMap<String, (ItemSummary, i64, Option<f32>)> = HashMap::new();
        for summary in self.collect_search(&lexical, None, None)? {
            let mut rank = result_rank(&summary, &query.text);
            if !needle.is_empty()
                && let Some(item) = self.semantic_item(&summary.id)?
                && item
                    .target
                    .as_ref()
                    .is_some_and(|target| target.to_lowercase().contains(&needle))
            {
                rank = rank.max(600_000)
            }
            merged.insert(summary.id.clone(), (summary, rank, None));
        }
        let mut excluded = self.deleted.clone();
        excluded.extend(
            self.pending
                .iter()
                .filter(|(_, p)| p.mutation == Mutation::Content)
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
            if change.mutation != Mutation::Content || self.vectors.items.contains_key(id) {
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
            merged
                .entry(id)
                .and_modify(|entry| entry.2 = Some(distance))
                .or_insert((item.summary, 0, Some(distance)));
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
