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
impl SmartView {
    /// Explicit Claude requests never scan or wait for unrelated embeddings.
    pub fn needs_index_status(&self) -> bool {
        self.preferred != SmartSearchMode::Claude && self.embedding_enabled
    }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartEntry {
    Toggle,
    Explicit(Option<SmartSearchMode>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartTransition {
    Cancel,
    Start {
        cancel_previous: bool,
        preferred: Option<SmartSearchMode>,
    },
}
pub fn entry_transition(entry: SmartEntry, active: bool) -> SmartTransition {
    if active && entry == SmartEntry::Toggle {
        return SmartTransition::Cancel;
    }
    SmartTransition::Start {
        cancel_previous: active,
        preferred: match entry {
            SmartEntry::Toggle => None,
            SmartEntry::Explicit(mode) => mode,
        },
    }
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
    terminal_refresh: Option<String>,
}
impl StatusCache {
    pub fn mark_dirty(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }
    pub fn request(&mut self, profile: &str) -> Option<Command> {
        self.discard_terminal_for_other_profile(profile);
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
    /// All native callers use this gate. Only a real index terminal can reserve
    /// one scan while settings owns navigation; a busy slot does not consume it.
    pub fn request_for_view(&mut self, profile: &str, panel_allowed: bool) -> Option<Command> {
        self.discard_terminal_for_other_profile(profile);
        if !panel_allowed && self.terminal_refresh.is_none() {
            return None;
        }
        let command = self.request(profile)?;
        self.terminal_refresh = None;
        Some(command)
    }
    /// Called only after IndexSession::take_status_refresh reports a terminal.
    pub fn index_terminal(&mut self, profile: &str) {
        self.mark_dirty();
        self.terminal_refresh = (!profile.is_empty()).then(|| profile.to_owned());
    }
    pub fn invalidate_settings(&mut self) {
        self.terminal_refresh = None;
        self.mark_dirty();
    }
    fn discard_terminal_for_other_profile(&mut self, profile: &str) {
        if self
            .terminal_refresh
            .as_deref()
            .is_some_and(|p| p != profile)
        {
            self.terminal_refresh = None;
        }
    }
    pub fn ready(&self, profile: &str) -> Option<&SemanticIndexStatus> {
        if self.dirty || self.in_flight.is_some() {
            return None;
        }
        self.cached.as_ref().filter(|s| s.profile == profile)
    }
    pub fn complete(&mut self, id: u64, status: SemanticIndexStatus, profile: &str) -> bool {
        self.discard_terminal_for_other_profile(profile);
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
        self.terminal_refresh = None;
        self.cached = Some(status);
        true
    }
    pub fn fail(&mut self, id: u64) -> bool {
        if self.in_flight.as_ref().is_none_or(|p| p.0 != id) {
            return false;
        }
        self.in_flight = None;
        self.terminal_refresh = None;
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
        if view.needs_index_status() && status.is_none_or(|s| s.profile != view.profile) {
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
