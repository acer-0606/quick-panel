//! Portable preview sizing, capture admission and actor Load coalescing.
use crate::{
    clipboard_formats::MAX_ITEM_BYTES,
    model::{ItemKind, ItemSummary},
};
pub const MAX_SOURCE_PIXELS: u64 = 100_000_000;
pub const MAX_COMPATIBILITY_BYTES: usize = 64 * 1024 * 1024;
pub fn thumbnail_size(width: u32, height: u32) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err("图片源尺寸超过1亿像素上限；仍可复制原图".into());
    }
    let edge = width.max(height);
    if edge <= 1024 {
        return Ok((width, height));
    }
    Ok((
        (u64::from(width) * 1024 / u64::from(edge)).max(1) as u32,
        (u64::from(height) * 1024 / u64::from(edge)).max(1) as u32,
    ))
}
pub fn compatibility_bytes(width: u32, height: u32) -> Result<usize, String> {
    let bytes = (u64::from(width) * u64::from(height))
        .checked_mul(4)
        .ok_or("图片兼容表示尺寸溢出")?;
    if width == 0 || height == 0 || bytes > MAX_COMPATIBILITY_BYTES as u64 {
        return Err("图片兼容表示超过64 MiB上限；保留PNG原图".into());
    }
    Ok(bytes as usize)
}
pub fn capture_format(
    name: &str,
    length: usize,
    total: usize,
    limit: usize,
    png: bool,
) -> Result<bool, String> {
    if total
        .checked_add(length)
        .is_none_or(|n| n > limit.min(MAX_ITEM_BYTES))
    {
        if png && matches!(name, "CF_DIB" | "CF_DIBV5") {
            return Ok(false);
        }
        return Err("剪贴板内容超过8 MiB或配置上限，未记录".into());
    }
    Ok(true)
}
pub fn bounded_text(text: &str) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(6000).collect();
    if chars.next().is_some() {
        result.push_str("\r\n\r\n[预览已截断，仅显示前6000字符]");
    }
    result.replace('\n', "\r\n").replace("\r\r\n", "\r\n")
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    id: String,
    kind: ItemKind,
    title: String,
    subtitle: String,
    alias: String,
    tags: Vec<String>,
}
impl From<&ItemSummary> for Identity {
    fn from(i: &ItemSummary) -> Self {
        Self {
            id: i.id.clone(),
            kind: i.kind,
            title: i.title.clone(),
            subtitle: i.subtitle.clone(),
            alias: i.alias.clone(),
            tags: i.tags.clone(),
        }
    }
}
#[derive(Default)]
pub struct PreviewLoads {
    generation: u64,
    desired: Option<Identity>,
    inflight: Option<(u64, Identity)>,
    pending: Option<(u64, ItemSummary)>,
}
impl PreviewLoads {
    pub fn select(&mut self, item: Option<ItemSummary>, id: u64) -> Option<(u64, ItemSummary)> {
        let identity = item.as_ref().map(Identity::from);
        if self.desired == identity {
            return None;
        }
        self.generation = id;
        self.desired = identity;
        self.pending = item.map(|item| (id, item));
        if self.inflight.is_some() {
            return None;
        }
        self.start_pending()
    }
    fn start_pending(&mut self) -> Option<(u64, ItemSummary)> {
        let (id, item) = self.pending.take()?;
        self.inflight = Some((id, Identity::from(&item)));
        Some((id, item))
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cancel(&mut self) {
        self.desired = None;
        self.pending = None;
    }
    pub fn complete(&mut self, id: u64) -> (bool, Option<(u64, ItemSummary)>) {
        if self
            .inflight
            .as_ref()
            .is_none_or(|(active, _)| *active != id)
        {
            return (false, None);
        }
        let (_, identity) = self.inflight.take().unwrap();
        let current = self.desired.as_ref() == Some(&identity);
        // Returning to the in-flight target does not need a second Load.
        if current {
            self.pending = None;
        }
        (current, self.start_pending())
    }
}

/// Content-derived clipboard ids do not version mutable display metadata.
pub fn same_content_version(a: &ItemSummary, b: &ItemSummary) -> bool {
    Identity::from(a) == Identity::from(b)
}
