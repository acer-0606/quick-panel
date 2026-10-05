//! Local Windows path observations. Normalization is syntactic and never accesses targets.
use crate::model::ItemKind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathActivitySettings {
    pub enabled: bool,
    pub recent_documents: bool,
    pub excluded_paths: Vec<String>,
}
impl Default for PathActivitySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            recent_documents: true,
            excluded_paths: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationSource {
    Explorer,
    RecentDocuments,
    QuickPanel,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PathObservation {
    pub path: String,
    pub kind: ItemKind,
    pub observed_at_ms: i64,
    pub source: ObservationSource,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSuggestion {
    pub key: String,
    pub path: String,
    pub title: String,
    pub kind: ItemKind,
    pub visits: u64,
    pub last_seen_ms: i64,
}
pub(crate) const MAX_PATH_UNITS: usize = 2048;
pub(crate) const MAX_BATCH: usize = 32;
pub(crate) const MAX_QUEUE_BYTES: usize = 256 * 1024;

pub fn normalize_path(path: &str) -> Result<String, String> {
    let invalid = || "需要有效的 Windows 绝对路径".to_string();
    if path.encode_utf16().count() > MAX_PATH_UNITS || path.chars().any(char::is_control) {
        return Err(invalid());
    }
    let path = path.replace('/', "\\").to_lowercase();
    if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return Err(invalid());
    }
    let (root, rest) = if let Some(unc) = path.strip_prefix(r"\\") {
        let mut parts = unc.split('\\').filter(|p| !p.is_empty());
        let server = parts.next().ok_or_else(invalid)?;
        let share = parts.next().ok_or_else(invalid)?;
        if !valid_component(server)
            || !valid_component(share)
            || matches!(server, "." | "..")
            || matches!(share, "." | "..")
        {
            return Err(invalid());
        }
        (format!(r"\\{server}\{share}\"), parts.collect::<Vec<_>>())
    } else {
        let bytes = path.as_bytes();
        if bytes.len() < 3
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || bytes[2] != b'\\'
        {
            return Err(invalid());
        }
        (
            path[..3].to_owned(),
            path[3..].split('\\').collect::<Vec<_>>(),
        )
    };
    let mut components = Vec::new();
    for component in rest {
        match component {
            "" | "." => (),
            ".." => {
                components.pop().ok_or_else(invalid)?;
            }
            value if valid_component(value) && !device_component(value) => components.push(value),
            _ => return Err(invalid()),
        }
    }
    let result = root + &components.join("\\");
    if result.encode_utf16().count() > MAX_PATH_UNITS {
        return Err(invalid());
    }
    Ok(result)
}
fn valid_component(component: &str) -> bool {
    !component
        .chars()
        .any(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        && !component.ends_with(' ')
        && (!component.ends_with('.') || matches!(component, "." | ".."))
}
pub(crate) fn excluded(path: &str, roots: &[String]) -> bool {
    roots
        .iter()
        .filter_map(|root| normalize_path(root).ok())
        .any(|root| {
            path == root
                || path
                    .strip_prefix(&root)
                    .is_some_and(|rest| root.ends_with('\\') || rest.starts_with('\\'))
        })
}

fn device_component(component: &str) -> bool {
    let name = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ');
    matches!(name, "con" | "prn" | "aux" | "nul" | "conin$" | "conout$")
        || name
            .strip_prefix("com")
            .or_else(|| name.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}
