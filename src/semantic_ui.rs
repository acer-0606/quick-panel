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

#[derive(Default)]
pub struct IndexSession {
    serial: u64,
    profile: Option<String>,
    pending: Option<IndexPending>,
    accepted: usize,
}
enum IndexPending {
    Page(u64),
    Network { batch: IndexBatch, job: Option<u64> },
    Store(IndexBatch),
}
impl IndexSession {
    pub fn active(&self) -> bool {
        self.profile.is_some()
    }
    pub fn accepted(&self) -> usize {
        self.accepted
    }
    pub fn cancel(&mut self) {
        self.profile = None;
        self.pending = None;
    }
    pub fn begin(&mut self, profile: &str) -> Command {
        self.cancel();
        self.profile = Some(profile.into());
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
            self.cancel();
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
            self.cancel();
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
