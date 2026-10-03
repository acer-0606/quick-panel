//! OpenAI-compatible Embeddings protocol; only explicit bounded text batches.
use crate::{
    claude::{Endpoint, HttpRequest},
    semantic::{EmbeddingSettings, MAX_INPUT_CHARS, validate_vectors},
};
use serde::Deserialize;
use serde_json::json;

pub const MAX_BATCH_INPUTS: usize = 16;
pub const MAX_REQUEST_BYTES: usize = 512 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const INVALID_RESPONSE: &str = "Embedding API 返回的向量无效";

pub fn parse_endpoint(address: &str) -> Result<Endpoint, String> {
    crate::claude::parse_endpoint_for(address, "embeddings")
}

pub(crate) fn validate_model(model: &str) -> Result<(), String> {
    if model.trim().is_empty() || model.chars().count() > 256 || model.chars().any(char::is_control)
    {
        return Err("Embedding 模型 ID 无效".into());
    }
    Ok(())
}

pub(crate) fn validate_request(
    settings: &EmbeddingSettings,
    inputs: &[String],
) -> Result<(), String> {
    if !settings.enabled {
        return Err("请先启用语义检索".into());
    }
    validate_model(&settings.model)?;
    parse_endpoint(&settings.endpoint)?;
    if inputs.is_empty() || inputs.len() > MAX_BATCH_INPUTS {
        return Err("语义索引批次须包含 1 到 16 条文本".into());
    }
    for input in inputs {
        if input.trim().is_empty()
            || input.chars().count() > MAX_INPUT_CHARS
            || input
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            return Err("语义输入为空、过长或含无效字符".into());
        }
    }
    Ok(())
}

pub fn prepare(
    settings: &EmbeddingSettings,
    key: &str,
    inputs: &[String],
) -> Result<HttpRequest, String> {
    validate_request(settings, inputs)?;
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err("Embedding API 密钥无效".into());
    }
    let body = serde_json::to_vec(
        &json!({"model": settings.model.trim(), "input": inputs, "encoding_format": "float"}),
    )
    .map_err(|_| "无法准备语义检索请求")?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err("语义检索请求过大".into());
    }
    Ok(HttpRequest {
        endpoint: parse_endpoint(&settings.endpoint)?,
        headers: vec![
            ("Content-Type".into(), "application/json".into()),
            ("Authorization".into(), format!("Bearer {key}")),
        ],
        body,
    })
}

#[derive(Deserialize)]
struct Response {
    data: Vec<ResponseEntry>,
}
#[derive(Deserialize)]
struct ResponseEntry {
    index: usize,
    embedding: Vec<f32>,
}

pub fn parse_response(
    status: u16,
    response: &[u8],
    expected_inputs: usize,
) -> Result<Vec<Vec<f32>>, String> {
    if !(200..=299).contains(&status) {
        return Err(match status {
            401 | 403 => "Embedding API 认证失败，请检查密钥和权限",
            404 => "Embedding API 地址或模型不存在，请检查配置",
            429 => "Embedding API 请求过于频繁，请稍后再试",
            500..=599 => "Embedding API 服务暂时不可用，请稍后再试",
            _ => "Embedding API 请求失败，请检查配置",
        }
        .into());
    }
    if response.len() > MAX_RESPONSE_BYTES {
        return Err("Embedding API 响应过大".into());
    }
    if expected_inputs == 0 || expected_inputs > MAX_BATCH_INPUTS {
        return Err(INVALID_RESPONSE.into());
    }
    let response: Response = serde_json::from_slice(response).map_err(|_| INVALID_RESPONSE)?;
    if response.data.len() != expected_inputs {
        return Err(INVALID_RESPONSE.into());
    }
    let mut ordered = vec![None; expected_inputs];
    for entry in response.data {
        let slot = ordered.get_mut(entry.index).ok_or(INVALID_RESPONSE)?;
        if slot.is_some() {
            return Err(INVALID_RESPONSE.into());
        }
        *slot = Some(entry.embedding);
    }
    let vectors = ordered
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(INVALID_RESPONSE)?;
    validate_vectors(&vectors).map_err(|_| INVALID_RESPONSE)?;
    Ok(vectors)
}
