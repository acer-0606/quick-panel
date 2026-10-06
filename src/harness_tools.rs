//! Actor-facing Pi context and trusted human memory contracts. No transport or I/O.
use crate::{
    organizer::Source,
    pi_harness::{Checkpoint, ItemRef, PublicEffort, ToolError},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FEEDBACK_REASON_MAX_CHARS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiContext {
    pub model: String,
    pub effort: PublicEffort,
    pub instructions: String,
    pub item_refs: Vec<ItemRef>,
    pub max_rounds: u32,
    pub checkpoint: Option<Checkpoint>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryScope {
    Item { source: Source },
    Directory { path: String },
    Global,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeedbackTarget {
    pub source: Source,
    pub expected_version: String,
    pub rejected_alias: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum FeedbackScope {
    Item,
    Directory,
    Global,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Rule,
    Accepted,
    Edited,
    Rejected,
    AiApplied,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrigin {
    Human,
    Ai,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord {
    pub id: u64,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub origin: MemoryOrigin,
    /// Keeps AI provenance when a human confirms or edits a draft.
    pub proposed_by_ai: bool,
    pub text: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub confirmed: bool,
    pub enabled: bool,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryEdit {
    pub id: Option<u64>,
    pub scope: MemoryScope,
    pub text: String,
    pub enabled: bool,
    pub confirmed: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub source_labels: Vec<(Source, String)>,
    pub version: u64,
    pub records: Vec<MemoryRecord>,
    pub storage_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    pub title: String,
    pub source: Source,
    pub expected_version: String,
    pub code: String,
    pub message: String,
}
pub(crate) fn error(code: &str, message: impl Into<String>, retryable: bool) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into(),
        retryable,
    }
}
pub(crate) fn text_valid(s: &str, max: usize, empty: bool) -> bool {
    (empty || !s.trim().is_empty())
        && s == s.trim()
        && s.chars().count() <= max
        && !s
            .chars()
            .any(|c| c.is_control() || crate::claude::is_bidi(c))
}
pub(crate) fn validate_scope(scope: &MemoryScope) -> Result<(), ToolError> {
    let valid = match scope {
        MemoryScope::Global => true,
        MemoryScope::Directory { path } => {
            crate::path_activity::normalize_path(path).is_ok_and(|p| p == *path)
        }
        MemoryScope::Item { source } => match source {
            Source::SavedItem { id } => text_valid(id, 4096, false),
            Source::ObservedPath { normalized_path } => {
                crate::path_activity::normalize_path(normalized_path)
                    .is_ok_and(|p| p == *normalized_path)
            }
        },
    };
    if valid {
        Ok(())
    } else {
        Err(error("INVALID_ARGUMENTS", "记忆范围无效", true))
    }
}
pub(crate) fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ToolError> {
    serde_json::from_value(value)
        .map_err(|_| error("INVALID_ARGUMENTS", "工具参数无效或包含未知字段", true))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexArgs {
    pub index: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchArgs {
    pub query: String,
    pub limit: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecallArgs {
    pub index: usize,
    pub query: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubmitArgs {
    pub index: usize,
    pub expected_version: String,
    pub alias: Option<String>,
    pub tags: Vec<String>,
    pub reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleArgs {
    pub index: usize,
    pub text: String,
    pub evidence_ids: Vec<String>,
}
