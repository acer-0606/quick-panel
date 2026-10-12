//! Derived vector storage. Schema creation and writes only happen in dirty commits.
use crate::{
    model::*,
    repository::PendingItem,
    semantic::{self, IndexScope, IndexedEmbedding},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

pub(crate) const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
pub(crate) fn register() -> Result<(), String> {
    static REGISTERED: OnceLock<i32> = OnceLock::new();
    let rc = *REGISTERED.get_or_init(|| unsafe {
        // sqlite-vec exports the C extension initializer; SQLite calls it for each
        // subsequently opened connection. The process registration is one-time.
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut std::ffi::c_char,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )))
    });
    if rc == rusqlite::ffi::SQLITE_OK {
        Ok(())
    } else {
        Err("无法加载语义索引扩展".into())
    }
}
pub(crate) fn normalize(vector: &[f32]) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    vector
        .iter()
        .map(|v| (f64::from(*v) / norm) as f32)
        .collect()
}
fn blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}
#[derive(Default)]
pub(crate) struct PendingVectors {
    pub profile: String,
    pub dimensions: usize,
    pub items: HashMap<String, IndexedEmbedding>,
}
impl PendingVectors {
    pub fn bytes(&self) -> usize {
        self.items.iter().fold(
            map_allocation_bytes(self.items.capacity()).saturating_add(self.profile.capacity()),
            |bytes, (key, item)| {
                bytes
                    .saturating_add(key.capacity())
                    .saturating_add(owned_bytes(item))
            },
        )
    }
    /// Predict the actual post-admission map capacity without growing the live
    /// cache before the batch is admitted. This map only grows by insert and is
    /// replaced by an empty map on successful commit; no reserve/remove paths.
    pub fn projected_bytes(&self, profile_capacity: usize, items: &[IndexedEmbedding]) -> usize {
        let new_keys = items
            .iter()
            .filter(|item| !self.items.contains_key(&item.id))
            .count();
        let length = self.items.len().saturating_add(new_keys);
        let capacity = if length <= self.items.capacity() {
            self.items.capacity()
        } else {
            HashMap::<String, IndexedEmbedding>::with_capacity(length).capacity()
        };
        let mut owned = self.items.iter().fold(0usize, |bytes, (key, item)| {
            bytes
                .saturating_add(key.capacity())
                .saturating_add(owned_bytes(item))
        });
        for item in items {
            if let Some(old) = self.items.get(&item.id) {
                owned = owned.saturating_sub(owned_bytes(old))
            } else {
                owned = owned.saturating_add(item.id.len())
            }
            owned = owned.saturating_add(owned_bytes(item));
        }
        map_allocation_bytes(capacity)
            .saturating_add(profile_capacity)
            .saturating_add(owned)
    }
}
fn owned_bytes(item: &IndexedEmbedding) -> usize {
    item.id
        .capacity()
        .saturating_add(item.text_hash.capacity())
        .saturating_add(
            item.vector
                .capacity()
                .saturating_mul(std::mem::size_of::<f32>()),
        )
}
fn map_allocation_bytes(capacity: usize) -> usize {
    // std's hashbrown table uses power-of-two buckets with 7/8 usable slots
    // (small tables reserve one slot). Recover a conservative bucket count
    // from usable capacity, count every tuple and control byte, and allow 64
    // bytes for the control group/alignment tail on supported desktop targets.
    let buckets = if capacity == 0 {
        0
    } else {
        capacity
            .saturating_add(1)
            .checked_next_power_of_two()
            .unwrap_or(usize::MAX)
    };
    buckets
        .saturating_mul(std::mem::size_of::<(String, IndexedEmbedding)>() + 1)
        .saturating_add(if buckets == 0 { 0 } else { 64 })
        .saturating_add(std::mem::size_of::<PendingVectors>())
}
pub(crate) fn embedding_bytes(item: &IndexedEmbedding) -> usize {
    // Command queue estimate only. The retained vector cache uses the actual
    // and projected table capacities above, including its profile allocation.
    item.id
        .capacity()
        .saturating_add(item.id.len())
        .saturating_add(item.text_hash.capacity())
        .saturating_add(item.vector.capacity().saturating_mul(4))
        .saturating_add(192)
}
#[cfg(test)]
thread_local! { pub(crate) static PROFILE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(crate) fn active(db: &Connection) -> Result<Option<(String, usize)>, String> {
    #[cfg(test)]
    PROFILE_READS.with(|n| n.set(n.get() + 1));
    let exists: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='semantic_profile'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Ok(None);
    }
    db.query_row(
        "SELECT profile,dimensions FROM semantic_profile WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get::<_, u32>(1)? as usize)),
    )
    .optional()
    .map_err(|e| e.to_string())
}
/// Caller reuses the verified active profile for its current scan.
pub(crate) fn indexed_in_active_profile(
    db: &Connection,
    id: &str,
    hash: &str,
) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM semantic_map m JOIN semantic_vectors v ON v.rowid=m.rowid WHERE m.item_id=?1 AND m.text_hash=?2)",
        params![id, hash],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
