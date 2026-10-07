//! API replies never carry error text into the ordinary search view.
use crate::{
    ai::AiPlan,
    model::{Command, ItemSummary, Scope},
    semantic_ui::{IndexSession, QuerySession},
};

#[derive(Debug, PartialEq, Eq)]
pub enum QueryReply {
    Applied,
    LocalFallback,
    Ignored,
}
pub enum IndexReply {
    Store(Box<Command>),
    LocalFallback,
    Ignored,
}

pub fn claude_reply(
    result: Result<AiPlan, String>,
    adopted: &mut Option<AiPlan>,
    saved: &mut Vec<ItemSummary>,
    ready: &mut bool,
) -> QueryReply {
    match result {
        Ok(plan) => {
            *adopted = Some(plan);
            saved.clear();
            *ready = false;
            QueryReply::Applied
        }
        Err(_) => QueryReply::LocalFallback,
    }
}
pub fn embedding_reply(
    result: Result<Vec<Vec<f32>>, String>,
    session: &mut QuerySession,
    id: u64,
    identity: (&str, Scope, &str, bool),
) -> QueryReply {
    let (query, scope, profile, active) = identity;
    if !session.current(id, query, scope, profile, active) {
        return QueryReply::Ignored;
    }
    if let Ok(mut vectors) = result
        && vectors.len() == 1
        && session.finish(id, query, scope, profile, active, vectors.remove(0))
    {
        return QueryReply::Applied;
    }
    // End only the network session. The ordinary result token and selection
    // remain usable, including a local response still queued in the engine.
    session.cancel();
    QueryReply::LocalFallback
}
pub fn index_reply(
    result: Result<Vec<Vec<f32>>, String>,
    session: &mut IndexSession,
    id: u64,
    profile: &str,
) -> IndexReply {
    if !session.network_current(id, profile) {
        return IndexReply::Ignored;
    }
    if let Ok(vectors) = result
        && let Some(command) = session.network_result(id, profile, vectors)
    {
        return IndexReply::Store(Box::new(command));
    }
    session.fail();
    IndexReply::LocalFallback
}
