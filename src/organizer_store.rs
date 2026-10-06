//! Bounded durable organizer state, committed with source metadata by Repository.
use crate::organizer::*;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Status {
    Queued,
    Processing,
    Ready(Proposal),
    Done,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub candidate: Candidate,
    pub status: Status,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct OrganizerStore {
    #[serde(skip)]
    pub blocked: bool,
    pub records: Vec<Record>,
    pub failures: u8,
    pub retry_at_ms: Option<i64>,
    pub next_at_ms: i64,
    pub paused: bool,
    pub saturated: bool,
    #[serde(skip)]
    pub dirty: bool,
}
impl OrganizerStore {
    pub fn load(db: &Connection) -> Result<Self, String> {
        // An optional cache must never prevent opening the user's source history.
        Ok(Self::load_checked(db).unwrap_or_else(|_| Self {
            blocked: true,
            paused: true,
            saturated: true,
            ..Self::default()
        }))
    }
    fn load_checked(db: &Connection) -> Result<Self, String> {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='organizer_state')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Ok(Self::default());
        }
        let oversized: bool=db.query_row("SELECT EXISTS(SELECT 1 FROM organizer_state WHERE id=1 AND length(CAST(data AS BLOB))>?1)",[MAX_STORE_BYTES as i64],|r|r.get(0)).map_err(|e|e.to_string())?;
        if oversized {
            return Err("整理缓存超过上限".into());
        }
        let json: Option<String> = db
            .query_row(
                "SELECT data FROM organizer_state WHERE id=1 AND length(CAST(data AS BLOB))<=?1",
                [MAX_STORE_BYTES as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(json) = json else {
            return Ok(Self::default());
        };
        let mut state: Self =
            serde_json::from_str(&json).map_err(|_| "整理缓存无效".to_string())?;
        state.records.shrink_to_fit();
        if !state.fits() {
            return Err("整理缓存超过上限".into());
        }
        let mut seen = std::collections::HashSet::new();
        for r in &mut state.records {
            crate::organizer::validate_candidate(&r.candidate)?;
            if !seen.insert(r.candidate.source.clone()) {
                return Err("整理缓存来源重复".into());
            }
            if let Status::Ready(p) = &r.status {
                p.validate()?;
                if p.candidate != r.candidate {
                    return Err("整理缓存版本无效".into());
                }
            }
            if matches!(r.status, Status::Processing) {
                r.status = Status::Queued;
                state.dirty = true;
            }
        }
        state.failures = state.failures.min(3);
        Ok(state)
    }
    pub fn ready_count(&self) -> usize {
        self.records
            .iter()
            .filter(|r| matches!(r.status, Status::Ready(_)))
            .count()
    }
    pub fn snapshot(&self, in_flight: bool) -> Snapshot {
        Snapshot {
            storage_error: self
                .blocked
                .then(|| "整理缓存超限或损坏，管家已暂停；原始内容不受影响".into()),
            proposals: self
                .records
                .iter()
                .filter_map(|r| {
                    if let Status::Ready(p) = &r.status {
                        Some(p.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            queued: self
                .records
                .iter()
                .filter(|r| matches!(r.status, Status::Queued))
                .count(),
            in_flight,
            failures: self.failures,
            retry_at_ms: self.retry_at_ms,
            paused: self.paused,
            saturated: self.saturated || self.ready_count() >= MAX_READY_PROPOSALS,
        }
    }
    pub fn fits(&self) -> bool {
        let memory = std::mem::size_of::<Self>()
            + self.records.capacity() * std::mem::size_of::<Record>()
            + self
                .records
                .iter()
                .map(|r| {
                    candidate_bytes(&r.candidate)
                        + if let Status::Ready(p) = &r.status {
                            candidate_bytes(&p.candidate)
                                + p.alias.as_ref().map_or(0, String::capacity)
                                + tags_bytes(&p.tags)
                                + p.reason.capacity()
                        } else {
                            0
                        }
                })
                .sum::<usize>();
        self.records.len() <= MAX_RECORDS
            && self.ready_count() <= MAX_READY_PROPOSALS
            && memory <= MAX_STORE_BYTES
            && serde_json::to_vec(self).is_ok_and(|b| b.len() <= MAX_STORE_BYTES)
    }
    pub fn enqueue(&mut self, candidate: Candidate) -> bool {
        if self.blocked {
            return false;
        }
        if self.records.iter().any(|r| {
            r.candidate.source == candidate.source
                && r.candidate.fingerprint == candidate.fingerprint
        }) {
            return false;
        }
        let mut next = self.clone();
        next.records
            .retain(|r| r.candidate.source != candidate.source);
        next.records.push(Record {
            candidate,
            status: Status::Queued,
        });
        next.records.shrink_to_fit();
        while !next.fits() {
            let Some(i) = next
                .records
                .iter()
                .position(|r| matches!(r.status, Status::Done))
            else {
                let changed = !self.saturated;
                self.saturated = true;
                self.dirty |= changed;
                return changed;
            };
            next.records.remove(i);
        }
        next.saturated = false;
        next.dirty = true;
        *self = next;
        true
    }
    pub fn reset(&mut self) {
        if self.blocked {
            return;
        }
        self.dirty |=
            self.failures != 0 || self.retry_at_ms.is_some() || self.paused || self.next_at_ms != 0;
        self.failures = 0;
        self.retry_at_ms = None;
        self.paused = false;
        self.next_at_ms = 0;
    }
    pub fn recover(&mut self) {
        for r in &mut self.records {
            if matches!(r.status, Status::Processing) {
                r.status = Status::Queued;
                self.dirty = true;
            }
        }
    }
    pub fn forget(&mut self, source: &Source) {
        let old = self.records.len();
        self.records.retain(|r| &r.candidate.source != source);
        self.dirty |= old != self.records.len();
    }
    pub fn write(&self, tx: &Transaction<'_>) -> Result<(), String> {
        if !self.dirty || self.blocked {
            return Ok(());
        }
        if !self.fits() {
            return Err("整理缓存超过上限".into());
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS organizer_state(id INTEGER PRIMARY KEY CHECK(id=1),data TEXT NOT NULL)").map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO organizer_state(id,data) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data",[serde_json::to_string(self).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn committed(&mut self) {
        self.dirty = false;
    }
}
fn tags_bytes(tags: &Vec<String>) -> usize {
    tags.capacity() * std::mem::size_of::<String>()
        + tags.iter().map(String::capacity).sum::<usize>()
}
fn candidate_bytes(c: &Candidate) -> usize {
    let source = match &c.source {
        Source::SavedItem { id } => id,
        Source::ObservedPath { normalized_path } => normalized_path,
    };
    source.capacity()
        + c.fingerprint.capacity()
        + c.expected_version.capacity()
        + c.title.capacity()
        + c.alias.capacity()
        + tags_bytes(&c.tags)
        + c.context.capacity()
}
