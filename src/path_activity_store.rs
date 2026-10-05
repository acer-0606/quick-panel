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
const HALF_LIFE_MS: f64 = 7.0 * 86_400_000.0;
#[derive(Clone)]
struct Row {
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
        let mut statement = db.prepare("SELECT path,kind,visits,last_seen_ms,weight,hidden FROM path_activity WHERE length(path)<=8192 ORDER BY hidden DESC,last_seen_ms DESC,path LIMIT 1152").map_err(|e| e.to_string())?;
        let records = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, bool>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for record in records {
            let (path, kind, visits, last_seen, weight, hidden) =
                record.map_err(|e| e.to_string())?;
            if normalize_path(&path).as_ref() != Ok(&path)
                || !matches!(kind, 1 | 2)
                || visits < 0
                || !weight.is_finite()
                || weight < 0.0
                || !store.can_add(&path, hidden)
            {
                continue;
            }
            store.bytes += Self::row_bytes(&path);
            store.rows.insert(
                path,
                Row {
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
    fn row_bytes(path: &str) -> usize {
        // Path plus dirty-set key, node/row overhead, and temporary search metadata.
        path.len().saturating_mul(4).saturating_add(256)
    }
    fn can_add(&self, path: &str, hidden: bool) -> bool {
        self.rows.values().filter(|r| r.hidden == hidden).count()
            < if hidden { MAX_HIDDEN } else { MAX_ACTIVE }
            && self.bytes.saturating_add(Self::row_bytes(path)) <= MAX_BYTES
    }
    pub fn is_dirty(&self) -> bool {
        self.cleared || !self.dirty.is_empty()
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
        let mut changed = false;
        for observation in observations.into_iter().take(MAX_BATCH) {
            if observation.kind == ItemKind::Clipboard
                || (observation.source == ObservationSource::RecentDocuments
                    && !settings.recent_documents)
            {
                continue;
            }
            let Ok(path) = normalize_path(&observation.path) else {
                continue;
            };
            if !seen.insert(path.clone()) || excluded(&path, &settings.excluded_paths) {
                continue;
            }
            let time = observation.observed_at_ms.clamp(0, now.max(0));
            let contribution = if observation.source == ObservationSource::RecentDocuments {
                0.5
            } else {
                1.0
            };
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
            } else {
                if !self.can_add(&path, false) {
                    continue;
                }
                self.bytes += Self::row_bytes(&path);
                self.rows.insert(
                    path.clone(),
                    Row {
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
        let path = normalize_path(path)?;
        if self.rows.get(&path).is_some_and(|r| r.hidden) {
            return Ok(false);
        }
        if self.rows.values().filter(|r| r.hidden).count() >= MAX_HIDDEN {
            return Err("不再推荐记录已达上限".into());
        }
        if let Some(row) = self.rows.get_mut(&path) {
            row.hidden = true;
        } else {
            if !self.can_add(&path, true) {
                return Err("路径缓存已满".into());
            }
            self.bytes += Self::row_bytes(&path);
            self.rows.insert(
                path.clone(),
                Row {
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
        if self.rows.is_empty() {
            return false;
        }
        self.rows.clear();
        self.dirty.clear();
        self.bytes = 0;
        self.cleared = true;
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
            let title = path
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
                    alias: String::new(),
                    tags: vec![],
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
                path: path.clone(),
                title: path
                    .trim_end_matches('\\')
                    .rsplit('\\')
                    .next()
                    .unwrap_or(path)
                    .to_owned(),
                kind: row.kind,
                visits: row.visits,
                last_seen_ms: row.last_seen,
            })
            .collect())
    }
    pub fn write(&self, tx: &Transaction<'_>) -> Result<(), String> {
        if !self.is_dirty() {
            return Ok(());
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS path_activity(path TEXT PRIMARY KEY,kind INTEGER NOT NULL,visits INTEGER NOT NULL,last_seen_ms INTEGER NOT NULL,weight REAL NOT NULL,hidden INTEGER NOT NULL)").map_err(|e| e.to_string())?;
        if self.cleared {
            tx.execute("DELETE FROM path_activity", [])
                .map_err(|e| e.to_string())?;
        }
        for path in &self.dirty {
            if let Some(row) = self.rows.get(path) {
                tx.execute("INSERT INTO path_activity VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(path) DO UPDATE SET kind=excluded.kind,visits=excluded.visits,last_seen_ms=excluded.last_seen_ms,weight=excluded.weight,hidden=excluded.hidden", params![path, if row.kind == ItemKind::Folder { 1 } else { 2 }, row.visits as i64, row.last_seen, row.weight, row.hidden]).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    pub fn committed(&mut self) {
        self.dirty.clear();
        self.cleared = false;
    }
}
fn decay(weight: f64, last: i64, now: i64) -> f64 {
    weight * 2.0_f64.powf(-(now.saturating_sub(last).max(0) as f64) / HALF_LIFE_MS)
}
