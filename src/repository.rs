//! SQLite is accessed exclusively by the engine worker. Result queries never read BLOBs.
use crate::{model::*, search};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
#[cfg(test)]
thread_local! {
    pub(crate) static FULL_BODY_READS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    pub(crate) static PREVIEW_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn count_body(text: &str) {
    FULL_BODY_READS.with(|n| {
        let (rows, bytes) = n.get();
        n.set((rows + 1, bytes + text.len()));
    });
}
#[derive(Default)]
struct WalProgress {
    frames: AtomicUsize,
    checkpointed: AtomicUsize,
}
unsafe extern "C" fn observe_wal(
    context: *mut std::ffi::c_void,
    _database: *mut rusqlite::ffi::sqlite3,
    _name: *const std::ffi::c_char,
    frames: std::ffi::c_int,
) -> std::ffi::c_int {
    // SQLite invokes this on the connection's writer thread. The context is a
    // stable Box owned by Repository, alive until after Connection is dropped.
    let progress = unsafe { &*context.cast::<WalProgress>() };
    let frames = frames.max(0) as usize;
    let previous = progress.frames.swap(frames, Ordering::Relaxed);
    if frames <= previous {
        progress.checkpointed.store(0, Ordering::Relaxed);
    }
    rusqlite::ffi::SQLITE_OK
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mutation {
    Content,
    Metadata,
    Eligibility,
    SearchMetadata,
}
pub(crate) struct PendingItem {
    pub item: StoredItem,
    pub mutation: Mutation,
}
pub(crate) struct CommitEffects {
    pub sources_removed: bool,
}
pub(crate) struct Repository {
    pub(crate) harness: crate::harness_store::HarnessStore,
    pub(crate) organizer: crate::organizer_store::OrganizerStore,
    pub(crate) paths: crate::path_activity_store::PathActivityStore,
    pub(crate) connection: Connection,
    wal_path: std::path::PathBuf,
    page_size: u64,
    // Field drop order keeps this Box alive until after Connection closes.
    wal_progress: Box<WalProgress>,
    retention_policy: Option<(u32, usize)>,
    maintenance_due_ms: i64,
    repair_vectors: bool,
    observed_rowid: i64,
}

fn period_predicate(
    from_ms: Option<i64>,
    until_ms: Option<i64>,
    first_parameter: usize,
) -> (String, Vec<i64>) {
    if from_ms.is_none() && until_ms.is_none() {
        return (String::new(), Vec::new());
    }
    let mut predicate = String::new();
    let mut bounds = Vec::with_capacity(2);
    if let Some(from_ms) = from_ms {
        predicate.push_str(&format!(" AND created_at_ms>=?{first_parameter}"));
        bounds.push(from_ms);
    }
    if let Some(until_ms) = until_ms {
        let parameter = first_parameter + bounds.len();
        predicate.push_str(&format!(" AND created_at_ms<?{parameter}"));
        bounds.push(until_ms);
    }
    (predicate, bounds)
}

impl Repository {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        crate::semantic_store::register()?;
        let mut connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(|e| e.to_string())?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY; PRAGMA wal_autocheckpoint=0; PRAGMA journal_size_limit=16777216;
            CREATE TABLE IF NOT EXISTS items (
                id TEXT PRIMARY KEY, kind INTEGER NOT NULL, title TEXT NOT NULL, pinned INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL, last_used_ms INTEGER NOT NULL, use_count INTEGER NOT NULL,
                summary TEXT NOT NULL, text TEXT NOT NULL, target TEXT, open_with TEXT
            );
            CREATE INDEX IF NOT EXISTS items_recent ON items(pinned DESC, last_used_ms DESC);
            CREATE TABLE IF NOT EXISTS representations (item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE, format TEXT NOT NULL, data BLOB NOT NULL, PRIMARY KEY(item_id, format));
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE VIRTUAL TABLE IF NOT EXISTS item_search USING fts5(content);
            PRAGMA foreign_keys=ON;").map_err(|e| e.to_string())?;
        connection.execute_batch("CREATE INDEX IF NOT EXISTS items_alias_search ON items(lower(json_extract(summary,'$.alias')));
            CREATE INDEX IF NOT EXISTS items_title_search ON items(lower(title));
            CREATE INDEX IF NOT EXISTS items_active ON items(last_used_ms DESC);
            CREATE INDEX IF NOT EXISTS items_kind_active ON items(kind,last_used_ms DESC);
            CREATE INDEX IF NOT EXISTS items_pinned_active ON items(last_used_ms DESC) WHERE pinned=1;
            CREATE INDEX IF NOT EXISTS items_favorite_active ON items(last_used_ms DESC) WHERE kind<>0;
            CREATE INDEX IF NOT EXISTS items_used_active ON items(last_used_ms DESC) WHERE use_count>0;").map_err(|e|e.to_string())?;
        let version: Option<String> = connection
            .query_row(
                "SELECT value FROM settings WHERE key='index_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if version.as_deref() != Some(search::INDEX_VERSION.to_string().as_str()) {
            let tx = connection.transaction().map_err(|e| e.to_string())?;
            tx.execute_batch("DROP TABLE item_search; CREATE VIRTUAL TABLE item_search USING fts5(content,heading,phonetic,phonetic_prefix)")
                .map_err(|e| e.to_string())?;
            {
                let mut statement = tx
                    .prepare("SELECT rowid,summary,text,target,open_with FROM items")
                    .map_err(|e| e.to_string())?;
                let mut rows = statement.query([]).map_err(|e| e.to_string())?;
                while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                    let rowid: i64 = row.get(0).map_err(|e| e.to_string())?;
                    let item = StoredItem {
                        summary: serde_json::from_str(
                            &row.get::<_, String>(1).map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?,
                        text: row.get(2).map_err(|e| e.to_string())?,
                        representations: vec![],
                        target: row.get(3).map_err(|e| e.to_string())?,
                        open_with: row.get(4).map_err(|e| e.to_string())?,
                    };
                    tx.execute(
                        "INSERT INTO item_search(rowid,content,heading,phonetic,phonetic_prefix) VALUES(?1,?2,?3,?4,?5)",
                        params![rowid, search::index_text(&item), search::index_heading(&item.summary, false), search::index_heading(&item.summary, true), search::index_phonetic_prefix(&item.summary)],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            tx.execute("INSERT INTO settings(key,value) VALUES('index_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[search::INDEX_VERSION.to_string()]).map_err(|e|e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        let mut wal_path = path.as_os_str().to_os_string();
        wal_path.push("-wal");
        let page_size = connection
            .query_row("PRAGMA page_size", [], |row| {
                row.get::<_, u32>(0).map(u64::from)
            })
            .map_err(|e| e.to_string())?;
        let wal_progress = Box::<WalProgress>::default();
        // Install after wal_autocheckpoint=0, which itself replaces SQLite's WAL hook.
        // Callback only observes frame counts; it never enters SQLite recursively.
        unsafe {
            rusqlite::ffi::sqlite3_wal_hook(
                connection.handle(),
                Some(observe_wal),
                (&*wal_progress as *const WalProgress).cast_mut().cast(),
            );
        }
        let observed_rowid = connection
            .query_row("SELECT coalesce(max(rowid),0) FROM items", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let paths = crate::path_activity_store::PathActivityStore::load(&connection)?;
        let organizer = crate::organizer_store::OrganizerStore::load(&connection)?;
        let harness = crate::harness_store::HarnessStore::load(&connection);
        let repo = Self {
            harness,
            organizer,
            paths,
            connection,
            wal_path: wal_path.into(),
            page_size,
            wal_progress,
            retention_policy: None,
            maintenance_due_ms: 0,
            repair_vectors: true,
            observed_rowid,
        };
        if repo.wal_bytes() >= 64 * 1024 * 1024 {
            repo.checkpoint();
        }
        Ok(repo)
    }
    pub fn settings(&self) -> Result<Settings, String> {
        let json: Option<String> = self
            .connection
            .query_row("SELECT value FROM settings WHERE key='settings'", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        json.map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .unwrap_or_else(|| Ok(Settings::default()))
    }
    pub fn metadata(&self, id: &str) -> Result<Option<StoredItem>, String> {
        let raw: Option<(String, Option<String>, Option<String>)> = self
            .connection
            .query_row(
                "SELECT summary,target,open_with FROM items WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        raw.map(|(summary, target, open_with)| {
            Ok(StoredItem {
                summary: serde_json::from_str(&summary).map_err(|e| e.to_string())?,
                text: String::new(),
                representations: vec![],
                target,
                open_with,
            })
        })
        .transpose()
    }
    pub(crate) fn body_preview(&self, id: &str) -> Result<String, String> {
        self.connection
            .prepare_cached("SELECT substr(text,1,152) FROM items WHERE id=?1")
            .map_err(|e| e.to_string())?
            .query_row([id], |r| r.get(0))
            .optional()
            .map(|v: Option<String>| {
                let text = v.unwrap_or_default();
                #[cfg(test)]
                PREVIEW_BYTES.with(|n| n.set(n.get() + text.len()));
                text
            })
            .map_err(|e| e.to_string())
    }
    pub fn organizer_context(&self, id: &str) -> Result<String, String> {
        self.connection
            .query_row(
                "SELECT substr(text,1,1024) FROM items WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map(|v| v.unwrap_or_default())
            .map_err(|e| e.to_string())
    }
    pub fn load(&self, id: &str, blobs: bool) -> Result<Option<StoredItem>, String> {
        let raw: Option<(String, String, Option<String>, Option<String>)> = self
            .connection
            .query_row(
                "SELECT summary,text,target,open_with FROM items WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((summary, text, target, open_with)) = raw else {
            return Ok(None);
        };
        #[cfg(test)]
        count_body(&text);
        let mut item = StoredItem {
            summary: serde_json::from_str(&summary).map_err(|e| e.to_string())?,
            text,
            representations: vec![],
            target,
            open_with,
        };
        if blobs {
            let mut stmt = self
                .connection
                .prepare("SELECT format,data FROM representations WHERE item_id=?1 ORDER BY format")
                .map_err(|e| e.to_string())?;
            item.representations = stmt
                .query_map([id], |r| {
                    Ok(Representation {
                        format: r.get(0)?,
                        data: r.get(1)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
        }
        Ok(Some(item))
    }
    fn candidate_ids(
        &self,
        sql: &str,
        arguments: &[&dyn rusqlite::ToSql],
        ids: &mut HashSet<i64>,
    ) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare_cached(sql)
            .map_err(|e| e.to_string())?;
        let mut rows = statement.query(arguments).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            ids.insert(row.get(0).map_err(|e| e.to_string())?);
        }
        Ok(())
    }
    fn candidate_ids_with_bounds(
        &self,
        sql: &str,
        arguments: &[&dyn rusqlite::ToSql],
        bounds: &[i64],
        ids: &mut HashSet<i64>,
    ) -> Result<(), String> {
        if bounds.is_empty() {
            return self.candidate_ids(sql, arguments, ids);
        }
        let mut combined = Vec::with_capacity(arguments.len() + bounds.len());
        combined.extend_from_slice(arguments);
        combined.extend(bounds.iter().map(|value| value as &dyn rusqlite::ToSql));
        self.candidate_ids(sql, &combined, ids)
    }
    fn auxiliary_fts_ids(
        &self,
        expression: &str,
        mut ids: Vec<i64>,
    ) -> Result<HashSet<i64>, String> {
        ids.sort_unstable();
        let mut matches = HashSet::new();
        let mut start = 0;
        while start < ids.len() {
            let mut end = start + 1;
            while end < ids.len()
                && end - start < 128
                && ids[end].saturating_sub(ids[start]) <= 2048
            {
                end += 1;
            }
            let chunk = &ids[start..end];
            let placeholders = (4..4 + chunk.len())
                .map(|number| format!("?{number}"))
                .collect::<Vec<_>>()
                .join(",");
            // Unary + keeps the IN list out of FTS's rowid-equality strategy,
            // otherwise SQLite invokes MATCH once for every auxiliary rowid.
            // A bounded range also prevents an old pinned row from making this
            // scan the whole posting list just to check a nearby recent group.
            let sql = format!(
                "SELECT rowid FROM item_search WHERE item_search MATCH ?1 AND rowid>=?2 AND rowid<=?3 AND +rowid IN ({placeholders})"
            );
            let mut args = vec![
                rusqlite::types::Value::Text(expression.into()),
                rusqlite::types::Value::Integer(chunk[0]),
                rusqlite::types::Value::Integer(*chunk.last().unwrap()),
            ];
            args.extend(chunk.iter().copied().map(rusqlite::types::Value::Integer));
            let mut statement = self
                .connection
                .prepare_cached(&sql)
                .map_err(|e| e.to_string())?;
            let mut rows = statement
                .query(rusqlite::params_from_iter(args))
                .map_err(|e| e.to_string())?;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                matches.insert(row.get(0).map_err(|e| e.to_string())?);
            }
            start = end;
        }
        Ok(matches)
    }
    fn candidate_metadata(&self, rowid: i64) -> Result<StoredItem, String> {
        let (summary, target, open_with): (String, Option<String>, Option<String>) = self
            .connection
            .prepare_cached("SELECT summary,target,open_with FROM items WHERE rowid=?1")
            .map_err(|e| e.to_string())?
            .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        Ok(StoredItem {
            summary: serde_json::from_str(&summary).map_err(|e| e.to_string())?,
            text: String::new(),
            representations: vec![],
            target,
            open_with,
        })
    }
    /// Time predicates precede every candidate quota. `until_ms` is exclusive.
    /// Omitting both bounds leaves the ordinary-search SQL unchanged.
    pub fn summaries_in_period(
        &self,
        query: &SearchQuery,
        from_ms: Option<i64>,
        until_ms: Option<i64>,
        excerpts: &mut HashMap<String, String>,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Vec<ItemSummary>, String> {
        let needle = search::normalize(query.text.trim());
        let expression = search::query_expression(&query.text);
        let nonempty = !query.text.trim().is_empty();
        let scope = match query.scope {
            Scope::All => "1",
            Scope::Clipboard => "kind=0",
            Scope::Folder => "kind=1",
            Scope::Document => "kind=2",
        };
        let mut ids = HashSet::new();
        let mut fts_ids = HashSet::new();
        let mut confirmed = HashMap::new();
        if let Some(expression) = expression.as_ref() {
            // Keyset paging counts literal matches, never raw gram collisions.
            // Heading streams have independent quotas so newer body matches cannot
            // crowd old literal/phonetic title matches out of relevance ranking.
            let channels = [
                (format!("heading: ({expression})"), 600_000),
                (
                    format!(
                        "phonetic_prefix: {} AND phonetic: ({expression})",
                        search::phonetic_prefix_expression(&query.text)
                    ),
                    500_000,
                ),
                (format!("phonetic: ({expression})"), 400_000),
                (format!("content: ({expression})"), 0),
            ];
            for (qualified, minimum_rank) in channels {
                let mut cursor = i64::MAX;
                let mut accepted = 0;
                loop {
                    checkpoint()?;
                    let (period, bounds) = period_predicate(from_ms, until_ms, 3);
                    let sql = format!(
                        "SELECT item_search.rowid FROM item_search CROSS JOIN items ON items.rowid=item_search.rowid WHERE item_search MATCH ?1 AND item_search.rowid<?2 AND {scope}{period} ORDER BY item_search.rowid DESC LIMIT 128"
                    );
                    let mut page = HashSet::new();
                    self.candidate_ids_with_bounds(
                        &sql,
                        &[&qualified, &cursor],
                        &bounds,
                        &mut page,
                    )?;
                    if page.is_empty() {
                        break;
                    }
                    cursor = *page.iter().min().unwrap();
                    let exhausted = page.len() < 128;
                    for rowid in page {
                        checkpoint()?;
                        let mut item = self.candidate_metadata(rowid)?;
                        if minimum_rank > 0
                            && search::rank_normalized(&item.summary, &needle) < minimum_rank
                        {
                            continue;
                        }
                        let metadata_match = search::matches_normalized(&item, &needle);
                        let excerpt = if !metadata_match {
                            if minimum_rank > 0 {
                                continue;
                            }
                            item.text = self
                                .connection
                                .prepare_cached("SELECT text FROM items WHERE rowid=?1")
                                .map_err(|e| e.to_string())?
                                .query_row([rowid], |r| r.get(0))
                                .map_err(|e| e.to_string())?;
                            #[cfg(test)]
                            count_body(&item.text);
                            search::body_excerpt(&item.text, &needle)
                        } else {
                            None
                        };
                        if metadata_match || excerpt.is_some() {
                            accepted += 1;
                            fts_ids.insert(rowid);
                            if let Some(excerpt) = excerpt {
                                excerpts.insert(item.summary.id.clone(), excerpt);
                            }
                            confirmed.insert(rowid, item.summary);
                        }
                    }
                    if exhausted || accepted >= 50 {
                        break;
                    }
                }
            }
        } else {
            let (period, bounds) = period_predicate(from_ms, until_ms, 1);
            self.candidate_ids_with_bounds(
                &format!(
                    "SELECT rowid FROM items WHERE {scope}{period} ORDER BY last_used_ms DESC LIMIT 512"
                ),
                &[],
                &bounds,
                &mut ids,
            )?;
        }
        if nonempty {
            let needle = query.text.trim().to_lowercase();
            let ceiling = format!("{needle}\u{10ffff}");
            for field in ["lower(json_extract(summary,'$.alias'))", "lower(title)"] {
                let (period, bounds) = period_predicate(from_ms, until_ms, 2);
                self.candidate_ids_with_bounds(
                    &format!(
                        "SELECT rowid FROM items WHERE {scope} AND {field}=?1{period} LIMIT 128"
                    ),
                    &[&needle],
                    &bounds,
                    &mut ids,
                )?;
                let (period, bounds) = period_predicate(from_ms, until_ms, 3);
                self.candidate_ids_with_bounds(&format!("SELECT rowid FROM items WHERE {scope} AND {field}>=?1 AND {field}<?2{period} LIMIT 128"),&[&needle,&ceiling],&bounds,&mut ids)?;
            }
        }
        // Indexed auxiliary pools preserve old pinned/favorite/recently-used entries
        // that fall outside the newest FTS rows. Recent captures have their own pool
        // too: deduplicated captures update recency without changing rowid.
        // Each category has its own quota.
        for predicate in ["pinned=1", "kind<>0", "use_count>0", "1"] {
            let (period, bounds) = period_predicate(from_ms, until_ms, 1);
            self.candidate_ids_with_bounds(&format!("SELECT rowid FROM items WHERE {scope} AND {predicate}{period} ORDER BY last_used_ms DESC LIMIT 128"),&[],&bounds,&mut ids)?;
        }
        let mut summaries: Vec<_> = confirmed.values().cloned().collect();
        let mut body_candidates = Vec::new();
        for rowid in ids {
            checkpoint()?;
            if confirmed.contains_key(&rowid) {
                continue;
            }
            let raw: Option<(String, Option<String>, Option<String>)> = self
                .connection
                .prepare_cached("SELECT summary,target,open_with FROM items WHERE rowid=?1")
                .map_err(|e| e.to_string())?
                .query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()
                .map_err(|e| e.to_string())?;
            let Some((summary, target, open_with)) = raw else {
                continue;
            };
            let item = StoredItem {
                summary: serde_json::from_str(&summary).map_err(|e| e.to_string())?,
                text: String::new(),
                representations: vec![],
                target,
                open_with,
            };
            if !matches!(
                (item.summary.kind, query.scope),
                (_, Scope::All)
                    | (ItemKind::Clipboard, Scope::Clipboard)
                    | (ItemKind::Folder, Scope::Folder)
                    | (ItemKind::Document, Scope::Document)
            ) {
                continue;
            }
            if !nonempty || search::matches_normalized(&item, &needle) {
                summaries.push(item.summary);
                continue;
            }
            body_candidates.push((rowid, item));
        }
        let auxiliary = if let Some(expression) = expression.as_ref() {
            self.auxiliary_fts_ids(
                expression,
                body_candidates
                    .iter()
                    .filter(|(id, _)| !fts_ids.contains(id))
                    .map(|(id, _)| *id)
                    .collect(),
            )?
        } else {
            HashSet::new()
        };
        for (rowid, mut item) in body_candidates {
            checkpoint()?;
            if expression.is_some() && !fts_ids.contains(&rowid) && !auxiliary.contains(&rowid) {
                continue;
            }
            let text: Option<String> = self
                .connection
                .prepare_cached("SELECT text FROM items WHERE rowid=?1")
                .map_err(|e| e.to_string())?
                .query_row([rowid], |row| row.get(0))
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(text) = text {
                // Metadata was already loaded above. Avoid another statement prepare,
                // a string-ID lookup and duplicate summary JSON parsing for FTS collisions.
                #[cfg(test)]
                count_body(&text);
                item.text = text;
                if let Some(excerpt) = search::body_excerpt(&item.text, &needle) {
                    excerpts.insert(item.summary.id.clone(), excerpt);
                    summaries.push(item.summary);
                }
            }
        }
        Ok(summaries)
    }
    /// A separate metadata-only lane keeps literal paths out of lexical quotas.
    /// The actor filters overlays/deletions and retains at most 50 matches per page.
    pub fn path_page(&self, scope: Scope, cursor: i64) -> Result<Vec<(i64, StoredItem)>, String> {
        let scope = match scope {
            Scope::All => "1",
            Scope::Clipboard => "kind=0",
            Scope::Folder => "kind=1",
            Scope::Document => "kind=2",
        };
        let sql = format!(
            "SELECT rowid,summary,target FROM items WHERE {scope} AND target IS NOT NULL AND rowid<?1 ORDER BY rowid DESC LIMIT 128"
        );
        let mut statement = self
            .connection
            .prepare_cached(&sql)
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([cursor], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.map(|row| {
            let (rowid, summary, target) = row.map_err(|e| e.to_string())?;
            Ok((
                rowid,
                StoredItem {
                    summary: serde_json::from_str(&summary).map_err(|e| e.to_string())?,
                    target,
                    text: String::new(),
                    representations: vec![],
                    open_with: None,
                },
            ))
        })
        .collect()
    }
    pub fn commit(
        &mut self,
        pending: &HashMap<String, PendingItem>,
        deleted: &HashSet<String>,
        settings: Option<&Settings>,
        retention: &Settings,
        now_ms: i64,
        vectors: &crate::semantic_store::PendingVectors,
    ) -> Result<CommitEffects, String> {
        // A real dirty commit is the only persistence maintenance boundary.
        // Cache eviction remains pending if this transaction fails.
        self.paths.expire(now_ms);
        let policy = (retention.retention_days, retention.max_history);
        let policy_changed = self.retention_policy != Some(policy);
        let tx = self.connection.transaction().map_err(|e| e.to_string())?;
        let disk_rowid: i64 = tx
            .query_row("SELECT coalesce(max(rowid),0) FROM items", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut added_history = disk_rowid > self.observed_rowid;
        for (id, p) in pending {
            if p.mutation == Mutation::Content
                && p.item.summary.kind == ItemKind::Clipboard
                && !tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM items WHERE id=?1)",
                        [id],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(|e| e.to_string())?
            {
                added_history = true;
            }
        }
        let maintain = policy_changed || added_history || now_ms >= self.maintenance_due_ms;
        let mut removed = deleted.clone();
        for (id, change) in pending {
            let mut indexed_item;
            let item = if change.mutation == Mutation::SearchMetadata {
                indexed_item = change.item.clone();
                indexed_item.text = tx
                    .query_row("SELECT text FROM items WHERE id=?1", [id], |r| r.get(0))
                    .map_err(|e| e.to_string())?;
                &indexed_item
            } else {
                &change.item
            };
            let s = &item.summary;
            let summary = serde_json::to_string(s).map_err(|e| e.to_string())?;
            if change.mutation != Mutation::Content {
                tx.execute("UPDATE items SET pinned=?2,last_used_ms=?3,use_count=?4,summary=?5,title=?6 WHERE id=?1",params![id,s.pinned,s.last_used_ms,i64::try_from(s.use_count).unwrap_or(i64::MAX),summary,s.title]).map_err(|e|e.to_string())?;
                if matches!(change.mutation, Mutation::Metadata | Mutation::Eligibility) {
                    continue;
                }
            } else {
                let kind = match s.kind {
                    ItemKind::Clipboard => 0,
                    ItemKind::Folder => 1,
                    ItemKind::Document => 2,
                };
                tx.execute("INSERT INTO items(id,kind,title,pinned,created_at_ms,last_used_ms,use_count,summary,text,target,open_with) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,title=excluded.title,pinned=excluded.pinned,created_at_ms=excluded.created_at_ms,last_used_ms=excluded.last_used_ms,use_count=excluded.use_count,summary=excluded.summary,text=excluded.text,target=excluded.target,open_with=excluded.open_with", params![id,kind,s.title,s.pinned,s.created_at_ms,s.last_used_ms,i64::try_from(s.use_count).unwrap_or(i64::MAX),summary,item.text,item.target,item.open_with]).map_err(|e| e.to_string())?;
                if change.mutation == Mutation::Content {
                    tx.execute("DELETE FROM representations WHERE item_id=?1", [id])
                        .map_err(|e| e.to_string())?;
                    for repr in &item.representations {
                        tx.execute(
                            "INSERT INTO representations(item_id,format,data) VALUES(?1,?2,?3)",
                            params![id, repr.format, repr.data],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
            }
            tx.execute(
                "DELETE FROM item_search WHERE rowid=(SELECT rowid FROM items WHERE id=?1)",
                [id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO item_search(rowid,content,heading,phonetic,phonetic_prefix) SELECT rowid,?2,?3,?4,?5 FROM items WHERE id=?1",
                params![id, search::index_text(item), search::index_heading(&item.summary, false), search::index_heading(&item.summary, true), search::index_phonetic_prefix(&item.summary)],
            )
            .map_err(|e| e.to_string())?;
        }
        if let Some(settings) = settings {
            tx.execute("INSERT INTO settings(key,value) VALUES('settings',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(settings).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        }
        if maintain {
            let cutoff = now_ms.saturating_sub(i64::from(retention.retention_days) * 86_400_000);
            let max_history = i64::try_from(retention.max_history).unwrap_or(i64::MAX);
            let mut stmt = tx.prepare("SELECT id FROM items WHERE kind=0 AND pinned=0 AND last_used_ms<?1 UNION SELECT id FROM (SELECT id FROM items WHERE kind=0 AND pinned=0 ORDER BY last_used_ms DESC,id LIMIT -1 OFFSET ?2)").map_err(|e|e.to_string())?;
            let ids = stmt
                .query_map(params![cutoff, max_history], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for id in ids {
                removed.insert(id.map_err(|e| e.to_string())?);
            }
        }
        for id in &removed {
            tx.execute(
                "DELETE FROM item_search WHERE rowid=(SELECT rowid FROM items WHERE id=?1)",
                [id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM items WHERE id=?1", [id])
                .map_err(|e| e.to_string())?;
        }
        crate::semantic_store::commit(&tx, pending, vectors, &removed, self.repair_vectors)?;
        for id in &removed {
            self.organizer
                .forget(&crate::organizer::Source::SavedItem { id: id.clone() });
        }
        self.paths.write(&tx)?;
        self.organizer.write(&tx)?;
        self.harness.write(&tx)?;
        let observed_rowid = tx
            .query_row("SELECT coalesce(max(rowid),0) FROM items", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        self.paths.committed();
        self.organizer.committed();
        self.harness.committed();
        self.observed_rowid = observed_rowid;
        self.retention_policy = Some(policy);
        if maintain {
            self.maintenance_due_ms = now_ms.saturating_add(3_600_000);
        }
        self.repair_vectors = false;
        Ok(CommitEffects {
            sources_removed: !removed.is_empty(),
        })
    }
    pub fn wal_bytes(&self) -> u64 {
        std::fs::metadata(&self.wal_path).map_or(0, |metadata| metadata.len())
    }
    pub fn wal_backlog_bytes(&self) -> u64 {
        let frames = self.wal_progress.frames.load(Ordering::Relaxed);
        let completed = self.wal_progress.checkpointed.load(Ordering::Relaxed);
        (frames.saturating_sub(completed) as u64).saturating_mul(self.page_size + 24)
    }
    pub fn checkpoint(&self) -> bool {
        match self
            .connection
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            }) {
            Ok((busy, frames, completed)) if frames >= 0 && completed >= 0 => {
                self.wal_progress
                    .frames
                    .store(frames as usize, Ordering::Relaxed);
                self.wal_progress
                    .checkpointed
                    .store(completed as usize, Ordering::Relaxed);
                if busy != 0 || completed != frames {
                    return false;
                }
                // PASSIVE makes frames reusable, but the physical file can remain
                // oversized after one large FTS merge. Reclaim it at the emergency
                // threshold only. SQLite's bounded busy timeout protects readers;
                // failure leaves progress intact and the actor schedules a retry.
                if self.wal_bytes() >= 64 * 1024 * 1024 {
                    let truncated =
                        self.connection
                            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                                row.get::<_, i64>(0)
                            })
                            == Ok(0);
                    if !truncated {
                        return false;
                    }
                    self.wal_progress.frames.store(0, Ordering::Relaxed);
                    self.wal_progress.checkpointed.store(0, Ordering::Relaxed);
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    #[test]
    fn collision_pages_can_yield_release_reads_and_resume_complete_recall() {
        let mut repo = Repository::open(Path::new(":memory:")).unwrap();
        for i in 0..400 {
            let summary = ItemSummary {
                id: format!("page:{i}"),
                kind: ItemKind::Clipboard,
                title: "标题".into(),
                subtitle: "来源".into(),
                alias: String::new(),
                tags: vec![],
                pinned: false,
                created_at_ms: 1,
                last_used_ms: 1,
                use_count: 0,
            };
            repo.connection
                .execute(
                    "INSERT INTO items VALUES(?1,0,?2,0,1,1,0,?3,?4,NULL,NULL)",
                    params![
                        summary.id,
                        summary.title,
                        serde_json::to_string(&summary).unwrap(),
                        if i == 0 { "abcde" } else { "abc bcd cde" }
                    ],
                )
                .unwrap();
            repo.connection
                .execute(
                    "INSERT INTO item_search(rowid,content) VALUES(?1,'abc bcd cde')",
                    [i + 1],
                )
                .unwrap();
        }
        let query = SearchQuery {
            text: "abcde".into(),
            ..Default::default()
        };
        let mut pages = 0;
        let interrupted = repo.summaries_in_period(&query, None, None, &mut HashMap::new(), || {
            pages += 1;
            if pages == 300 {
                Err("due save".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(interrupted.unwrap_err(), "due save");
        // The aborted reader must release its statement/transaction for due writes.
        let settings = Settings {
            retention_days: u32::MAX,
            ..Default::default()
        };
        repo.commit(
            &HashMap::new(),
            &HashSet::new(),
            Some(&settings),
            &settings,
            1,
            &crate::semantic_store::PendingVectors::default(),
        )
        .unwrap();
        let complete = repo
            .summaries_in_period(&query, None, None, &mut HashMap::new(), || Ok(()))
            .unwrap();
        assert_eq!(complete.len(), 1);
        assert_eq!(complete[0].id, "page:0");
    }
}