#[cfg(test)]
thread_local! { pub(crate) static BODY_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
/// Return at most 128 scope-eligible metadata rows. The actor merges pending
/// eligibility/content/deletion before loading any body, then stops at its quota.
pub(crate) fn page(
    db: &Connection,
    after: i64,
    scope: crate::semantic::IndexScope,
    mut visit: impl FnMut(StoredItem) -> Result<bool, String>,
) -> Result<(i64, bool), String> {
    let mut stmt=db.prepare("SELECT rowid,summary,target,open_with FROM items WHERE rowid>?1 AND (?2 OR kind<>0 OR pinned=1) ORDER BY rowid LIMIT 128").map_err(|e|e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params![
            after,
            scope == crate::semantic::IndexScope::AllText
        ])
        .map_err(|e| e.to_string())?;
    let mut last = after;
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        last = row.get(0).map_err(|e| e.to_string())?;
        count += 1;
        let item = StoredItem {
            summary: serde_json::from_str(&row.get::<_, String>(1).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
            text: String::new(),
            target: row.get(2).map_err(|e| e.to_string())?,
            open_with: row.get(3).map_err(|e| e.to_string())?,
            representations: vec![],
        };
        if !visit(item)? {
            return Ok((last, false));
        }
    }
    Ok((last, count < 128))
}
fn remove(db: &Connection, id: &str) -> Result<(), String> {
    db.execute("DELETE FROM semantic_vectors WHERE rowid=(SELECT rowid FROM semantic_map WHERE item_id=?1)",[id]).map_err(|e|e.to_string())?;
    db.execute("DELETE FROM semantic_map WHERE item_id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
/// Called after source mutations and retention, inside the same transaction.
pub(crate) fn commit(
    db: &Connection,
    changes: &HashMap<String, PendingItem>,
    vectors: &PendingVectors,
    deleted: &HashSet<String>,
    repair: bool,
) -> Result<(), String> {
    let mut current = active(db)?;
    // A completed first batch determines profile/dimension. Replacement is atomic
    // with both original data and vector rows, including rollback on write errors.
    if !vectors.items.is_empty()
        && current.as_ref() != Some(&(vectors.profile.clone(), vectors.dimensions))
    {
        db.execute_batch("DROP TABLE IF EXISTS semantic_vectors; DROP TABLE IF EXISTS semantic_map; CREATE TABLE IF NOT EXISTS semantic_profile(singleton INTEGER PRIMARY KEY CHECK(singleton=1),profile TEXT NOT NULL,dimensions INTEGER NOT NULL); CREATE TABLE semantic_map(rowid INTEGER PRIMARY KEY AUTOINCREMENT,item_id TEXT NOT NULL UNIQUE,text_hash TEXT NOT NULL);").map_err(|e|e.to_string())?;
        db.execute_batch(&format!("CREATE VIRTUAL TABLE semantic_vectors USING vec0(embedding float[{}] distance_metric=cosine)",vectors.dimensions)).map_err(|e|e.to_string())?;
        db.execute("INSERT INTO semantic_profile VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET profile=excluded.profile,dimensions=excluded.dimensions",params![vectors.profile,vectors.dimensions as u32]).map_err(|e|e.to_string())?;
        current = Some((vectors.profile.clone(), vectors.dimensions));
    }
    if current.is_none() {
        return Ok(());
    }
    if repair {
        // Orphan cleanup covers explicit deletion and both retention policies.
        db.execute("DELETE FROM semantic_vectors WHERE rowid IN (SELECT m.rowid FROM semantic_map m LEFT JOIN items i ON i.id=m.item_id WHERE i.id IS NULL)",[]).map_err(|e|e.to_string())?;
        db.execute(
            "DELETE FROM semantic_map WHERE item_id NOT IN (SELECT id FROM items)",
            [],
        )
        .map_err(|e| e.to_string())?;
    }
    for id in deleted {
        remove(db, id)?;
    }
    for (id, change) in changes {
        if !matches!(
            change.mutation,
            crate::repository::Mutation::Metadata | crate::repository::Mutation::Eligibility
        ) {
            let mut source = change.item.clone();
            if change.mutation == crate::repository::Mutation::SearchMetadata {
                source.text = db
                    .query_row("SELECT text FROM items WHERE id=?1", [id], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?
                    .unwrap_or_default();
            }
            let hash = semantic::candidate(&source, IndexScope::AllText).map(|c| c.text_hash);
            let stored: Option<String> = db
                .query_row(
                    "SELECT text_hash FROM semantic_map WHERE item_id=?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if stored.is_some() && stored != hash {
                remove(db, id)?
            }
        }
    }
    for item in vectors.items.values() {
        // Never resurrect a source removed by retention in this transaction.
        let raw: Option<(String, String, Option<String>, Option<String>)> = db
            .query_row(
                "SELECT summary,text,target,open_with FROM items WHERE id=?1",
                [&item.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((summary, text, target, open_with)) = raw else {
            continue;
        };
        let source = StoredItem {
            summary: serde_json::from_str(&summary).map_err(|e| e.to_string())?,
            text,
            target,
            open_with,
            representations: vec![],
        };
        if semantic::candidate(&source, IndexScope::AllText)
            .is_none_or(|c| c.text_hash != item.text_hash)
        {
            continue;
        }
        remove(db, &item.id)?;
        db.execute(
            "INSERT INTO semantic_map(item_id,text_hash) VALUES(?1,?2)",
            params![item.id, item.text_hash],
        )
        .map_err(|e| e.to_string())?;
        let rowid = db.last_insert_rowid();
        db.execute(
            "INSERT INTO semantic_vectors(rowid,embedding) VALUES(?1,?2)",
            params![rowid, blob(&item.vector)],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
/// vec0 receives only eligible rowids before k=50. SQL overlays pin changes;
/// content changes/deletions and RAM replacements are excluded before truncation.
pub(crate) fn nearest(
    db: &Connection,
    profile: &str,
    vector: &[f32],
    scope: Scope,
    index_scope: IndexScope,
    excluded: &HashSet<String>,
    pins: &HashMap<String, bool>,
) -> Result<Vec<(String, f32)>, String> {
    if active(db)?.is_none_or(|(p, d)| p != profile || d != vector.len()) {
        return Ok(vec![]);
    }
    let kind = match scope {
        Scope::All => -1,
        Scope::Clipboard => 0,
        Scope::Folder => 1,
        Scope::Document => 2,
    };
    let excluded = serde_json::to_string(excluded).map_err(|e| e.to_string())?;
    let pinned: Vec<_> = pins
        .iter()
        .filter(|(_, value)| **value)
        .map(|(id, _)| id)
        .collect();
    let unpinned: Vec<_> = pins
        .iter()
        .filter(|(_, value)| !**value)
        .map(|(id, _)| id)
        .collect();
    let pinned = serde_json::to_string(&pinned).map_err(|e| e.to_string())?;
    let unpinned = serde_json::to_string(&unpinned).map_err(|e| e.to_string())?;
    let mut stmt=db.prepare("SELECT m.item_id,v.distance FROM semantic_vectors v JOIN semantic_map m ON m.rowid=v.rowid WHERE v.embedding MATCH ?1 AND k=50 AND v.rowid IN (SELECT m.rowid FROM semantic_map m JOIN items i ON i.id=m.item_id WHERE (?2=-1 OR i.kind=?2) AND (?3=1 OR i.kind<>0 OR (i.id IN (SELECT value FROM json_each(?5)) OR (i.pinned=1 AND i.id NOT IN (SELECT value FROM json_each(?6))))) AND i.id NOT IN (SELECT value FROM json_each(?4))) ORDER BY v.distance,m.item_id").map_err(|e|e.to_string())?;
    stmt.query_map(
        params![
            blob(&normalize(vector)),
            kind,
            index_scope == IndexScope::AllText,
            excluded,
            pinned,
            unpinned
        ],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(|e| e.to_string())?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| e.to_string())
}

/// An unchanged derived input can reuse disk coordinates under a content
/// overlay (for example an open_with-only favorite edit), without any writes.
pub(crate) fn matching_vector(
    db: &Connection,
    profile: &str,
    id: &str,
    hash: &str,
    dimensions: usize,
) -> Result<Option<Vec<f32>>, String> {
    if active(db)?.is_none_or(|(p, d)| p != profile || d != dimensions) {
        return Ok(None);
    }
    let bytes:Option<Vec<u8>>=db.query_row("SELECT v.embedding FROM semantic_map m JOIN semantic_vectors v ON v.rowid=m.rowid WHERE m.item_id=?1 AND m.text_hash=?2",params![id,hash],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    bytes
        .map(|bytes| {
            if bytes.len() != dimensions * 4 {
                return Err("语义索引数据无效".into());
            }
            let vector: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            crate::semantic::validate_vectors(std::slice::from_ref(&vector))?;
            Ok(vector)
        })
        .transpose()
}
