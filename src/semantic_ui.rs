//! Portable gates for explicitly started query and sequential indexing sessions.
use crate::{
    model::{Command, Scope},
    semantic::{IndexBatch, IndexCursor, IndexedEmbedding, SemanticSearchQuery},
    ui_state::UiState,
};

#[derive(Default)]
pub struct QuerySession {
    pending: Option<(u64, String, Scope, String)>,
    adopted: Option<SemanticSearchQuery>,
}
impl QuerySession {
    pub fn begin(&mut self, id: u64, query: &str, scope: Scope, profile: &str) {
        self.adopted = None;
        self.pending = Some((id, query.into(), scope, profile.into()));
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn adopted(&self) -> bool {
        self.adopted.is_some()
    }
    pub fn cancel(&mut self) {
        self.pending = None;
        self.adopted = None;
    }
    pub fn finish(
        &mut self,
        id: u64,
        query: &str,
        scope: Scope,
        profile: &str,
        active: bool,
        vector: Vec<f32>,
    ) -> bool {
        if !active
            || self
                .pending
                .as_ref()
                .is_none_or(|p| p.0 != id || p.1 != query || p.2 != scope || p.3 != profile)
        {
            return false;
        }
        self.pending = None;
        if crate::semantic::validate_vectors(std::slice::from_ref(&vector)).is_err() {
            return false;
        }
        self.adopted = Some(SemanticSearchQuery {
            text: query.into(),
            scope,
            request_id: 0,
            profile: profile.into(),
            vector,
        });
        true
    }
    pub fn current(&self, id: u64, query: &str, scope: Scope, profile: &str, active: bool) -> bool {
        active
            && self
                .pending
                .as_ref()
                .is_some_and(|p| p.0 == id && p.1 == query && p.2 == scope && p.3 == profile)
    }
    pub fn local_query(&self, request_id: u64) -> Option<SemanticSearchQuery> {
        self.adopted.clone().map(|mut query| {
            query.request_id = request_id;
            query
        })
    }
}
/// Revokes local retrieval too, including a result queued after HTTP completion.
pub fn cancel_semantic_view<T>(
    session: &mut QuerySession,
    state: &mut UiState,
    saved: &mut Vec<T>,
    ready: &mut bool,
) {
    session.cancel();
    state.next_request();
    state.replace_ids(Vec::new());
    saved.clear();
    *ready = false;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndexOutcome {
    #[default]
    Idle,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Default)]
pub struct IndexSession {
    serial: u64,
    profile: Option<String>,
    pending: Option<IndexPending>,
    accepted: usize,
    outcome: IndexOutcome,
    outcome_profile: String,
    refresh_needed: bool,
}
enum IndexPending {
    Page(u64),
    Network { batch: IndexBatch, job: Option<u64> },
    Store(IndexBatch),
}
impl IndexSession {
    pub fn outcome(&self) -> IndexOutcome {
        self.outcome
    }
    pub fn needs_content_guidance(
        &self,
        status: Option<&crate::model::SemanticIndexStatus>,
    ) -> bool {
        self.outcome == IndexOutcome::Completed
            && status.is_some_and(|s| s.profile == self.outcome_profile && s.indexed == 0)
    }

