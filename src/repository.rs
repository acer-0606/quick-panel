//! SQLite is accessed exclusively by the engine worker. Result queries never read BLOBs.
use crate::{model::*, search};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::Duration,
};

pub(crate) struct PendingItem {
    pub item: StoredItem,
    pub replace_representations: bool,
}
pub(crate) struct Repository {
    connection: Connection,
    wal_path: std::path::PathBuf,
}
const SUMMARY_COLUMNS: &str = "i.summary";

impl Repository {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
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
        let mut wal_path = path.as_os_str().to_os_string();
        wal_path.push("-wal");
        Ok(Self {
            connection,
            wal_path: wal_path.into(),
        })
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
    pub fn summaries(&self, query: &SearchQuery) -> Result<Vec<ItemSummary>, String> {
        let scope = match query.scope {
            Scope::All => -1,
            Scope::Clipboard => 0,
            Scope::Folder => 1,
            Scope::Document => 2,
        };
        let expression = search::query_expression(&query.text);
        let nonempty = !query.text.trim().is_empty();
        let columns = if nonempty {
            "i.summary,i.text,i.target,i.open_with"
        } else {
            SUMMARY_COLUMNS
        };
        // Relevance must influence recall before LIMIT: recent broad-prefix matches
        // must not evict an older exact alias or title from the candidate set.
        let order = "CASE WHEN ?3='' THEN 0 WHEN lower(json_extract(i.summary,'$.alias'))=lower(?3) THEN 0 WHEN lower(i.title)=lower(?3) THEN 1 WHEN substr(lower(json_extract(i.summary,'$.alias')),1,length(?3))=lower(?3) THEN 2 WHEN substr(lower(i.title),1,length(?3))=lower(?3) THEN 3 ELSE 4 END, i.pinned DESC, (i.kind<>0) DESC, i.last_used_ms DESC";
        let sql = if expression.is_some() {
            format!(
                "SELECT {columns} FROM item_search JOIN items i ON i.rowid=item_search.rowid WHERE item_search MATCH ?1 AND (?2=-1 OR i.kind=?2) ORDER BY {order} LIMIT 500"
            )
        } else {
            format!(
                "SELECT {columns} FROM items i WHERE (?2=-1 OR i.kind=?2) ORDER BY {order} LIMIT 500"
            )
        };
        let mut stmt = self.connection.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![
                expression.unwrap_or_default(),
                scope,
                query.text.trim()
            ])
            .map_err(|e| e.to_string())?;
        let mut summaries = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let summary: ItemSummary =
                serde_json::from_str(&row.get::<_, String>(0).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if nonempty {
                // Inspect one candidate's text at a time, never its representations or all database payloads.
                let item = StoredItem {
                    summary,
                    text: row.get(1).map_err(|e| e.to_string())?,
                    representations: vec![],
                    target: row.get(2).map_err(|e| e.to_string())?,
                    open_with: row.get(3).map_err(|e| e.to_string())?,
                };
                if search::matches(&item, &query.text) {
                    summaries.push(item.summary);
                }
            } else {
                summaries.push(summary);
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
            let kind = match s.kind {
                ItemKind::Clipboard => 0,
                ItemKind::Folder => 1,
                ItemKind::Document => 2,
            };
            tx.execute("INSERT INTO items(id,kind,title,pinned,created_at_ms,last_used_ms,use_count,summary,text,target,open_with) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,title=excluded.title,pinned=excluded.pinned,created_at_ms=excluded.created_at_ms,last_used_ms=excluded.last_used_ms,use_count=excluded.use_count,summary=excluded.summary,text=excluded.text,target=excluded.target,open_with=excluded.open_with", params![id,kind,s.title,s.pinned,s.created_at_ms,s.last_used_ms,i64::try_from(s.use_count).unwrap_or(i64::MAX),summary,item.text,item.target,item.open_with]).map_err(|e| e.to_string())?;
            if change.replace_representations {
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
    pub fn checkpoint(&self) {
        let _ = self
            .connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE)");
    }
}
