//! Optional bounded Pi state. Writes share the repository's source transaction.
use crate::{
    harness_tools::*,
    organizer::{Candidate, Proposal},
    pi_harness::{Checkpoint, HostResult, ToolError},
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_MEMORIES: usize = 256;
pub const MAX_CONFIRMED_RULES: usize = 64;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Run {
    pub profile: String,
    pub memory_version: u64,
    pub candidates: Vec<Candidate>,
    pub checkpoint: Option<Checkpoint>,
    pub staged: Vec<Option<Proposal>>,
    pub finished: bool,
    pub receipts: Vec<(String, String, HostResult<Value>)>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct HarnessStore {
    pub version: u64,
    pub next_id: u64,
    pub memories: Vec<MemoryRecord>,
    pub run: Option<Run>,
    pub human_requests: Vec<(u64, String)>,
    #[serde(skip)]
    pub blocked: bool,
    #[serde(skip)]
    pub dirty: bool,
}
impl HarnessStore {
    pub fn load(db: &Connection) -> Self {
        Self::load_checked(db).unwrap_or_else(|_| Self {
            blocked: true,
            ..Self::default()
        })
    }
    fn load_checked(db: &Connection) -> Result<Self, String> {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='harness_state')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Ok(Self::default());
        }
        let too_big: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM harness_state WHERE length(CAST(data AS BLOB))>?1)",
                [MAX_BYTES as i64],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if too_big {
            return Err("Pi 状态超过容量".into());
        }
        let json: Option<String> = db
            .query_row(
                "SELECT data FROM harness_state WHERE id=1 AND length(CAST(data AS BLOB))<=?1",
                [MAX_BYTES as i64],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(json) = json else {
            return Ok(Self::default());
        };
        let state: Self = serde_json::from_str(&json).map_err(|_| "Pi 状态损坏")?;
        state.validate()?;
        Ok(state)
    }
    pub fn fits(&self) -> bool {
        let encoded = serde_json::to_vec(self).map_or(MAX_BYTES + 1, |v| v.len());
        self.memories.len() <= MAX_MEMORIES
            && self.memories.iter().filter(|m| authoritative(m)).count() <= MAX_CONFIRMED_RULES
            && encoded <= MAX_BYTES
            && encoded
                .saturating_mul(2)
                .saturating_add(self.memories.capacity() * std::mem::size_of::<MemoryRecord>())
                .saturating_add(
                    self.human_requests.capacity() * std::mem::size_of::<(u64, String)>(),
                )
                <= MAX_BYTES
    }
    fn validate(&self) -> Result<(), String> {
        if !self.fits() {
            return Err("Pi 状态超过容量".into());
        }
        let mut ids = std::collections::HashSet::new();
        for m in &self.memories {
            if m.id == 0
                || m.id > self.next_id
                || !ids.insert(m.id)
                || validate_scope(&m.scope).is_err()
                || !text_valid(&m.text, 512, false)
                || m.alias.as_ref().is_some_and(|a| {
                    !text_valid(
                        a,
                        if matches!(
                            m.kind,
                            MemoryKind::Accepted | MemoryKind::Edited | MemoryKind::AiApplied
                        ) {
                            256
                        } else {
                            48
                        },
                        false,
                    )
                })
                || m.tags.len() > 32
                || m.tags.iter().any(|s| !text_valid(s, 64, false))
                || m.evidence_ids.len() > 8
                || m.evidence_ids.iter().any(|s| !text_valid(s, 128, false))
                || (m.confirmed && m.origin != MemoryOrigin::Human)
            {
                return Err("记忆记录无效".into());
            }
        }
        let unique_requests: std::collections::HashSet<_> =
            self.human_requests.iter().map(|r| r.0).collect();
        if self.human_requests.len() > 32
            || unique_requests.len() != self.human_requests.len()
            || self.human_requests.iter().any(|r| !digest(&r.1))
        {
            return Err("反馈收据超限".into());
        }
        if let Some(r) = &self.run {
            if !digest(&r.profile)
                || r.candidates.is_empty()
                || r.candidates.len() > 8
                || r.staged.len() != r.candidates.len()
                || r.receipts.len() > 32
                || r.memory_version != self.version
            {
                return Err("Pi 任务绑定无效".into());
            }
            let mut sources = std::collections::HashSet::new();
            for (i, c) in r.candidates.iter().enumerate() {
                crate::organizer::validate_candidate(c)?;
                if !sources.insert(c.source.clone()) {
                    return Err("Pi 任务来源重复".into());
                }
                if let Some(p) = &r.staged[i] {
                    p.validate()?;
                    if p.candidate != *c {
                        return Err("Pi 暂存来源无效".into());
                    }
                }
            }
            if r.finished && r.staged.iter().any(Option::is_none) {
                return Err("Pi 完成覆盖无效".into());
            }
            if let Some(cp) = &r.checkpoint {
                cp.validate().map_err(|e| e.to_string())?;
            }
            let mut calls = std::collections::HashSet::new();
            for (call, signature, result) in &r.receipts {
                if !digest(signature)
                    || !valid_receipt(result, r.candidates.len())
                    || !text_valid(call, 160, false)
                    || !calls.insert(call)
                    || serde_json::to_vec(result)
                        .map_or(true, |b| b.len() > crate::pi_harness::MAX_TOOL_RESULT_BYTES)
                {
                    return Err("Pi 工具收据无效".into());
                }
            }
        }
        Ok(())
    }
    pub fn clear_run(&mut self) {
        if !self.blocked && self.run.take().is_some() {
            self.dirty = true;
        }
    }
    pub fn begin(&mut self, profile: String, candidates: Vec<Candidate>) -> Result<(), ToolError> {
        if self.blocked {
            return Err(error("STORAGE", "Pi 状态损坏或超限，已暂停", false));
        }
        if self.run.as_ref().is_some_and(|r| {
            r.profile == profile && r.memory_version == self.version && r.candidates == candidates
        }) {
            return Ok(());
        }
        let mut next = self.clone();
        next.run = Some(Run {
            profile,
            memory_version: self.version,
            staged: vec![None; candidates.len()],
            candidates,
            checkpoint: None,
            finished: false,
            receipts: vec![],
        });
        while !next.fits() {
            let Some(index) = next.memories.iter().position(|m| !authoritative(m)) else {
                return Err(error("CAPACITY", "Pi 状态容量已满", false));
            };
            next.memories.remove(index);
            next.memories.shrink_to_fit();
        }
        next.dirty = true;
        *self = next;
        Ok(())
    }
    pub fn checkpoint(&mut self, cp: Checkpoint) -> Result<(), ToolError> {
        cp.validate()
            .map_err(|e| error("checkpoint_limit", e.to_string(), false))?;
        let r = self
            .run
            .as_ref()
            .ok_or_else(|| error("STALE_RUN", "整理任务已撤销", false))?;
        if r.checkpoint.as_ref().is_some_and(|old| {
            cp.rounds < old.rounds || cp.calls < old.calls || cp.elapsed_ms < old.elapsed_ms
        }) {
            return Err(error("protocol", "检查点预算倒退", false));
        }
        let mut next = self.clone();
        next.run.as_mut().unwrap().checkpoint = Some(cp);
        if !next.fits() {
            return Err(error("CAPACITY", "Pi 状态容量已满", false));
        }
        next.dirty = true;
        *self = next;
        Ok(())
    }
    pub fn stage(&mut self, index: usize, proposal: Proposal) -> Result<(), ToolError> {
        let mut next = self.clone();
        let r = next
            .run
            .as_mut()
            .ok_or_else(|| error("STALE_RUN", "整理任务已撤销", false))?;
        if r.finished {
            return Err(error("STALE_RUN", "任务已经完成暂存", false));
        }
        let slot = r
            .staged
            .get_mut(index)
            .ok_or_else(|| error("INVALID_ARGUMENTS", "索引不属于当前批次", true))?;
        *slot = Some(proposal);
        if !next.fits() {
            return Err(error("CAPACITY", "Pi 暂存容量已满", false));
        }
        next.dirty = true;
        *self = next;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<Vec<Proposal>, String> {
        let r = self.run.as_mut().ok_or("整理任务已撤销")?;
        let proposals = r
            .staged
            .iter()
            .cloned()
            .collect::<Option<Vec<_>>>()
            .ok_or("整理任务尚未覆盖全部来源")?;
        if !r.finished {
            r.finished = true;
            self.dirty = true;
        }
        Ok(proposals)
    }
    pub fn receipt(
        &self,
        call: &str,
        signature: &str,
    ) -> Result<Option<HostResult<Value>>, ToolError> {
        let Some(prior) = self
            .run
            .as_ref()
            .and_then(|r| r.receipts.iter().find(|r| r.0 == call))
        else {
            return Ok(None);
        };
        if prior.1 != signature {
            return Err(error("protocol", "调用 ID 被用于不同参数", false));
        }
        Ok(Some(prior.2.clone()))
    }
    pub fn record_receipt(
        &mut self,
        call: String,
        signature: String,
        result: HostResult<Value>,
    ) -> Result<(), ToolError> {
        let mut next = self.clone();
        let r = next
            .run
            .as_mut()
            .ok_or_else(|| error("STALE_RUN", "整理任务已撤销", false))?;
        if r.receipts.len() >= 32 {
            return Err(error("call_limit", "工具调用预算耗尽", false));
        }
        r.receipts.push((call, signature, result));
        if !next.fits() {
            return Err(error("CAPACITY", "Pi 工具收据容量已满", false));
        }
        next.dirty = true;
        *self = next;
        Ok(())
    }
    pub fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            version: self.version,
            records: self.memories.clone(),
            storage_error: self
                .blocked
                .then(|| "Pi 记忆或任务状态损坏/超限，已暂停；原始历史可继续使用".into()),
        }
    }
    pub fn human_seen(&self, id: u64, signature: &str) -> Result<bool, String> {
        if self.blocked {
            return Err("Pi 状态损坏或超限，编辑尚未保存".into());
        }
        match self
            .human_requests
            .iter()
            .find(|(request, _)| *request == id)
        {
            Some((_, prior)) if prior == signature => Ok(true),
            Some(_) => Err("请求 ID 已用于其他记忆操作".into()),
            None => Ok(false),
        }
    }
    pub fn human_request(&mut self, id: u64, signature: String) {
        if self.human_requests.len() == 32 {
            self.human_requests.remove(0);
        }
        self.human_requests.push((id, signature));
        self.dirty = true;
    }
    pub fn insert(&mut self, mut record: MemoryRecord) -> Result<u64, ToolError> {
        if self.blocked {
            return Err(error("STORAGE", "Pi 状态损坏或超限", false));
        }
        let mut next = self.clone();
        next.next_id = next
            .next_id
            .checked_add(1)
            .ok_or_else(|| error("CAPACITY", "记忆编号耗尽", false))?;
        record.id = next.next_id;
        let id = record.id;
        next.memories.push(record);
        while !next.fits() {
            let Some(i) = next
                .memories
                .iter()
                .position(|m| !authoritative(m) && m.id != id)
            else {
                return Err(error(
                    "CAPACITY",
                    "记忆容量已满，请编辑或删除已确认规则",
                    false,
                ));
            };
            next.memories.remove(i);
            next.memories.shrink_to_fit();
        }
        next.dirty = true;
        *self = next;
        Ok(id)
    }
    pub fn edit(&mut self, input: MemoryEdit, now: i64) -> Result<(), String> {
        validate_scope(&input.scope).map_err(|e| e.message)?;
        if !text_valid(&input.text, 512, false) {
            return Err("规则须为 1–512 字符且不含控制字符".into());
        }
        let mut next = self.clone();
        if let Some(id) = input.id {
            let m = next
                .memories
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or("记忆记录不存在")?;
            m.scope = input.scope;
            m.text = input.text;
            m.enabled = input.enabled;
            m.confirmed = input.confirmed;
            m.origin = MemoryOrigin::Human;

            if !next.fits() {
                return Err("记忆容量已满，请编辑或删除已确认规则".into());
            }
        } else {
            next.insert(MemoryRecord {
                id: 0,
                scope: input.scope,
                text: input.text,
                enabled: input.enabled,
                confirmed: input.confirmed,
                origin: MemoryOrigin::Human,
                proposed_by_ai: false,
                kind: MemoryKind::Rule,
                alias: None,
                tags: vec![],
                evidence_ids: vec![],
                created_at_ms: now,
            })
            .map_err(|e| e.message)?;
        }
        next.version = next.version.checked_add(1).ok_or("记忆版本耗尽")?;
        next.run = None;
        next.dirty = true;
        *self = next;
        Ok(())
    }
    pub fn delete(&mut self, ids: &[u64]) -> Result<(), String> {
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        if ids.is_empty()
            || ids.len() > MAX_MEMORIES
            || unique.len() != ids.len()
            || ids
                .iter()
                .any(|id| !self.memories.iter().any(|m| m.id == *id))
        {
            return Err("记忆选择无效或已过期".into());
        }
        self.memories.retain(|m| !ids.contains(&m.id));
        self.memories.shrink_to_fit();
        self.version = self.version.checked_add(1).ok_or("记忆版本耗尽")?;
        self.clear_run();
        self.dirty = true;
        Ok(())
    }
    pub fn write(&self, tx: &Transaction<'_>) -> Result<(), String> {
        if !self.dirty || self.blocked {
            return Ok(());
        }
        self.validate()?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS harness_state(id INTEGER PRIMARY KEY CHECK(id=1),data TEXT NOT NULL)").map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO harness_state(id,data) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data",[serde_json::to_string(self).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn committed(&mut self) {
        self.dirty = false;
    }
}

fn authoritative(m: &MemoryRecord) -> bool {
    m.confirmed && (m.kind == MemoryKind::Rule || !matches!(m.scope, MemoryScope::Item { .. }))
}

fn digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_receipt(result: &HostResult<Value>, batch: usize) -> bool {
    if !result.ok {
        return result.error.as_ref().is_some_and(|e| e.validate().is_ok());
    }
    let Some(v) = &result.value else {
        return false;
    };
    let Some(map) = v.as_object() else {
        return false;
    };
    (map.len() == 2
        && v["accepted"] == true
        && v["index"].as_u64().is_some_and(|i| i < batch as u64))
        || (map.len() == 3
            && v["proposed"] == true
            && v["confirmed"] == false
            && v["id"].as_str().is_some_and(|s| text_valid(s, 128, false)))
        || (map.len() == 2 && v["proposed"] == false && v["memory_enabled"] == false)
}