    pub fn clear_display(&mut self) {
        self.outcome = IndexOutcome::Idle;
        self.outcome_profile.clear();
        self.refresh_needed = false;
    }
    pub fn take_status_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh_needed)
    }
    fn stop(&mut self, outcome: IndexOutcome) {
        if let Some(profile) = self.profile.take() {
            self.outcome_profile = profile;
            self.outcome = outcome;
            self.refresh_needed = true;
        }
        self.pending = None;
    }
    pub fn fail(&mut self) {
        self.stop(IndexOutcome::Failed);
    }
    pub fn label(&self, status: Option<&crate::model::SemanticIndexStatus>) -> String {
        if self.active() {
            return format!("索引更新中 · 已接收 {} 条 · 点击取消", self.accepted);
        }
        let outcome = if status.is_some_and(|s| s.profile != self.outcome_profile) {
            IndexOutcome::Idle
        } else {
            self.outcome
        };
        let prefix = match outcome {
            IndexOutcome::Completed => "索引已完成",
            IndexOutcome::Cancelled => "索引已取消",
            IndexOutcome::Failed => "索引已停止",
            _ => "索引",
        };
        if let Some(status) = status {
            if status.indexed == 0 && outcome == IndexOutcome::Completed {
                return if status.needs_update {
                    "本次未生成可用索引 · 内容待更新 · 点击检查语义检索设置"
                } else {
                    "当前范围没有可索引内容 · 固定文本/添加收藏，或点击调整索引范围"
                }
                .into();
            }
            if matches!(outcome, IndexOutcome::Cancelled | IndexOutcome::Failed) {
                return format!(
                    "{prefix} · 保留可用 {} 条 · {}",
                    status.indexed,
                    if outcome == IndexOutcome::Failed {
                        "点击检查语义检索设置"
                    } else {
                        "继续更新索引"
                    },
                );
            }
            if status.indexed == 0 {
                return "暂无可用索引 · 固定文本/添加收藏后建立，或调整索引范围".into();
            }
            let prefix = if outcome == IndexOutcome::Completed && !status.needs_update {
                "索引已是最新"
            } else {
                prefix
            };
            return format!(
                "{prefix} · 可用 {} 条{} · 更新索引",
                status.indexed,
                if status.needs_update {
                    " · 待更新"
                } else {
                    ""
                }
            );
        }
        if outcome == IndexOutcome::Idle {
            "索引状态待刷新 · 建立/更新索引".into()
        } else {
            format!(
                "{prefix} · 本次已接收 {} 条 · 可用数量待刷新",
                self.accepted
            )
        }
    }
    pub fn active(&self) -> bool {
        self.profile.is_some()
    }
    pub fn accepted(&self) -> usize {
        self.accepted
    }
    pub fn cancel(&mut self) {
        self.stop(IndexOutcome::Cancelled);
    }
    pub fn begin(&mut self, profile: &str) -> Command {
        self.cancel();
        self.profile = Some(profile.into());
        self.outcome_profile = profile.into();
        self.outcome = IndexOutcome::Running;
        self.refresh_needed = false;
        self.accepted = 0;
        self.page(IndexCursor::default())
    }
    fn page(&mut self, cursor: IndexCursor) -> Command {
        self.serial = self.serial.wrapping_add(1);
        self.pending = Some(IndexPending::Page(self.serial));
        Command::PrepareSemanticIndex {
            request_id: self.serial,
            cursor,
        }
    }
    pub fn accept_batch(&mut self, batch: IndexBatch) -> Option<Vec<String>> {
        if self.profile.as_deref() != Some(batch.profile.as_str())
            || !matches!(self.pending, Some(IndexPending::Page(id)) if id == batch.request_id)
        {
            return None;
        }
        let inputs = batch.candidates.iter().map(|c| c.text.clone()).collect();
        self.pending = Some(IndexPending::Network { batch, job: None });
        Some(inputs)
    }
    pub fn network_started(&mut self, id: u64) {
        if let Some(IndexPending::Network { job, .. }) = &mut self.pending {
            *job = Some(id);
        }
    }
    pub fn network_current(&self, id: u64, profile: &str) -> bool {
        self.profile.as_deref() == Some(profile)
            && matches!(&self.pending, Some(IndexPending::Network { job: Some(job), .. }) if *job == id)
    }
    pub fn network_result(
        &mut self,
        id: u64,
        profile: &str,
        vectors: Vec<Vec<f32>>,
    ) -> Option<Command> {
        if !self.network_current(id, profile) {
            return None;
        }
        let Some(IndexPending::Network { batch, .. }) = &self.pending else {
            return None;
        };
        if vectors.len() != batch.candidates.len()
            || crate::semantic::validate_vectors(&vectors).is_err()
        {
            self.fail();
            return None;
        }
        let items = batch
            .candidates
            .iter()
            .zip(vectors)
            .map(|(candidate, vector)| IndexedEmbedding {
                id: candidate.id.clone(),
                text_hash: candidate.text_hash.clone(),
                vector,
            })
            .collect();
        let command = Command::StoreEmbeddings {
            request_id: batch.request_id,
            profile: profile.into(),
            items,
        };
        let batch = batch.clone();
        self.pending = Some(IndexPending::Store(batch));
        Some(command)
    }
    fn advance(&mut self, batch: IndexBatch) -> Option<Command> {
        if batch.done {
            self.stop(IndexOutcome::Completed);
            None
        } else {
            Some(self.page(batch.cursor))
        }
    }
    pub fn empty_page(&mut self) -> Option<Command> {
        if !matches!(&self.pending, Some(IndexPending::Network { batch, job: None }) if batch.candidates.is_empty())
        {
            return None;
        }
        let Some(IndexPending::Network { batch, .. }) = self.pending.take() else {
            return None;
        };
        self.advance(batch)
    }
    pub fn acknowledge(&mut self, id: u64, accepted: usize) -> Option<Command> {
        if !matches!(&self.pending, Some(IndexPending::Store(batch)) if batch.request_id == id) {
            return None;
        }
        let Some(IndexPending::Store(batch)) = self.pending.take() else {
            return None;
        };
        self.accepted += accepted;
        self.advance(batch)
    }
}

