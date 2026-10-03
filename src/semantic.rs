//! Portable configuration and input contracts for explicit semantic indexing.
use crate::model::{ItemKind, Scope, StoredItem};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexScope {
    #[default]
    PinnedAndFavorites,
    AllText,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingSettings {
    pub enabled: bool,
    pub endpoint: String,
    pub model: String,
    pub encrypted_key: Vec<u8>,
    pub index_scope: IndexScope,
}
impl Default for EmbeddingSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://api.openai.com".into(),
            model: String::new(),
            encrypted_key: Vec::new(),
            index_scope: IndexScope::default(),
        }
    }
}
impl fmt::Debug for EmbeddingSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddingSettings")
            .field("enabled", &self.enabled)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("encrypted_key", &"<protected>")
            .field("index_scope", &self.index_scope)
            .finish()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingCandidate {
    pub id: String,
    pub text: String,
    pub text_hash: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct IndexedEmbedding {
    pub id: String,
    pub text_hash: String,
    pub vector: Vec<f32>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexCursor {
    pub after_rowid: i64,
    pub disk_done: bool,
    pub pending_after_id: Option<String>,
}
#[derive(Clone, Debug)]
pub struct IndexBatch {
    pub request_id: u64,
    pub profile: String,
    pub candidates: Vec<EmbeddingCandidate>,
    pub cursor: IndexCursor,
    pub done: bool,
}
#[derive(Clone, Debug, Default)]
pub struct SemanticSearchQuery {
    pub text: String,
    pub scope: Scope,
    pub request_id: u64,
    pub profile: String,
    pub vector: Vec<f32>,
}
/// Version the actual text format separately from credentials and indexing scope.
const INPUT_FORMAT_VERSION: &str = "quick-panel-semantic-input-v1";
pub const MAX_INPUT_CHARS: usize = 4096;
pub const MAX_VECTOR_DIMENSIONS: usize = 4096;

pub fn profile_id(settings: &EmbeddingSettings) -> Result<String, String> {
    crate::embedding::validate_model(&settings.model)?;
    let endpoint = crate::embedding::parse_endpoint(&settings.endpoint)?;
    let host = endpoint
        .host
        .parse::<std::net::Ipv6Addr>()
        .map(|host| host.to_string())
        .unwrap_or(endpoint.host);
    // JSON frames variable-length fields, avoiding ambiguous concatenation.
    let encoded = serde_json::to_vec(&(
        INPUT_FORMAT_VERSION,
        endpoint.secure,
        host,
        endpoint.port,
        endpoint.path,
        settings.model.trim(),
    ))
    .map_err(|_| "无法准备语义索引配置")?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub(crate) fn sendable_character(character: char) -> bool {
    !character.is_control() || matches!(character, '\n' | '\r' | '\t')
}

fn has_input_text(text: &str) -> bool {
    text.chars()
        .any(|character| sendable_character(character) && !character.is_whitespace())
}

pub fn candidate(item: &StoredItem, scope: IndexScope) -> Option<EmbeddingCandidate> {
    let clipboard = item.summary.kind == ItemKind::Clipboard;
    if clipboard
        && (!has_input_text(&item.text)
            || (scope == IndexScope::PinnedAndFavorites && !item.summary.pinned))
    {
        return None;
    }
    let content = if clipboard {
        item.text.as_str()
    } else {
        item.target.as_deref().unwrap_or("")
    };
    let fields = [item.summary.title.as_str(), item.summary.alias.as_str()]
        .into_iter()
        .chain(item.summary.tags.iter().map(String::as_str))
        .chain(std::iter::once(content));
    let mut text = String::new();
    let mut remaining = MAX_INPUT_CHARS;
    for field in fields.filter(|field| has_input_text(field)) {
        if remaining == 0 {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
            remaining -= 1;
        }
        for character in field
            .chars()
            .filter(|character| sendable_character(*character))
            .take(remaining)
        {
            text.push(character);
            remaining -= 1;
        }
    }
    if text.is_empty() {
        return None;
    }
    let text_hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    Some(EmbeddingCandidate {
        id: item.summary.id.clone(),
        text,
        text_hash,
    })
}

fn validate_vector(vector: &[f32]) -> Result<usize, String> {
    if vector.is_empty()
        || vector.len() > MAX_VECTOR_DIMENSIONS
        || vector.iter().any(|value| !value.is_finite())
        || vector.iter().all(|value| *value == 0.0)
    {
        return Err("语义向量无效".into());
    }
    Ok(vector.len())
}

pub fn validate_vectors(vectors: &[Vec<f32>]) -> Result<usize, String> {
    let dimensions = validate_vector(vectors.first().ok_or("语义向量为空")?)?;
    for vector in vectors.iter().skip(1) {
        if validate_vector(vector)? != dimensions {
            return Err("语义向量维度不一致".into());
        }
    }
    Ok(dimensions)
}

pub fn cosine_distance(left: &[f32], right: &[f32]) -> Result<f32, String> {
    if validate_vector(left)? != validate_vector(right)? {
        return Err("语义向量维度不一致".into());
    }
    // f64 accumulation keeps both extreme and subnormal finite f32 inputs safe.
    let (mut dot, mut left_norm, mut right_norm) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (&left, &right) in left.iter().zip(right) {
        let (left, right) = (f64::from(left), f64::from(right));
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    Ok((1.0 - (dot / (left_norm.sqrt() * right_norm.sqrt())).clamp(-1.0, 1.0)) as f32)
}
