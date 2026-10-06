//! Small shared contracts for optional query interpretation; no model runs here.
use crate::model::Scope;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    ApiKey,
    Bearer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    #[default]
    Default,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh", alias = "x_high")]
    XHigh,
    Max,
}
impl Effort {
    pub fn wire_name(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("xhigh"),
            Self::Max => Some("max"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    pub enabled: bool,
    pub endpoint: String,
    pub model: String,
    pub auth: AuthMethod,
    pub effort: Effort,
    pub encrypted_key: Vec<u8>,
}
impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://api.anthropic.com".into(),
            model: String::new(),
            auth: AuthMethod::ApiKey,
            effort: Effort::Default,
            encrypted_key: Vec::new(),
        }
    }
}
impl fmt::Debug for AiSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiSettings")
            .field("enabled", &self.enabled)
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("auth", &self.auth)
            .field("effort", &self.effort)
            .field("encrypted_key", &"<protected>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    #[default]
    Any,
    Today,
    Yesterday,
    #[serde(rename = "last_7_days")]
    Last7Days,
    #[serde(rename = "last_30_days")]
    Last30Days,
}

#[derive(Clone, Debug, Serialize)]
pub struct FunctionDescriptor {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiPlan {
    pub queries: Vec<String>,
    pub scope: Option<Scope>,
    pub period: Period,
    pub functions: Vec<String>,
}