/// Reconcile view identity before accepting or displaying derived rows.
pub fn sync_semantic_view<T>(
    session: &mut QuerySession,
    state: &mut UiState,
    saved: &mut Vec<T>,
    ready: &mut bool,
    view: (&str, Scope, &str, bool),
) -> bool {
    let (query, scope, profile, active) = view;
    let pending_changed = session
        .pending
        .as_ref()
        .is_some_and(|p| !active || p.1 != query || p.2 != scope || p.3 != profile);
    let adopted_changed = session
        .adopted
        .as_ref()
        .is_some_and(|p| !active || p.text != query || p.scope != scope || p.profile != profile);
    if pending_changed || adopted_changed {
        cancel_semantic_view(session, state, saved, ready);
        true
    } else {
        false
    }
}

/// Menus preserve their captured target and derived rows; other effects revoke them.
/// The native caller cancels the worker before applying this gate.
pub fn prepare_semantic_effect<T>(
    session: &mut QuerySession,
    state: &mut UiState,
    saved: &mut Vec<T>,
    ready: &mut bool,
    preserve_results: bool,
) -> bool {
    if preserve_results {
        session.pending = None;
        return false;
    }
    let active = session.busy() || session.adopted();
    if active {
        cancel_semantic_view(session, state, saved, ready);
    }
    active
}

/// Resume ordinary retrieval after a modal return while the panel is still active.
pub fn resume_ordinary_query<T>(
    session: &mut QuerySession,
    state: &mut UiState,
    saved: &mut Vec<T>,
    ready: &mut bool,
    text: &str,
    scope: Scope,
) -> Option<crate::model::SearchQuery> {
    state.modal_open = false;
    if !state.visible || state.lifecycle != crate::ui_state::Lifecycle::Running {
        return None;
    }
    session.cancel();
    saved.clear();
    *ready = false;
    state.replace_ids(Vec::new());
    Some(crate::model::SearchQuery {
        text: text.into(),
        scope,
        request_id: state.next_request(),
    })
}

/// Pair a modal's semantic revocation with ordinary retrieval on every return path.
/// A newly launched semantic query, a menu, hiding, or shutdown needs no restoration.
pub fn finish_semantic_effect<T>(
    session: &mut QuerySession,
    state: &mut UiState,
    saved: &mut Vec<T>,
    ready: &mut bool,
    text: &str,
    scope: Scope,
    revoked: bool,
) -> Option<crate::model::SearchQuery> {
    finish_semantic_effect_with_revoke(
        session,
        state,
        saved,
        ready,
        (text, scope, revoked),
        |_, _, _| {},
    )
}

/// Complete the same modal contract while revoking any other adopted local query mode.
pub fn finish_semantic_effect_with_revoke<T>(
    session: &mut QuerySession,
    state: &mut UiState,
    saved: &mut Vec<T>,
    ready: &mut bool,
    view: (&str, Scope, bool),
    revoke: impl FnOnce(&mut UiState, &mut Vec<T>, &mut bool),
) -> Option<crate::model::SearchQuery> {
    let (text, scope, revoked) = view;
    state.modal_open = false;
    if !revoked
        || session.busy()
        || session.adopted()
        || !state.visible
        || state.lifecycle != crate::ui_state::Lifecycle::Running
    {
        return None;
    }
    // Other adopted modes may invalidate the request token when revoked.
    // Allocate the final ordinary token only after those invalidations complete.
    revoke(state, saved, ready);
    resume_ordinary_query(session, state, saved, ready, text, scope)
}
