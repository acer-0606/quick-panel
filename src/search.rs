use crate::model::{ItemSummary, StoredItem};
use pinyin::ToPinyin;
use std::collections::BTreeSet;

fn normalize(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch == '\\' { '/' } else { ch })
        .collect()
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x3134f)
}

fn tokens(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut word = String::new();
    let mut previous_cjk = None;
    for ch in normalize(text).chars() {
        if is_cjk(ch) {
            if !word.is_empty() {
                result.insert(std::mem::take(&mut word));
            }
            result.insert(ch.to_string());
            if let Some(previous) = previous_cjk {
                result.insert(format!("{previous}{ch}"));
            }
            previous_cjk = Some(ch);
        } else {
            previous_cjk = None;
            if ch.is_alphanumeric() {
                word.push(ch);
            } else if !word.is_empty() {
                result.insert(std::mem::take(&mut word));
            }
        }
    }
    if !word.is_empty() {
        result.insert(word);
    }
    result
}

fn phonetic(text: &str) -> (String, String) {
    let mut full = String::new();
    let mut initials = String::new();
    let mut latin_start = true;
    for ch in text.chars() {
        if let Some(pronunciation) = ch.to_pinyin() {
            let syllable = pronunciation.plain();
            full.push_str(syllable);
            if let Some(first) = syllable.chars().next() {
                initials.push(first);
            }
            latin_start = true;
        } else if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                full.push(lower);
                if latin_start || ch.is_uppercase() {
                    initials.push(lower);
                }
            }
            latin_start = false;
        } else {
            full.push(' ');
            initials.push(' ');
            latin_start = true;
        }
    }
    (full, initials)
}

fn summary_fields(summary: &ItemSummary) -> Vec<&str> {
    let mut fields = vec![summary.title.as_str(), summary.subtitle.as_str(), summary.alias.as_str()];
    fields.extend(summary.tags.iter().map(String::as_str));
    fields
}

/// Generate FTS tokens, including short Chinese substrings and title phonetics.
/// A set bounds redundant index growth when long clips repeat the same words.
pub fn index_text(item: &StoredItem) -> String {
    let mut indexed = tokens(&item.text);
    for field in summary_fields(&item.summary)
        .into_iter()
        .chain(item.target.as_deref())
    {
        indexed.extend(tokens(field));
        let (full, initials) = phonetic(field);
        indexed.extend(tokens(&full));
        indexed.extend(tokens(&initials));
    }
    indexed.into_iter().collect::<Vec<_>>().join(" ")
}

/// Quote literal tokens rather than interpreting user input as FTS syntax.
pub fn query_expression(query: &str) -> Option<String> {
    let parts = tokens(query)
        .into_iter()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    if parts.is_empty() { None } else { Some(parts.join(" AND ")) }
}

pub fn matches(item: &StoredItem, query: &str) -> bool {
    let query = normalize(query.trim());
    if query.is_empty() {
        return true;
    }
    if normalize(&item.text).contains(&query) {
        return true;
    }
    summary_fields(&item.summary)
        .into_iter()
        .chain(item.target.as_deref())
        .any(|field| {
            let field_normalized = normalize(field);
            if field_normalized.contains(&query) {
                return true;
            }
            let (full, initials) = phonetic(field);
            full.contains(&query) || initials.contains(&query)
        })
}

pub fn rank(summary: &ItemSummary, query: &str) -> i64 {
    let query = normalize(query.trim());
    let alias = normalize(&summary.alias);
    let title = normalize(&summary.title);
    let relevance = if query.is_empty() {
        0
    } else if !alias.is_empty() && alias == query {
        1_000_000
    } else if title == query {
        900_000
    } else if !alias.is_empty() && alias.starts_with(&query) {
        800_000
    } else if title.starts_with(&query) {
        700_000
    } else if title.contains(&query) {
        600_000
    } else {
        let (full, initials) = phonetic(&summary.title);
        if full.starts_with(&query) || initials.starts_with(&query) {
            500_000
        } else if full.contains(&query) || initials.contains(&query) {
            400_000
        } else {
            100_000
        }
    };
    relevance + i64::from(summary.pinned) * 10_000
        + summary.use_count.min(1_000) as i64
}
