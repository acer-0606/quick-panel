//! Credential-safe, bounded Claude-compatible model discovery.
use crate::{
    ai::{AiSettings, Effort},
    claude::{self, HttpRequest},
};
use serde::{Deserialize, Serialize};

pub const MAX_PAGES: usize = 5;
pub const MAX_MODELS: usize = 200;
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const INVALID: &str = "AI API 返回的模型列表无效";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortCapabilities {
    pub supported: Option<bool>,
    pub low: Option<bool>,
    pub medium: Option<bool>,
    pub high: Option<bool>,
    pub xhigh: Option<bool>,
    pub max: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub effort: Option<EffortCapabilities>,
}
impl ModelInfo {
    /// None means the service did not report this capability, not unsupported.
    pub fn supports_effort(&self, effort: Effort) -> Option<bool> {
        if effort == Effort::Default {
            return None;
        }
        let capabilities = self.effort.as_ref()?;
        if capabilities.supported == Some(false) {
            return Some(false);
        }
        match effort {
            Effort::Low => capabilities.low,
            Effort::Medium => capabilities.medium,
            Effort::High => capabilities.high,
            Effort::XHigh => capabilities.xhigh,
            Effort::Max => capabilities.max,
            Effort::Default => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPage {
    pub models: Vec<ModelInfo>,
    pub next_cursor: Option<String>,
}

#[derive(Default)]
pub struct ModelListing {
    models: Vec<ModelInfo>,
    pages: usize,
}
impl ModelListing {
    pub fn models(&self) -> &[ModelInfo] {
        &self.models
    }
    pub fn into_models(self) -> Vec<ModelInfo> {
        self.models
    }
    pub fn push_page(&mut self, page: ModelPage) -> Result<Option<String>, String> {
        if self.pages >= MAX_PAGES
            || (self.pages + 1 == MAX_PAGES && page.next_cursor.is_some())
            || self.models.len().saturating_add(page.models.len()) > MAX_MODELS
            || page
                .models
                .iter()
                .any(|model| self.models.iter().any(|prior| prior.id == model.id))
        {
            return Err("模型列表超过上限或含重复项".into());
        }
        self.pages += 1;
        self.models.extend(page.models);
        Ok(page.next_cursor)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._+/".contains(&c))
}
fn escaped_cursor(cursor: &str) -> Result<String, String> {
    if !valid_id(cursor) {
        return Err("模型列表游标无效".into());
    }
    let mut escaped = String::new();
    for byte in cursor.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    Ok(escaped)
}

/// Explicit GET request data, independent of the AI search switch and model ID.
pub fn prepare(
    settings: &AiSettings,
    key: &str,
    cursor: Option<&str>,
) -> Result<HttpRequest, String> {
    let mut endpoint = claude::parse_endpoint_for(&settings.endpoint, "models")?;
    endpoint.path.push_str("?limit=100");
    if let Some(cursor) = cursor {
        endpoint.path.push_str("&after_id=");
        endpoint.path.push_str(&escaped_cursor(cursor)?);
    }
    Ok(HttpRequest {
        endpoint,
        headers: claude::request_headers(settings, key)?,
        body: Vec::new(),
    })
}

#[derive(Deserialize)]
struct RawPage {
    data: Vec<RawModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}
#[derive(Deserialize)]
struct RawModel {
    id: String,
    display_name: Option<String>,
    capabilities: Option<RawCapabilities>,
}
#[derive(Deserialize)]
struct RawCapabilities {
    effort: Option<RawEffort>,
}
#[derive(Deserialize)]
struct Support {
    supported: bool,
}
#[derive(Deserialize)]
struct RawEffort {
    supported: Option<bool>,
    low: Option<Support>,
    medium: Option<Support>,
    high: Option<Support>,
    xhigh: Option<Support>,
    max: Option<Support>,
}

pub fn parse_page(status: u16, response: &[u8]) -> Result<ModelPage, String> {
    claude::check_status(status)?;
    if response.len() > MAX_RESPONSE_BYTES {
        return Err("模型列表响应超过 256 KiB".into());
    }
    let raw: RawPage = serde_json::from_slice(response).map_err(|_| INVALID)?;
    if raw.data.len() > MAX_MODELS || raw.last_id.as_ref().is_some_and(|id| !valid_id(id)) {
        return Err(INVALID.into());
    }
    let mut models: Vec<ModelInfo> = Vec::new();
    for model in raw.data {
        if !valid_id(&model.id)
            || models.iter().any(|prior| prior.id == model.id)
            || model.display_name.as_ref().is_some_and(|name| {
                name.trim().is_empty()
                    || name.chars().count() > 256
                    || name.chars().any(|c| c.is_control() || claude::is_bidi(c))
            })
        {
            return Err(INVALID.into());
        }
        let effort = model
            .capabilities
            .and_then(|c| c.effort)
            .map(|c| EffortCapabilities {
                supported: c.supported,
                low: c.low.map(|s| s.supported),
                medium: c.medium.map(|s| s.supported),
                high: c.high.map(|s| s.supported),
                xhigh: c.xhigh.map(|s| s.supported),
                max: c.max.map(|s| s.supported),
            });
        models.push(ModelInfo {
            id: model.id,
            display_name: model.display_name,
            effort,
        });
    }
    let next_cursor = if raw.has_more {
        let cursor = raw.last_id.ok_or(INVALID)?;
        if models.last().is_none_or(|model| model.id != cursor) {
            return Err(INVALID.into());
        }
        Some(cursor)
    } else {
        None
    };
    Ok(ModelPage {
        models,
        next_cursor,
    })
}
