//! Pure display labels for one bounded palette snapshot. Never resolves a path.
use crate::path_activity::normalize_target;

pub(crate) struct Label {
    pub leaf: String,
    pub title: String,
    pub context: String,
}
struct Path {
    text: String,
    parts: Vec<String>,
    keys: Vec<String>,
}

pub(crate) fn labels(paths: &[Option<&str>], query: &str) -> Vec<Option<Label>> {
    let paths: Vec<_> = paths
        .iter()
        .take(50)
        .map(|path| {
            let text = normalize_target(path.as_ref()?).ok()?;
            let parts: Vec<String> = text
                .split('\\')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let keys = parts.iter().map(|s| s.to_lowercase()).collect();
            Some(Path { text, parts, keys })
        })
        .collect();
    let query = query.trim().to_lowercase();
    paths
        .iter()
        .map(|path| {
            let path = path.as_ref()?;
            let leaf = path.parts.last()?.clone();
            let peers: Vec<_> = paths
                .iter()
                .flatten()
                .filter(|other| other.keys != path.keys && other.keys.last() == path.keys.last())
                .collect();
            let common = peers
                .iter()
                .map(|other| {
                    path.keys
                        .iter()
                        .rev()
                        .zip(other.keys.iter().rev())
                        .take_while(|(a, b)| a == b)
                        .count()
                })
                .max()
                .unwrap_or(0);
            let focus = path.parts.len().saturating_sub(common + 1);
            let other_names: Vec<_> = peers
                .iter()
                .filter_map(|other| {
                    other
                        .parts
                        .len()
                        .checked_sub(common + 1)
                        .map(|i| other.parts[i].as_str())
                })
                .collect();
            let distinct = focus_text(&path.parts[focus], &other_names);
            let tail = middle(&path.parts[focus + 1..].join(" / "), 34);
            let title = if common > 0 && !tail.is_empty() {
                format!("{distinct} · {tail}")
            } else {
                middle(&leaf, 60)
            };

            // Once filtering leaves only one item, keep its matching directory visible too.
            let context_focus = if common > 0 {
                Some(focus)
            } else if !query.is_empty() {
                path.keys.iter().position(|part| part.contains(&query))
            } else {
                None
            };
            let context = if let Some(index) = context_focus {
                let first = if index == focus {
                    distinct
                } else {
                    focus_text(&path.parts[index], &[])
                };
                let rest = middle(&path.parts[index + 1..].join("\\"), 44);
                let suffix = if rest.is_empty() {
                    first
                } else {
                    format!("{first}\\{rest}")
                };
                if index == 0 {
                    suffix
                } else {
                    let prefix = if path.text.starts_with("\\\\") {
                        "\\\\"
                    } else {
                        ""
                    };
                    format!(
                        "{suffix} · {prefix}{}\\…",
                        middle(&path.parts[..index].join("\\"), 28)
                    )
                }
            } else {
                middle(&path.text, 100)
            };
            Some(Label {
                leaf,
                title,
                context,
            })
        })
        .collect()
}

fn focus_text(value: &str, peers: &[&str]) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= 24 {
        return value.into();
    }
    let common = peers
        .iter()
        .filter(|other| other.to_lowercase() != value.to_lowercase())
        .map(|other| {
            chars
                .iter()
                .copied()
                .zip(other.chars())
                .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                .count()
        })
        .max();
    let Some(common) = common else {
        return middle(value, 24);
    };
    let start = common.saturating_sub(4).min(chars.len().saturating_sub(22));
    let end = (start + 22).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    )
}

fn middle(value: &str, max: usize) -> String {
    let chars: Vec<_> = value.chars().collect();
    if chars.len() <= max {
        return value.into();
    }
    let head = (max - 1) / 2;
    let tail = max - head - 1;
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}
