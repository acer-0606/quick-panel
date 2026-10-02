//! SQLite is accessed exclusively by the engine worker. Result queries never read BLOBs.
use crate::{model::*, search};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
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
}
pub(crate) struct PendingItem {
    pub item: StoredItem,
    pub mutation: Mutation,
}
pub(crate) struct Repository {
    connection: Connection,
    wal_path: std::path::PathBuf,
    page_size: u64,
    // Field drop order keeps this Box alive until after Connection closes.
    wal_progress: Box<WalProgress>,
}

impl Repository {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
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
            tx.execute("DELETE FROM item_search", [])
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
                        "INSERT INTO item_search(rowid,content) VALUES(?1,?2)",
                        params![rowid, search::index_text(&item)],
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
        let repo = Self {
            connection,
            wal_path: wal_path.into(),
            page_size,
            wal_progress,
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
    pub fn summaries(&self, query: &SearchQuery) -> Result<Vec<ItemSummary>, String> {
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
        if let Some(expression) = expression.as_ref() {
            // FTS supplies rowid order itself. Applying metadata relevance before LIMIT
            // would sort every hit; retrieve a bounded recent pool before table lookup.
            let fts_sql = if query.scope == Scope::All {
                "SELECT rowid FROM item_search WHERE item_search MATCH ?1 ORDER BY rowid DESC LIMIT 512".to_string()
            } else {
                format!(
                    "SELECT item_search.rowid FROM item_search CROSS JOIN items ON items.rowid=item_search.rowid WHERE item_search MATCH ?1 AND {scope} ORDER BY item_search.rowid DESC LIMIT 512"
                )
            };
            self.candidate_ids(&fts_sql, &[expression], &mut fts_ids)?;
            ids.extend(fts_ids.iter().copied());
        } else {
            self.candidate_ids(
                &format!(
                    "SELECT rowid FROM items WHERE {scope} ORDER BY last_used_ms DESC LIMIT 512"
                ),
                &[],
                &mut ids,
            )?;
        }
        if nonempty {
            let needle = query.text.trim().to_lowercase();
            let ceiling = format!("{needle}\u{10ffff}");
            for field in ["lower(json_extract(summary,'$.alias'))", "lower(title)"] {
                self.candidate_ids(
                    &format!("SELECT rowid FROM items WHERE {scope} AND {field}=?1 LIMIT 128"),
                    &[&needle],
                    &mut ids,
                )?;
                self.candidate_ids(&format!("SELECT rowid FROM items WHERE {scope} AND {field}>=?1 AND {field}<?2 LIMIT 128"),&[&needle,&ceiling],&mut ids)?;
            }
        }
        // Indexed auxiliary pools preserve old pinned/favorite/recently-used entries
        // that fall outside the newest FTS rows. Recent captures have their own pool
        // too: deduplicated captures update recency without changing rowid.
        // Each category has its own quota.
        for predicate in ["pinned=1", "kind<>0", "use_count>0", "1"] {
            self.candidate_ids(&format!("SELECT rowid FROM items WHERE {scope} AND {predicate} ORDER BY last_used_ms DESC LIMIT 128"),&[],&mut ids)?;
        }
        let mut summaries = Vec::new();
        let mut body_candidates = Vec::new();
        for rowid in ids {
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
            if !nonempty || search::matches(&item, &query.text) {
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
                item.text = text;
                if search::matches(&item, &query.text) {
                    summaries.push(item.summary);
                }
            }
        }
        Ok(summaries)
    }
    pub fn commit(
        &mut self,
        pending: &HashMap<String, PendingItem>,
        deleted: &HashSet<String>,
        settings: Option<&Settings>,
        retention: &Settings,
        now_ms: i64,
    ) -> Result<(), String> {
        let tx = self.connection.transaction().map_err(|e| e.to_string())?;
        for id in deleted {
            tx.execute(
                "DELETE FROM item_search WHERE rowid=(SELECT rowid FROM items WHERE id=?1)",
                [id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("DELETE FROM items WHERE id=?1", [id])
                .map_err(|e| e.to_string())?;
        }
        for (id, change) in pending {
            let item = &change.item;
            let s = &item.summary;
            let summary = serde_json::to_string(s).map_err(|e| e.to_string())?;
            if change.mutation == Mutation::Metadata {
                tx.execute("UPDATE items SET pinned=?2,last_used_ms=?3,use_count=?4,summary=?5 WHERE id=?1",params![id,s.pinned,s.last_used_ms,i64::try_from(s.use_count).unwrap_or(i64::MAX),summary]).map_err(|e|e.to_string())?;
                continue;
            }
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
            tx.execute(
                "DELETE FROM item_search WHERE rowid=(SELECT rowid FROM items WHERE id=?1)",
                [id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO item_search(rowid,content) SELECT rowid,?2 FROM items WHERE id=?1",
                params![id, search::index_text(item)],
            )
            .map_err(|e| e.to_string())?;
        }
        if let Some(settings) = settings {
            tx.execute("INSERT INTO settings(key,value) VALUES('settings',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(settings).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        }
        // Retention runs in a real dirty batch, never from an idle timer. Pinned history and favorites survive.
        let cutoff = now_ms.saturating_sub(i64::from(retention.retention_days) * 86_400_000);
        tx.execute("DELETE FROM item_search WHERE rowid IN (SELECT rowid FROM items WHERE kind=0 AND pinned=0 AND last_used_ms<?1)", [cutoff]).map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM items WHERE kind=0 AND pinned=0 AND last_used_ms<?1",
            [cutoff],
        )
        .map_err(|e| e.to_string())?;
        let max_history = i64::try_from(retention.max_history).unwrap_or(i64::MAX);
        tx.execute("DELETE FROM item_search WHERE rowid IN (SELECT rowid FROM items WHERE kind=0 AND pinned=0 ORDER BY last_used_ms DESC,id LIMIT -1 OFFSET ?1)", [max_history]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM items WHERE kind=0 AND pinned=0 AND rowid IN (SELECT rowid FROM items WHERE kind=0 AND pinned=0 ORDER BY last_used_ms DESC,id LIMIT -1 OFFSET ?1)", [max_history]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
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
                busy == 0 && completed == frames
            }
            _ => false,
        }
    }
}
