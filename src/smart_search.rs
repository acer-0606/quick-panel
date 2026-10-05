//! Portable user intent and local status gates for the single smart entry point.
use crate::model::*;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartView {
    pub query: String,
    pub scope: Scope,
    pub profile: String,
    pub preferred: SmartSearchMode,
    pub ai_enabled: bool,
    pub embedding_enabled: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartAction {
    Semantic,
    Claude,
    BuildIndex,
    Configure,
    ConfigureClaude,
    ConfigureSemantic,
}
pub fn choose(view: &SmartView, status: Option<&SemanticIndexStatus>) -> SmartAction {
    let indexed = view.embedding_enabled
        && status.is_some_and(|s| s.profile == view.profile && s.indexed > 0);
    match view.preferred {
        SmartSearchMode::Claude if view.ai_enabled => SmartAction::Claude,
        SmartSearchMode::Claude => SmartAction::ConfigureClaude,
        SmartSearchMode::Semantic if !view.embedding_enabled => SmartAction::ConfigureSemantic,
        SmartSearchMode::Semantic if indexed => SmartAction::Semantic,
        SmartSearchMode::Semantic => SmartAction::BuildIndex,
        SmartSearchMode::Auto if indexed => SmartAction::Semantic,
        SmartSearchMode::Auto if view.ai_enabled => SmartAction::Claude,
        SmartSearchMode::Auto if view.embedding_enabled => SmartAction::BuildIndex,
        SmartSearchMode::Auto => SmartAction::Configure,
    }
}

/// Exactly one actual state scan; changes during it collapse into one follow-up.
#[derive(Default)]
pub struct StatusCache {
    profile: String,
    serial: u64,
    generation: u64,
    in_flight: Option<(u64, String, u64)>,
    cached: Option<SemanticIndexStatus>,
    dirty: bool,
}
impl StatusCache {
    pub fn mark_dirty(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }
    pub fn request(&mut self, profile: &str) -> Option<Command> {
        if self.profile != profile {
            self.profile = profile.into();
            self.cached = None;
            self.mark_dirty();
        }
        if profile.is_empty() || self.in_flight.is_some() || (!self.dirty && self.cached.is_some())
        {
            return None;
        }
        self.serial = self.serial.wrapping_add(1);
        self.in_flight = Some((self.serial, profile.into(), self.generation));
        self.dirty = false;
        Some(Command::SemanticStatus {
            request_id: self.serial,
        })
    }
    pub fn ready(&self, profile: &str) -> Option<&SemanticIndexStatus> {
        if self.dirty || self.in_flight.is_some() {
            return None;
        }
        self.cached.as_ref().filter(|s| s.profile == profile)
    }
    pub fn complete(&mut self, id: u64, status: SemanticIndexStatus, profile: &str) -> bool {
        if self.in_flight.as_ref().is_none_or(|p| p.0 != id) {
            return false;
        }
        let (_, requested_profile, generation) = self.in_flight.take().unwrap();
        if requested_profile != profile
            || status.profile != profile
            || generation != self.generation
        {
            self.dirty = true;
            return false;
        }
        self.cached = Some(status);
        true
    }
    pub fn fail(&mut self, id: u64) -> bool {
        if self.in_flight.as_ref().is_none_or(|p| p.0 != id) {
            return false;
        }
        self.in_flight = None;
        self.cached = None;
        self.dirty = true;
        true
    }
}
#[derive(Default)]
pub struct SmartPreflight {
    pending: Option<SmartView>,
}
impl SmartPreflight {
    pub fn begin(&mut self, view: SmartView) {
        self.pending = Some(view);
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn finish(
        &mut self,
        view: &SmartView,
        active: bool,
        status: Option<&SemanticIndexStatus>,
    ) -> Option<SmartAction> {
        let pending = self.pending.as_ref()?;
        if !active || pending != view {
            self.cancel();
            return None;
        }
        if view.embedding_enabled && status.is_none_or(|s| s.profile != view.profile) {
            return None;
        }
        self.cancel();
        Some(choose(view, status))
    }
}

/// Shared validation, used by the native metadata form and the persistence command.
pub fn normalize_metadata(
    mut input: ClipboardMetadataInput,
) -> Result<ClipboardMetadataInput, String> {
    let title = input.title.trim();
    let alias = input.alias.trim();
    if title.is_empty()
        || title.chars().count() > 160
        || alias.chars().count() > 128
        || input.tags.len() > 32
        || input.tags.iter().any(|t| t.trim().chars().count() > 64)
        || title
            .chars()
            .chain(alias.chars())
            .chain(input.tags.iter().flat_map(|t| t.chars()))
            .any(char::is_control)
    {
        return Err(
            "名称需为 1–160 字符，别名最多 128 字符，标签最多 32 个、各 64 字符，且不能含控制字符"
                .into(),
        );
    }
    input.title = title.into();
    input.alias = alias.into();
    let mut seen = HashSet::new();
    input.tags = input
        .tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty() && seen.insert(t.to_lowercase()))
        .map(str::to_owned)
        .collect();
    Ok(input)
}

pub struct ClipboardMetadataForm {
    pub summary: ItemSummary,
}
impl ClipboardMetadataForm {
    pub fn new(summary: &ItemSummary) -> Option<Self> {
        (summary.kind == ItemKind::Clipboard && summary.pinned).then(|| Self {
            summary: summary.clone(),
        })
    }
    pub fn finish(
        &self,
        fields: Option<Vec<String>>,
        target_current: bool,
    ) -> Result<Option<ClipboardMetadataInput>, String> {
        let Some(fields) = fields.filter(|_| target_current) else {
            return Ok(None);
        };
        if fields.len() != 3 {
            return Err("名称、别名和标签字段不完整".into());
        }
        normalize_metadata(ClipboardMetadataInput {
            id: self.summary.id.clone(),
            title: fields[0].clone(),
            alias: fields[1].clone(),
            tags: fields[2]
                .split([',', '，'])
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .collect(),
        })
        .map(Some)
    }
}
