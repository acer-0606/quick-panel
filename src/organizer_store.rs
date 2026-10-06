//! Bounded durable organizer state, committed with source metadata by Repository.
use crate::organizer::*;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
// Inline proposals keep the existing serde/memory accounting contract; Record
// capacity includes this payload, so boxing here would require a budget redesign.
#[allow(clippy::large_enum_variant)]
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
    // Runtime headroom for the actor Work, one delivered worker Work, and the
    // eventual Ready growth. Every enqueue observes these reservations too.
    #[serde(skip)]
    resident_reservation: usize,
    #[serde(skip)]
    serialized_reservation: usize,
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
    /// Pure in-memory projection: no candidate/context clone, reconciliation or dirty flag.
    pub fn progress(&self, enabled: bool, now_ms: i64) -> crate::organizer_display::Progress {
        use crate::organizer_display::{Progress, Stage};
        if !enabled {
            return Progress::default();
        }
        Progress {
            items: self
                .records
                .iter()
                .filter_map(|r| {
                    let stage = match r.status {
                        Status::Queued => Stage::Waiting,
                        Status::Processing => Stage::Processing,
                        Status::Ready(_) => Stage::Ready,
                        Status::Done => return None,
                    };
                    Some((r.candidate.source.clone(), stage))
                })
                .collect(),
            paused: self.paused,
            cooling: self.retry_at_ms.is_some_and(|deadline| deadline > now_ms)
                && !self
                    .records
                    .iter()
                    .any(|r| matches!(r.status, Status::Processing)),
            saturated: self.saturated || self.ready_count() >= MAX_READY_PROPOSALS,
            storage_error: self.blocked,
        }
    }
    pub fn snapshot(&self, in_flight: bool, now_ms: i64) -> Snapshot {
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
            retry_at_ms: self
                .retry_at_ms
                .filter(|deadline| *deadline > now_ms && !in_flight),
            paused: self.paused,
            saturated: self.saturated || self.ready_count() >= MAX_READY_PROPOSALS,
        }
    }
    fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
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
                .sum::<usize>()
    }
    fn serialized_bytes(&self) -> usize {
        serde_json::to_vec(self).map_or(MAX_STORE_BYTES + 1, |b| b.len())
    }
    pub fn fits(&self) -> bool {
        self.records.len() <= MAX_RECORDS
            && self.ready_count() <= MAX_READY_PROPOSALS
            && self
                .memory_bytes()
                .saturating_add(self.resident_reservation)
                <= MAX_STORE_BYTES
            && self
                .serialized_bytes()
                .saturating_add(self.serialized_reservation)
                <= MAX_STORE_BYTES
    }
    fn evict_oldest_done(&mut self) -> bool {
        let Some(index) = self
            .records
            .iter()
            .position(|r| matches!(r.status, Status::Done))
        else {
            return false;
        };
        self.records.remove(index);
        self.records.shrink_to_fit();
        self.dirty = true;
        true
    }
    pub fn release_reservation(&mut self) {
        self.resident_reservation = 0;
        self.serialized_reservation = 0;
    }
    /// Called on a prospective store, committed by the actor only on success.
    /// Reclaim completed receipts for BOTH live and future-result headroom.
    pub fn reserve_work(&mut self, work: &Work) -> bool {
        self.records.shrink_to_fit();
        for record in &mut self.records {
            if work.candidates.contains(&record.candidate) {
                record.status = Status::Processing;
            }
        }
        loop {
            let mut future = self.clone();
            future.release_reservation();
            if work.organizer_settings.mode == OrganizerMode::Suggest {
                for record in &mut future.records {
                    if work.candidates.contains(&record.candidate) {
                        record.status = Status::Ready(Proposal {
                            candidate: record.candidate.clone(),
                            alias: Some("𐐀".repeat(MAX_ALIAS_CHARS)),
                            tags: (0..MAX_TAGS)
                                .map(|i| format!("{i}{}", "𐐀".repeat(MAX_TAG_CHARS - 1)))
                                .collect(),
                            reason: "𐐀".repeat(MAX_REASON_CHARS),
                        });
                    }
                }
            }
            // These are alternative long-lived states, not simultaneous: the
            // Work reservation is released before converting Processing to Ready.
            self.resident_reservation = work_bytes(work)
                .saturating_mul(2)
                .max(future.memory_bytes().saturating_sub(self.memory_bytes()));
            self.serialized_reservation = future
                .serialized_bytes()
                .saturating_sub(self.serialized_bytes());
            if self.fits() && future.ready_count() <= MAX_READY_PROPOSALS {
                self.saturated = false;
                return true;
            }
            if !self.evict_oldest_done() {
                return false;
            }
        }
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
            if !next.evict_oldest_done() {
                let changed = !self.saturated;
                self.saturated = true;
                self.dirty |= changed;
                return changed;
            }
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
        self.capacity_released();
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
        if old != self.records.len() {
            self.records.shrink_to_fit();
            self.capacity_released();
        }
    }
    pub fn capacity_released(&mut self) {
        if self.blocked {
            return;
        }
        let saturated = self.ready_count() >= MAX_READY_PROPOSALS
            || (self.records.len() >= MAX_RECORDS
                && !self
                    .records
                    .iter()
                    .any(|r| matches!(r.status, Status::Done)))
            || !self.fits();
        self.dirty |= self.saturated != saturated;
        self.saturated = saturated;
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
fn work_bytes(work: &Work) -> usize {
    std::mem::size_of::<Work>()
        + work.candidates.capacity() * std::mem::size_of::<Candidate>()
        + work.candidates.iter().map(candidate_bytes).sum::<usize>()
        + work.ai_settings.endpoint.capacity()
        + work.ai_settings.model.capacity()
        + work.ai_settings.encrypted_key.capacity()
        + work.organizer_settings.instructions.capacity()
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_cooldown_does_not_hide_an_automatic_retry() {
        use crate::organizer_display::Stage;
        let mut store = OrganizerStore {
            retry_at_ms: Some(100),
            failures: 1,
            ..Default::default()
        };
        assert!(store.progress(true, 99).cooling);
        assert!(store.snapshot(false, 99).retry_at_ms.is_some());
        assert!(!store.progress(true, 100).cooling);
        assert!(store.snapshot(false, 100).retry_at_ms.is_none());
        store.records.push(Record {
            candidate: Candidate {
                source: Source::SavedItem { id: "retry".into() },
                fingerprint: "f".into(),
                expected_version: "v".into(),
                kind: crate::model::ItemKind::Folder,
                title: "logs".into(),
                alias: String::new(),
                tags: vec![],
                context: "C:/logs".into(),
            },
            status: Status::Processing,
        });
        let p = store.progress(true, 101);
        assert_eq!(p.items[0].1, Stage::Processing);
        assert!(p.label().contains("整理中") && !p.label().contains("冷却"));
        assert!(store.snapshot(true, 101).retry_at_ms.is_none());
        assert_eq!(store.failures, 1);
        assert_eq!(store.retry_at_ms, Some(100));
        assert!(
            !store.dirty,
            "reading display state must not schedule a save"
        );
    }
    #[test]
    fn freed_ready_capacity_clears_saturation_without_requeueing_sources() {
        let candidate = Candidate {
            source: Source::SavedItem { id: "one".into() },
            fingerprint: "fingerprint".into(),
            expected_version: "version".into(),
            kind: crate::model::ItemKind::Clipboard,
            title: "标题".into(),
            alias: String::new(),
            tags: vec![],
            context: "摘要".into(),
        };
        let mut store = OrganizerStore {
            saturated: true,
            ..Default::default()
        };
        store.records.push(Record {
            candidate: candidate.clone(),
            status: Status::Ready(Proposal {
                candidate,
                alias: None,
                tags: vec![],
                reason: "依据".into(),
            }),
        });
        store.forget(&Source::SavedItem { id: "one".into() });
        assert!(!store.snapshot(false, 0).saturated);
        assert!(store.records.is_empty());
        store.saturated = true;
        store.reset();
        assert!(!store.snapshot(false, 0).saturated);
    }
}
