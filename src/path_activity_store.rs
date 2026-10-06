//! Bounded actor-owned cache. Only dirty rows are written in the repository transaction.
use crate::{
    model::{ItemKind, ItemSummary, Scope, StoredItem},
    path_activity::*,
    search,
};
use rusqlite::{Connection, Transaction, params};
use std::collections::{BTreeMap, BTreeSet};
const MAX_ACTIVE: usize = 1024;
const MAX_HIDDEN: usize = 128;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const RETENTION_MS: i64 = 90 * 86_400_000;
const HALF_LIFE_MS: f64 = 7.0 * 86_400_000.0;
#[derive(Clone)]
struct Row {
    target: String,
    alias: String,
    tags: Vec<String>,
    kind: ItemKind,
    visits: u64,
    last_seen: i64,
    weight: f64,
    hidden: bool,
}
pub(crate) struct PathSearch<'a> {
    pub terms: &'a [String],
    pub scope: Scope,
    pub now: i64,
    pub from_ms: Option<i64>,
    pub until_ms: Option<i64>,
}
#[derive(Default)]
pub(crate) struct PathActivityStore {
    rows: BTreeMap<String, Row>,
    dirty: BTreeSet<String>,
    cleared: bool,
    // One predicate represents all pending expired deletions, even across failed saves.
    expired_before: Option<i64>,
    bytes: usize,
}
impl PathActivityStore {
    pub fn load(db: &Connection) -> Result<Self, String> {
        let mut store = Self::default();
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='path_activity')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Ok(store);
        }
        let target_column = if has_target_column(db)? {
            "coalesce(target,path)"
        } else {
            "path"
        };
        let metadata_column = if has_column(db, "alias")? {
            "coalesce(alias,''),coalesce(tags,'[]')"
        } else {
            "'','[]'"
        };
        let sql = format!(
            "SELECT path,kind,visits,last_seen_ms,weight,hidden,{target_column},{metadata_column} FROM path_activity WHERE length(path)<=8192 AND length({target_column})<=8192 AND length({metadata_column_alias})<=192 AND length({metadata_column_tags})<=4096 ORDER BY hidden DESC,last_seen_ms DESC,path LIMIT 1152",
            metadata_column_alias = if has_column(db, "alias")? {
                "coalesce(alias,'')"
            } else {
                "''"
            },
            metadata_column_tags = if has_column(db, "tags")? {
                "coalesce(tags,'[]')"
            } else {
                "'[]'"
            }
        );
        let mut statement = db.prepare(&sql).map_err(|e| e.to_string())?;
        let records = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, bool>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for record in records {
            let (path, kind, visits, last_seen, weight, hidden, target, alias, tags) =
                record.map_err(|e| e.to_string())?;
            if normalize_path(&target).as_ref() != Ok(&path)
                || !matches!(kind, 1 | 2)
                || visits < 0
                || !weight.is_finite()
                || weight < 0.0
                || !store.can_add(&path, &target, hidden)
            {
                continue;
            }
            let tags: Vec<String> = serde_json::from_str(&tags).unwrap_or_default();
            if alias.chars().count() > 48
                || tags.len() > 32
                || tags.iter().any(|t| t.chars().count() > 24)
            {
                continue;
            }
            let extra = Self::metadata_bytes(&alias, &tags);
            if store.bytes + Self::row_bytes(&path, &target) + extra > MAX_BYTES {
                continue;
            }
            store.bytes += Self::row_bytes(&path, &target) + extra;
            store.rows.insert(
                path,
                Row {
                    target,
                    alias,
                    tags,
                    kind: if kind == 1 {
                        ItemKind::Folder
                    } else {
                        ItemKind::Document
                    },
                    visits: visits as u64,
                    last_seen,
                    weight: weight.min(1e9),
                    hidden,
                },
            );
        }
        Ok(store)
    }
    fn metadata_bytes(alias: &str, tags: &[String]) -> usize {
        alias.len()
            + tags
                .iter()
                .map(|t| t.len() + std::mem::size_of::<String>())
                .sum::<usize>()
    }
    pub fn can_annotate(&self, updates: &[(String, String, Vec<String>)]) -> bool {
        let mut bytes = self.bytes;
        for (path, alias, tags) in updates {
            if let Some(row) = self.rows.get(path) {
                bytes = bytes
                    .saturating_sub(Self::metadata_bytes(&row.alias, &row.tags))
                    .saturating_add(Self::metadata_bytes(alias, tags));
            }
        }
        bytes <= MAX_BYTES
    }
    fn row_bytes(path: &str, target: &str) -> usize {
        // Path plus dirty-set key, node/row overhead, and temporary search metadata.
        path.len()
            .saturating_mul(2)
            .saturating_add(target.len().saturating_mul(2))
            .saturating_add(256)
    }
    fn can_add(&self, path: &str, target: &str, hidden: bool) -> bool {
        self.rows.values().filter(|r| r.hidden == hidden).count()
            < if hidden { MAX_HIDDEN } else { MAX_ACTIVE }
            && self.bytes.saturating_add(Self::row_bytes(path, target)) <= MAX_BYTES
    }
    pub fn organizer_item(
        &self,
        path: &str,
        settings: &PathActivitySettings,
        now: i64,
    ) -> Option<(String, ItemKind, String, Vec<String>)> {
        let row = self.rows.get(path)?;
        if !settings.enabled
            || row.hidden
            || row.last_seen < now.saturating_sub(RETENTION_MS)
            || excluded(path, &settings.excluded_paths)
        {
            return None;
        }
        Some((
            row.target.clone(),
            row.kind,
            row.alias.clone(),
            row.tags.clone(),
        ))
    }
    pub fn observation_revision(&self, path: &str) -> Option<(u64, i64)> {
        self.rows.get(path).map(|r| (r.visits, r.last_seen))
    }
    pub fn annotate(&mut self, path: &str, alias: String, tags: Vec<String>) {
        if let Some(row) = self.rows.get_mut(path) {
            self.bytes = self.bytes - Self::metadata_bytes(&row.alias, &row.tags)
                + Self::metadata_bytes(&alias, &tags);
            row.alias = alias;
            row.tags = tags;
            self.dirty.insert(path.into());
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.cleared || self.expired_before.is_some() || !self.dirty.is_empty()
    }
    pub fn expire(&mut self, now: i64) -> bool {
        let cutoff = now.saturating_sub(RETENTION_MS);
        let before = self.rows.len();
        self.rows.retain(|key, row| {
            if !row.hidden && row.last_seen < cutoff {
                self.bytes = self.bytes.saturating_sub(
                    Self::row_bytes(key, &row.target) + Self::metadata_bytes(&row.alias, &row.tags),
                );
                self.dirty.remove(key);
                false
            } else {
                true
            }
        });
        let changed = self.rows.len() != before;
        if changed {
            self.expired_before = Some(self.expired_before.map_or(cutoff, |old| old.max(cutoff)));
        }
        changed
    }
    pub fn observe(
        &mut self,
        observations: Vec<PathObservation>,
        settings: &PathActivitySettings,
        now: i64,
    ) -> bool {
        if !settings.enabled {
            return false;
        }
        let mut seen = BTreeSet::new();
        let mut changed = self.expire(now);
        for observation in observations.into_iter().take(MAX_BATCH) {
            if observation.kind == ItemKind::Clipboard
                || (observation.source == ObservationSource::RecentDocuments
                    && !settings.recent_documents)
            {
                continue;
            }
            let Ok(target) = normalize_target(&observation.path) else {
                continue;
            };
            let Ok(path) = normalize_path(&target) else {
                continue;
            };
            if !seen.insert(path.clone()) || excluded(&path, &settings.excluded_paths) {
                continue;
            }
            let time = observation.observed_at_ms.clamp(0, now.max(0));
            if time < now.saturating_sub(RETENTION_MS) {
                continue;
            }
            let contribution = if observation.source == ObservationSource::RecentDocuments {
                0.5
            } else {
                1.0
            };
            let old_bytes = self
                .rows
                .get(&path)
                .map_or(0, |row| Self::row_bytes(&path, &row.target));
            let new_bytes = Self::row_bytes(&path, &target);
            if self
                .bytes
                .saturating_sub(old_bytes)
                .saturating_add(new_bytes)
                > MAX_BYTES
            {
                continue;
            }
            if let Some(row) = self.rows.get_mut(&path) {
                if row.hidden
                    || (time >= row.last_seen && time.saturating_sub(row.last_seen) < 30_000)
                {
                    continue;
                }
                row.weight = (decay(row.weight, row.last_seen, time) + contribution).min(1e9);
                row.visits = row.visits.saturating_add(1).min(i64::MAX as u64);
                row.last_seen = time;
                row.kind = observation.kind;
                row.target = target;
                self.bytes = self.bytes - old_bytes + new_bytes;
            } else {
                if !self.can_add(&path, &target, false) {
                    continue;
                }
                self.bytes += Self::row_bytes(&path, &target);
                self.rows.insert(
                    path.clone(),
                    Row {
                        target,
                        alias: String::new(),
                        tags: vec![],
                        kind: observation.kind,
                        visits: 1,
                        last_seen: time,
                        weight: contribution,
                        hidden: false,
                    },
                );
            }
            self.dirty.insert(path);
            changed = true;
        }
        changed
    }
    pub fn suppress(&mut self, path: &str) -> Result<bool, String> {
        let target = normalize_target(path)?;
        let path = normalize_path(&target)?;
        if self.rows.get(&path).is_some_and(|r| r.hidden) {
            return Ok(false);
        }
        if self.rows.values().filter(|r| r.hidden).count() >= MAX_HIDDEN {
            return Err("不再推荐记录已达上限".into());
        }
        if let Some(row) = self.rows.get_mut(&path) {
            row.hidden = true;
        } else {
            if !self.can_add(&path, &target, true) {
                return Err("路径缓存已满".into());
            }
            self.bytes += Self::row_bytes(&path, &target);
            self.rows.insert(
                path.clone(),
                Row {
                    target,
                    alias: String::new(),
                    tags: vec![],
                    kind: ItemKind::Folder,
                    visits: 0,
                    last_seen: 0,
                    weight: 0.0,
                    hidden: true,
                },
            );
        }
        self.dirty.insert(path);
        Ok(true)
    }
    pub fn clear(&mut self) -> bool {
        if self.rows.is_empty() && !self.is_dirty() {
            return false;
        }
        self.rows.clear();
        self.dirty.clear();
        self.bytes = 0;
        self.cleared = true;
        self.expired_before = None;
        true
    }
    pub fn suggestions(
        &self,
        query: PathSearch<'_>,
        settings: &PathActivitySettings,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Vec<PathSuggestion>, String> {
        let PathSearch {
            terms,
            scope,
            now,
            from_ms,
            until_ms,
        } = query;
        if !settings.enabled || scope == Scope::Clipboard {
            return Ok(vec![]);
        }
        let terms: Vec<_> = terms
            .iter()
            .map(|term| search::normalize(term.trim()))
            .collect();
        let empty = terms.iter().all(String::is_empty);
        let mut ranked = Vec::new();
        for (path, row) in &self.rows {
            checkpoint()?;
            if from_ms.is_some_and(|from| row.last_seen < from)
                || until_ms.is_some_and(|until| row.last_seen >= until)
            {
                continue;
            }
            if row.hidden
                || row.last_seen < now.saturating_sub(RETENTION_MS)
                || excluded(path, &settings.excluded_paths)
                || !matches!(
                    (row.kind, scope),
                    (_, Scope::All)
                        | (ItemKind::Folder, Scope::Folder)
                        | (ItemKind::Document, Scope::Document)
                )
            {
                continue;
            }
            let title = row
                .target
                .trim_end_matches('\\')
                .rsplit('\\')
                .next()
                .unwrap_or(path)
                .to_owned();
            let item = StoredItem {
                summary: ItemSummary {
                    id: path.clone(),
                    title: title.clone(),
                    subtitle: path.clone(),
                    kind: row.kind,
                    alias: row.alias.clone(),
                    tags: row.tags.clone(),
                    pinned: false,
                    created_at_ms: row.last_seen,
                    last_used_ms: row.last_seen,
                    use_count: row.visits,
                },
                text: String::new(),
                target: Some(path.clone()),
                representations: vec![],
                open_with: None,
            };
            if !empty
                && !terms
                    .iter()
                    .any(|term| search::matches_normalized(&item, term))
            {
                continue;
            }
            ranked.push((decay(row.weight, row.last_seen, now), path, row));
        }
        ranked.sort_by(|(a, x, xr), (b, y, yr)| {
            b.total_cmp(a)
                .then(yr.last_seen.cmp(&xr.last_seen))
                .then(x.cmp(y))
        });
        ranked.truncate(if empty { 5 } else { 10 });
        checkpoint()?;
        Ok(ranked
            .into_iter()
            .map(|(_, path, row)| PathSuggestion {
                key: path.clone(),
                path: row.target.clone(),
                title: row
                    .target
                    .trim_end_matches('\\')
                    .rsplit('\\')
                    .next()
                    .unwrap_or(path)
                    .to_owned(),
                kind: row.kind,
                alias: row.alias.clone(),
                tags: row.tags.clone(),
                visits: row.visits,
                last_seen_ms: row.last_seen,
            })
            .collect())
    }
    pub fn write(&self, tx: &Transaction<'_>) -> Result<(), String> {
        if !self.is_dirty() {
            return Ok(());
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS path_activity(path TEXT PRIMARY KEY,kind INTEGER NOT NULL,visits INTEGER NOT NULL,last_seen_ms INTEGER NOT NULL,weight REAL NOT NULL,hidden INTEGER NOT NULL,target TEXT)").map_err(|e| e.to_string())?;
        if !has_target_column(tx)? {
            tx.execute("ALTER TABLE path_activity ADD COLUMN target TEXT", [])
                .map_err(|e| e.to_string())?;
        }
        for (column, kind) in [("alias", "TEXT"), ("tags", "TEXT")] {
            if !has_column(tx, column)? {
                tx.execute(
                    &format!("ALTER TABLE path_activity ADD COLUMN {column} {kind}"),
                    [],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        if self.cleared {
            tx.execute("DELETE FROM path_activity", [])
                .map_err(|e| e.to_string())?;
        }
        if let Some(cutoff) = self.expired_before {
            tx.execute(
                "DELETE FROM path_activity WHERE hidden=0 AND last_seen_ms<?1",
                [cutoff],
            )
            .map_err(|e| e.to_string())?;
        }
        for path in &self.dirty {
            if let Some(row) = self.rows.get(path) {
                tx.execute("INSERT INTO path_activity(path,kind,visits,last_seen_ms,weight,hidden,target,alias,tags) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(path) DO UPDATE SET kind=excluded.kind,visits=excluded.visits,last_seen_ms=excluded.last_seen_ms,weight=excluded.weight,hidden=excluded.hidden,target=excluded.target,alias=excluded.alias,tags=excluded.tags", params![path, if row.kind == ItemKind::Folder { 1 } else { 2 }, row.visits as i64, row.last_seen, row.weight, row.hidden, row.target, row.alias, serde_json::to_string(&row.tags).map_err(|e|e.to_string())?]).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    pub fn committed(&mut self) {
        self.dirty.clear();
        self.cleared = false;
        self.expired_before = None;
    }
}
fn decay(weight: f64, last: i64, now: i64) -> f64 {
    weight * 2.0_f64.powf(-(now.saturating_sub(last).max(0) as f64) / HALF_LIFE_MS)
}

fn has_target_column(db: &Connection) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('path_activity') WHERE name='target')",
        [],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

fn has_column(db: &Connection, column: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('path_activity') WHERE name=?1)",
        [column],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
