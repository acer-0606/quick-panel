//! Protocol v1 for a bounded, credential-free, on-demand Pi Agent process.
//!
//! The host owns identity, persistence, tool admission and final source validation.
//! A completion receipt confirms a staged batch, not a database save.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt, io::BufRead};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_LLM_REQUEST_BYTES: usize = 192 * 1024;
pub const MAX_LLM_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_TOOL_RESULT_BYTES: usize = 16 * 1024;
pub const MAX_CHECKPOINT_BYTES: usize = 128 * 1024;
pub const MAX_CHECKPOINT_MESSAGES: usize = 64;
pub const MAX_TOOL_CALLS: u32 = 32;
pub const TASK_DEADLINE_MS: u32 = 120_000;
pub const DEFAULT_MAX_ROUNDS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);
impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ProtocolError {}
type ProtocolResult<T> = Result<T, ProtocolError>;
fn require(ok: bool, message: &str) -> ProtocolResult<()> {
    if ok {
        Ok(())
    } else {
        Err(ProtocolError(message.into()))
    }
}
fn size<T: Serialize>(value: &T, max: usize) -> ProtocolResult<()> {
    require(
        serde_json::to_vec(value)
            .map_err(|e| ProtocolError(e.to_string()))?
            .len()
            <= max,
        "serialized value exceeds byte limit",
    )
}
fn text(value: &str, max: usize) -> bool {
    value.chars().count() <= max
}
fn id(value: &str) -> bool {
    !value.is_empty() && text(value, 128)
}
fn model_id(value: &str) -> bool {
    !value.trim().is_empty() && text(value, 256) && !value.chars().any(|c| c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}
fn default_rounds() -> u32 {
    DEFAULT_MAX_ROUNDS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolName {
    ItemsGetContext,
    ItemsSearch,
    MemoryRecall,
    UsageGetSummary,
    OrganizerSubmit,
    MemoryProposeRule,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicEffort {
    Default,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemRef {
    pub index: u32,
    pub kind: String,
    pub title: String,
    pub version: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}
impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }
    pub fn validate(&self) -> ProtocolResult<()> {
        require(
            id(&self.code) && text(&self.message, 1024),
            "invalid error fields",
        )
    }
}
/// Exactly one of value/error is permitted, matching the JS `ok` discriminator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostResult<T> {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for HostResult<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let map = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("expected result object"))?;
        if map.len() != 2 {
            return Err(serde::de::Error::custom(
                "result requires exactly two fields",
            ));
        }
        match map.get("ok").and_then(Value::as_bool) {
            Some(true) if map.contains_key("value") => {
                let decoded =
                    T::deserialize(map["value"].clone()).map_err(serde::de::Error::custom)?;
                Ok(Self::success(decoded))
            }
            Some(false) if map.contains_key("error") => {
                let error = ToolError::deserialize(map["error"].clone())
                    .map_err(serde::de::Error::custom)?;
                Ok(Self::failure(error))
            }
            _ => Err(serde::de::Error::custom("invalid result discriminator")),
        }
    }
}
impl<T> HostResult<T> {
    pub fn success(value: T) -> Self {
        Self {
            ok: true,
            value: Some(value),
            error: None,
        }
    }
    pub fn failure(error: ToolError) -> Self {
        Self {
            ok: false,
            value: None,
            error: Some(error),
        }
    }
    pub fn into_result(self) -> Result<T, ToolError> {
        match (self.ok, self.value, self.error) {
            (true, Some(value), None) => Ok(value),
            (false, None, Some(error)) => Err(error),
            _ => Err(ToolError::new(
                "protocol",
                "invalid result discriminator",
                false,
            )),
        }
    }
}
impl<T: Serialize> HostResult<T> {
    fn validate(&self, max: usize) -> ProtocolResult<()> {
        match (&self.ok, &self.value, &self.error) {
            (true, Some(value), None) => size(value, max),
            (false, None, Some(error)) => {
                error.validate()?;
                size(error, MAX_TOOL_RESULT_BYTES)
            }
            _ => Err(ProtocolError("invalid result discriminator".into())),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    /// JSON-encoded, settled Pi 1.0.4 transcript. Never contains credentials.
    pub messages_json: String,
    pub rounds: u32,
    pub calls: u32,
    #[serde(default)]
    pub elapsed_ms: u32,
}
impl Checkpoint {
    pub fn validate(&self) -> ProtocolResult<()> {
        size(self, MAX_CHECKPOINT_BYTES)?;
        require(
            self.rounds <= 8 && self.calls <= MAX_TOOL_CALLS && self.elapsed_ms <= TASK_DEADLINE_MS,
            "checkpoint counters exceed budgets",
        )?;
        let messages: Vec<Value> =
            serde_json::from_str(&self.messages_json).map_err(|e| ProtocolError(e.to_string()))?;
        require(
            (2..=MAX_CHECKPOINT_MESSAGES).contains(&messages.len()),
            "invalid checkpoint message count",
        )?;
        let mut pending = BTreeMap::<String, String>::new();
        let mut seen = std::collections::BTreeSet::new();
        let (mut rounds, mut calls) = (0, 0);
        for (i, m) in messages.iter().enumerate() {
            require(m["timestamp"].is_number(), "invalid message timestamp")?;
            match m["role"].as_str() {
                Some("system") => {
                    fields(
                        m,
                        &["role", "content", "toolsAdded", "timestamp"],
                        &["role", "content", "toolsAdded", "timestamp"],
                    )?;
                    require(
                        i == 0 && m["content"].is_string(),
                        "invalid system baseline",
                    )?;
                    let tools = m["toolsAdded"]
                        .as_array()
                        .ok_or_else(|| ProtocolError("missing tool baseline".into()))?;
                    require(tools.len() == 6, "invalid tool count")?;
                    let mut names = std::collections::BTreeSet::new();
                    for tool in tools {
                        fields(
                            tool,
                            &["name", "description", "parameters"],
                            &["name", "description", "parameters"],
                        )?;
                        let name: ToolName = serde_json::from_value(tool["name"].clone())
                            .map_err(|_| ProtocolError("unknown checkpoint tool".into()))?;
                        names.insert(format!("{name:?}"));
                        require(
                            tool["description"].is_string() && tool["parameters"].is_object(),
                            "invalid declaration",
                        )?;
                    }
                    require(names.len() == 6, "duplicate tool declarations")?;
                }
                Some("user") => {
                    fields(
                        m,
                        &["role", "content", "timestamp"],
                        &["role", "content", "timestamp"],
                    )?;
                    require(
                        pending.is_empty() && text_content(&m["content"]),
                        "invalid user message",
                    )?;
                }
                Some("assistant") => {
                    fields(
                        m,
                        &[
                            "role",
                            "content",
                            "api",
                            "provider",
                            "model",
                            "usage",
                            "stopReason",
                            "timestamp",
                            "thinkingLevel",
                        ],
                        &[
                            "role",
                            "content",
                            "api",
                            "provider",
                            "model",
                            "usage",
                            "stopReason",
                            "timestamp",
                        ],
                    )?;
                    require(
                        pending.is_empty()
                            && matches!(
                                m["stopReason"].as_str(),
                                Some("stop" | "toolUse" | "length")
                            )
                            && m["usage"].is_object(),
                        "unsettled assistant message",
                    )?;
                    rounds += 1;
                    for content in m["content"]
                        .as_array()
                        .ok_or_else(|| ProtocolError("invalid assistant content".into()))?
                    {
                        match content["type"].as_str() {
                            Some("text") => {
                                fields(content, &["type", "text"], &["type", "text"])?;
                                require(content["text"].is_string(), "invalid text")?;
                            }
                            Some("thinking") => {
                                fields(
                                    content,
                                    &["type", "thinking", "thinkingSignature", "redacted"],
                                    &["type", "thinking", "thinkingSignature"],
                                )?;
                                require(
                                    content["thinking"].is_string()
                                        && content["thinkingSignature"].is_string()
                                        && content.get("redacted").is_none_or(Value::is_boolean),
                                    "invalid checkpoint thinking",
                                )?;
                            }
                            Some("toolCall") => {
                                fields(
                                    content,
                                    &["type", "id", "name", "arguments"],
                                    &["type", "id", "name", "arguments"],
                                )?;
                                let call = content["id"]
                                    .as_str()
                                    .filter(|v| id(v))
                                    .ok_or_else(|| ProtocolError("invalid tool id".into()))?;
                                let name = content["name"]
                                    .as_str()
                                    .filter(|v| id(v))
                                    .ok_or_else(|| ProtocolError("invalid tool name".into()))?;
                                require(
                                    content["arguments"].is_object()
                                        && seen.insert(call.to_owned()),
                                    "invalid or duplicate call",
                                )?;
                                pending.insert(call.into(), name.into());
                                calls += 1;
                            }
                            _ => {
                                return Err(ProtocolError("unsupported checkpoint content".into()));
                            }
                        }
                    }
                }
                Some("toolResult") => {
                    fields(
                        m,
                        &[
                            "role",
                            "toolCallId",
                            "toolName",
                            "content",
                            "details",
                            "isError",
                            "timestamp",
                        ],
                        &[
                            "role",
                            "toolCallId",
                            "toolName",
                            "content",
                            "isError",
                            "timestamp",
                        ],
                    )?;
                    let call = m["toolCallId"].as_str().unwrap_or("");
                    require(
                        pending.remove(call).as_deref() == m["toolName"].as_str()
                            && m["isError"].is_boolean()
                            && text_content(&m["content"]),
                        "orphan or invalid tool result",
                    )?;
                    size(&m["content"], MAX_TOOL_RESULT_BYTES + 1024)?;
                }
                _ => return Err(ProtocolError("unsupported checkpoint role".into())),
            }
        }
        require(
            pending.is_empty() && rounds <= self.rounds && calls <= self.calls,
            "pending checkpoint calls or inconsistent counters",
        )
    }
}
fn fields(v: &Value, allowed: &[&str], required: &[&str]) -> ProtocolResult<()> {
    let map = v
        .as_object()
        .ok_or_else(|| ProtocolError("expected object".into()))?;
    require(
        map.keys().all(|k| allowed.contains(&k.as_str()))
            && required.iter().all(|k| map.contains_key(*k)),
        "unknown or missing field",
    )
}
fn text_content(v: &Value) -> bool {
    v.is_string()
        || v.as_array().is_some_and(|a| {
            a.iter().all(|c| {
                c.as_object()
                    .is_some_and(|m| m.len() == 2 && c["type"] == "text" && c["text"].is_string())
            })
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRequest {
    pub system: String,
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    pub max_tokens: u32,
}
impl ModelRequest {
    pub fn validate(&self) -> ProtocolResult<()> {
        size(self, MAX_LLM_REQUEST_BYTES)?;
        require(
            self.max_tokens == 4096 && self.tools.len() == 6 && !self.messages.is_empty(),
            "invalid model request",
        )?;
        let mut names = std::collections::BTreeSet::new();
        for tool in &self.tools {
            fields(
                tool,
                &["name", "description", "input_schema"],
                &["name", "description", "input_schema"],
            )?;
            let name: ToolName = serde_json::from_value(tool["name"].clone())
                .map_err(|_| ProtocolError("unknown model tool".into()))?;
            names.insert(format!("{name:?}"));
            require(
                tool["description"].is_string() && tool["input_schema"].is_object(),
                "invalid model tool declaration",
            )?;
        }
        require(names.len() == 6, "duplicate model tools")?;
        for message in &self.messages {
            fields(message, &["role", "content"], &["role", "content"])?;
            require(
                matches!(message["role"].as_str(), Some("user" | "assistant"))
                    && message["content"].is_array(),
                "invalid Anthropic message",
            )?;
            for block in message["content"].as_array().expect("checked") {
                validate_anthropic_block(block, true)?;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelResponse {
    pub content: Vec<Value>,
    pub stop_reason: String,
}
impl ModelResponse {
    pub fn validate(&self) -> ProtocolResult<()> {
        size(self, MAX_LLM_RESPONSE_BYTES)?;
        require(
            matches!(
                self.stop_reason.as_str(),
                "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "refusal"
            ),
            "unknown stop reason",
        )?;
        let mut ids = std::collections::BTreeSet::new();
        for block in &self.content {
            validate_anthropic_block(block, false)?;
            if block["type"] == "tool_use" {
                require(ids.insert(block["id"].as_str()), "duplicate tool id")?;
            }
        }
        Ok(())
    }
}
fn validate_anthropic_block(block: &Value, allow_result: bool) -> ProtocolResult<()> {
    match block["type"].as_str() {
        Some("text") => {
            fields(block, &["type", "text"], &["type", "text"])?;
            require(block["text"].is_string(), "invalid model text")
        }
        Some("thinking") => {
            fields(
                block,
                &["type", "thinking", "signature"],
                &["type", "thinking", "signature"],
            )?;
            require(
                block["thinking"].is_string() && block["signature"].is_string(),
                "invalid signed thinking",
            )
        }
        Some("redacted_thinking") => {
            fields(block, &["type", "data"], &["type", "data"])?;
            require(block["data"].is_string(), "invalid redacted thinking")
        }
        Some("tool_use") => {
            fields(
                block,
                &["type", "id", "name", "input"],
                &["type", "id", "name", "input"],
            )?;
            require(
                block["id"].as_str().is_some_and(id)
                    && block["name"].as_str().is_some_and(id)
                    && block["input"].is_object(),
                "invalid structured tool use",
            )
        }
        Some("tool_result") if allow_result => {
            fields(
                block,
                &["type", "tool_use_id", "content", "is_error"],
                &["type", "tool_use_id", "content", "is_error"],
            )?;
            require(
                block["tool_use_id"].as_str().is_some_and(id)
                    && block["content"].is_string()
                    && block["is_error"].is_boolean(),
                "invalid structured tool result",
            )
        }
        _ => Err(ProtocolError("unsupported Anthropic content block".into())),
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionReceipt {
    pub accepted: bool,
    pub submitted_indices: Vec<u32>,
}
impl CompletionReceipt {
    pub fn validate(&self) -> ProtocolResult<()> {
        let unique: std::collections::BTreeSet<_> =
            self.submitted_indices.iter().copied().collect();
        require(
            self.accepted
                && (1..=8).contains(&unique.len())
                && unique.len() == self.submitted_indices.len()
                && unique.iter().all(|i| *i < 8),
            "invalid completion receipt",
        )
    }
    pub fn covers(&self, items: &[ItemRef]) -> bool {
        self.validate().is_ok()
            && self.submitted_indices.len() == items.len()
            && items
                .iter()
                .all(|i| self.submitted_indices.contains(&i.index))
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostFrame {
    Start {
        protocol: u32,
        request_id: String,
        model: String,
        effort: PublicEffort,
        instructions: String,
        item_refs: Vec<ItemRef>,
        #[serde(default = "default_rounds")]
        max_rounds: u32,
        #[serde(default)]
        checkpoint: Option<Checkpoint>,
    },
    LlmResult {
        request_id: String,
        call_id: String,
        result: HostResult<ModelResponse>,
    },
    ToolResult {
        request_id: String,
        call_id: String,
        result: HostResult<Value>,
    },
    FinishResult {
        request_id: String,
        call_id: String,
        result: HostResult<CompletionReceipt>,
    },
    Cancel {
        request_id: String,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeFrame {
    Ready {
        protocol: u32,
    },
    Llm {
        request_id: String,
        call_id: String,
        model: String,
        effort: PublicEffort,
        request: ModelRequest,
    },
    Tool {
        request_id: String,
        call_id: String,
        name: ToolName,
        arguments: Value,
    },
    Checkpoint {
        request_id: String,
        checkpoint: Checkpoint,
    },
    Finish {
        request_id: String,
        call_id: String,
    },
    Completed {
        request_id: String,
        receipt: CompletionReceipt,
    },
    Failed {
        request_id: Option<String>,
        error: ToolError,
    },
}
impl HostFrame {
    pub fn validate(&self) -> ProtocolResult<()> {
        match self {
            Self::Start {
                protocol,
                request_id,
                model,
                instructions,
                item_refs,
                max_rounds,
                checkpoint,
                ..
            } => {
                require(
                    *protocol == PROTOCOL_VERSION
                        && id(request_id)
                        && model_id(model)
                        && text(instructions, 512)
                        && (2..=8).contains(max_rounds)
                        && (1..=8).contains(&item_refs.len()),
                    "invalid start configuration",
                )?;
                let mut indices = std::collections::BTreeSet::new();
                for item in item_refs {
                    require(
                        item.index < 8
                            && indices.insert(item.index)
                            && text(&item.title, 512)
                            && text(&item.kind, 64)
                            && id(&item.version),
                        "invalid item reference",
                    )?;
                }
                if let Some(cp) = checkpoint {
                    cp.validate()?;
                }
            }
            Self::LlmResult {
                request_id,
                call_id,
                result,
            } => {
                correlation(request_id, call_id)?;
                result.validate(MAX_LLM_RESPONSE_BYTES)?;
                if let Some(value) = &result.value {
                    value.validate()?;
                }
            }
            Self::ToolResult {
                request_id,
                call_id,
                result,
            } => {
                correlation(request_id, call_id)?;
                result.validate(MAX_TOOL_RESULT_BYTES)?;
            }
            Self::FinishResult {
                request_id,
                call_id,
                result,
            } => {
                correlation(request_id, call_id)?;
                result.validate(MAX_TOOL_RESULT_BYTES)?;
                if let Some(value) = &result.value {
                    value.validate()?;
                }
            }
            Self::Cancel { request_id } => require(id(request_id), "invalid request id")?,
        }
        size(self, MAX_FRAME_BYTES)
    }
}
impl RuntimeFrame {
    pub fn validate(&self) -> ProtocolResult<()> {
        match self {
            Self::Ready { protocol } => {
                require(*protocol == PROTOCOL_VERSION, "unsupported protocol")?
            }
            Self::Llm {
                request_id,
                call_id,
                model,
                request,
                ..
            } => {
                correlation(request_id, call_id)?;
                require(model_id(model), "invalid model")?;
                request.validate()?;
            }
            Self::Tool {
                request_id,
                call_id,
                arguments,
                ..
            } => {
                correlation(request_id, call_id)?;
                require(arguments.is_object(), "invalid tool arguments")?;
            }
            Self::Checkpoint {
                request_id,
                checkpoint,
            } => {
                require(id(request_id), "invalid request id")?;
                checkpoint.validate()?;
            }
            Self::Finish {
                request_id,
                call_id,
            } => correlation(request_id, call_id)?,
            Self::Completed {
                request_id,
                receipt,
            } => {
                require(id(request_id), "invalid request id")?;
                receipt.validate()?;
            }
            Self::Failed { request_id, error } => {
                require(
                    request_id.as_ref().is_none_or(|v| id(v)),
                    "invalid request id",
                )?;
                error.validate()?;
            }
        }
        size(self, MAX_FRAME_BYTES)
    }
}
fn correlation(request_id: &str, call_id: &str) -> ProtocolResult<()> {
    require(
        id(request_id) && !call_id.is_empty() && text(call_id, 160),
        "invalid correlated request",
    )
}
pub fn parse_runtime_frame(bytes: &[u8]) -> ProtocolResult<RuntimeFrame> {
    require(bytes.len() <= MAX_FRAME_BYTES, "frame exceeds byte limit")?;
    let frame: RuntimeFrame =
        serde_json::from_slice(bytes).map_err(|e| ProtocolError(e.to_string()))?;
    frame.validate()?;
    Ok(frame)
}
pub fn parse_host_frame(bytes: &[u8]) -> ProtocolResult<HostFrame> {
    require(bytes.len() <= MAX_FRAME_BYTES, "frame exceeds byte limit")?;
    let frame: HostFrame =
        serde_json::from_slice(bytes).map_err(|e| ProtocolError(e.to_string()))?;
    frame.validate()?;
    Ok(frame)
}
pub fn encode_host_frame(frame: &HostFrame) -> ProtocolResult<Vec<u8>> {
    frame.validate()?;
    let mut bytes = serde_json::to_vec(frame).map_err(|e| ProtocolError(e.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}
/// Bounds allocations while reading. EOF mid-frame is a protocol failure.
/// Stop consuming this child stream on any error; do not attempt resynchronization.
pub fn read_runtime_frame(reader: &mut impl BufRead) -> ProtocolResult<Option<RuntimeFrame>> {
    let mut bytes = Vec::new();
    loop {
        let chunk = reader
            .fill_buf()
            .map_err(|e| ProtocolError(e.to_string()))?;
        if chunk.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(ProtocolError("EOF within frame".into()))
            };
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let count = newline.unwrap_or(chunk.len());
        require(
            bytes.len() + count <= MAX_FRAME_BYTES,
            "frame exceeds byte limit",
        )?;
        bytes.extend_from_slice(&chunk[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return parse_runtime_frame(&bytes).map(Some);
        }
    }
}
