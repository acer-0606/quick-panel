//! Non-secret scene data and explicit, single-use account fill authorization.
use serde::{Deserialize, Serialize};

pub const MAX_SCENE_NODES: usize = 256;
pub const MAX_SCENE_DEPTH: usize = 8;
pub const MAX_SELECTOR_BYTES: usize = 256;
pub const MAX_ANCHORS: usize = 7;
pub const MAX_TARGET_BYTES: usize = 1024;
pub const MAX_SCENE_KEY_BYTES: usize = 1024;
pub const UIA_QUERY_BUDGET_MS: u64 = 2000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountFillSettings {
    pub enabled: bool,
    pub show_hint: bool,
    pub lock_minutes: u32,
}
impl Default for AccountFillSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            show_hint: true,
            lock_minutes: 5,
        }
    }
}
impl AccountFillSettings {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if (1..=60).contains(&self.lock_minutes) {
            Ok(())
        } else {
            Err(PolicyError::InvalidSettings)
        }
    }
    pub fn observe_enabled(&self, unlocked: bool) -> bool {
        self.validate().is_ok() && self.enabled && self.show_hint && unlocked
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppIdentity {
    pub canonical_path: String,
    pub file_fingerprint: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowIdentity {
    pub handle: u64,
    pub pid: u32,
    pub process_start: u64,
}
/// Ordered ancestors, nearest parent first. Labels must come from non-input controls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectorAnchor {
    pub automation_id: Option<String>,
    pub control_type: u32,
    pub class_name: Option<String>,
    pub label: Option<String>,
}
/// No RuntimeId, coordinates, input Name or input Value is persistable here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSelector {
    pub automation_id: Option<String>,
    pub control_type: u32,
    pub class_name: Option<String>,
    pub anchors: Vec<SelectorAnchor>,
}
impl FieldSelector {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if self.anchors.len() > MAX_ANCHORS {
            return Err(PolicyError::LimitExceeded);
        }
        if self.control_type == 0 {
            return Err(PolicyError::InvalidSelector);
        }
        validate_optional(&self.automation_id, MAX_SELECTOR_BYTES)?;
        validate_optional(&self.class_name, MAX_SELECTOR_BYTES)?;
        for anchor in &self.anchors {
            if anchor.control_type == 0 {
                return Err(PolicyError::InvalidSelector);
            }
            validate_optional(&anchor.automation_id, MAX_SELECTOR_BYTES)?;
            validate_optional(&anchor.class_name, MAX_SELECTOR_BYTES)?;
            validate_optional(&anchor.label, MAX_SELECTOR_BYTES)?;
        }
        Ok(())
    }
    /// None means no condition; supplied conditions and nearest ancestor order are exact.
    pub fn matches(&self, observed: &Self) -> bool {
        self.validate().is_ok()
            && observed.validate().is_ok()
            && self.control_type == observed.control_type
            && optional_matches(&self.automation_id, &observed.automation_id)
            && optional_matches(&self.class_name, &observed.class_name)
            && self.anchors.len() <= observed.anchors.len()
            && self.anchors.iter().zip(&observed.anchors).all(|(a, b)| {
                a.control_type == b.control_type
                    && optional_matches(&a.automation_id, &b.automation_id)
                    && optional_matches(&a.class_name, &b.class_name)
                    && optional_matches(&a.label, &b.label)
            })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveField {
    /// Request-local identity maintained by the UIA worker, never persisted.
    pub field_id: u64,
    pub selector: FieldSelector,
    pub depth: usize,
    pub is_password: bool,
    pub visible: bool,
    pub enabled: bool,
    pub writable: bool,
}
/// Value read only from a user-selected non-password target control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTarget {
    pub selector: FieldSelector,
    pub value: String,
    pub depth: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneOrigin {
    NativeDesktop,
    UnverifiedBrowser,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneSnapshot {
    /// True only when bounded traversal finished without timeout or truncation.
    pub complete: bool,
    pub app: AppIdentity,
    pub window: WindowIdentity,
    pub scene_key: String,
    pub origin: SceneOrigin,
    pub fields: Vec<LiveField>,
    pub targets: Vec<LiveTarget>,
    pub node_count: usize,
    pub max_depth: usize,
}
impl SceneSnapshot {
    pub fn validate(&self) -> Result<(), PolicyError> {
        if !self.complete {
            return Err(PolicyError::IncompleteScene);
        }
        validate_app(&self.app)?;
        if self.window.handle == 0 || self.window.pid == 0 || self.window.process_start == 0 {
            return Err(PolicyError::InvalidIdentity);
        }
        validate_text(&self.scene_key, MAX_SCENE_KEY_BYTES)?;
        if self.node_count > MAX_SCENE_NODES || self.max_depth > MAX_SCENE_DEPTH {
            return Err(PolicyError::LimitExceeded);
        }
        if self.node_count == 0
            || self.max_depth == 0
            || self.fields.len().saturating_add(self.targets.len()) > self.node_count
        {
            return Err(PolicyError::InvalidScene);
        }
        for (index, field) in self.fields.iter().enumerate() {
            validate_depth(&field.selector, field.depth, self.max_depth)?;
            if field.field_id == 0
                || self.fields[..index]
                    .iter()
                    .any(|f| f.field_id == field.field_id)
            {
                return Err(PolicyError::InvalidField);
            }
        }
        for target in &self.targets {
            validate_depth(&target.selector, target.depth, self.max_depth)?;
            validate_text(&target.value, MAX_TARGET_BYTES)?;
            if self
                .fields
                .iter()
                .any(|field| field.is_password && target.selector.matches(&field.selector))
            {
                return Err(PolicyError::InvalidField);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCondition {
    pub selector: FieldSelector,
    pub expected: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub binding_id: String,
    pub account_id: String,
    pub app: AppIdentity,
    pub scene_key: String,
    pub password: FieldSelector,
    pub username: Option<FieldSelector>,
    pub target: Option<TargetCondition>,
}
impl Binding {
    pub fn validate(&self) -> Result<(), PolicyError> {
        validate_text(&self.binding_id, 96)?;
        validate_text(&self.account_id, 96)?;
        validate_app(&self.app)?;
        validate_text(&self.scene_key, MAX_SCENE_KEY_BYTES)?;
        self.password.validate()?;
        if let Some(username) = &self.username {
            username.validate()?;
            if username == &self.password {
                return Err(PolicyError::InvalidBinding);
            }
        }
        if let Some(target) = &self.target {
            target.selector.validate()?;
            validate_text(&target.expected, MAX_TARGET_BYTES)?;
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedFields {
    pub password_id: u64,
    pub username_id: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchDecision {
    Exact(ResolvedFields),
    ApplicationOnly(ResolvedFields),
    Rejected(PolicyError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    InvalidSettings,
    InvalidIdentity,
    InvalidSelector,
    InvalidBinding,
    InvalidScene,
    IncompleteScene,
    LimitExceeded,
    DifferentApplication,
    DifferentScene,
    AmbiguousField,
    InvalidField,
    TargetMissing,
    TargetConflict,
    UnverifiedBrowser,
    Disabled,
    Locked,
    StaleTicket,
    Cancelled,
    IncompleteTeaching,
    Exhausted,
}
fn validate_text(value: &str, max_bytes: usize) -> Result<(), PolicyError> {
    if value.len() > max_bytes {
        return Err(PolicyError::LimitExceeded);
    }
    if value.trim().is_empty() || value.contains('\0') {
        return Err(PolicyError::InvalidScene);
    }
    Ok(())
}
fn validate_optional(value: &Option<String>, max_bytes: usize) -> Result<(), PolicyError> {
    if let Some(value) = value {
        validate_text(value, max_bytes)?;
    }
    Ok(())
}
fn optional_matches(expected: &Option<String>, observed: &Option<String>) -> bool {
    expected
        .as_ref()
        .is_none_or(|value| observed.as_ref() == Some(value))
}
fn validate_app(app: &AppIdentity) -> Result<(), PolicyError> {
    validate_text(&app.canonical_path, 2048)?;
    let bytes = app.canonical_path.as_bytes();
    let absolute = (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes[2] == b'\\')
        || app.canonical_path.starts_with("\\\\");
    if !absolute || app.file_fingerprint == [0; 32] {
        return Err(PolicyError::InvalidIdentity);
    }
    Ok(())
}
fn validate_depth(
    selector: &FieldSelector,
    depth: usize,
    max_depth: usize,
) -> Result<(), PolicyError> {
    selector.validate()?;
    if depth > MAX_SCENE_DEPTH {
        return Err(PolicyError::LimitExceeded);
    }
    if depth == 0 || depth > max_depth || selector.anchors.len() >= depth {
        return Err(PolicyError::InvalidScene);
    }
    Ok(())
}
fn unique_field<'a>(
    selector: &FieldSelector,
    snapshot: &'a SceneSnapshot,
    password: bool,
) -> Result<&'a LiveField, PolicyError> {
    let mut candidates = snapshot
        .fields
        .iter()
        .filter(|field| selector.matches(&field.selector));
    let field = candidates.next().ok_or(PolicyError::InvalidField)?;
    if candidates.next().is_some() {
        return Err(PolicyError::AmbiguousField);
    }
    if field.is_password != password || !field.visible || !field.enabled || !field.writable {
        return Err(PolicyError::InvalidField);
    }
    Ok(field)
}
fn unique_target<'a>(
    selector: &FieldSelector,
    snapshot: &'a SceneSnapshot,
) -> Result<&'a LiveTarget, PolicyError> {
    let mut candidates = snapshot
        .targets
        .iter()
        .filter(|target| selector.matches(&target.selector));
    let target = candidates.next().ok_or(PolicyError::TargetMissing)?;
    if candidates.next().is_some() {
        return Err(PolicyError::AmbiguousField);
    }
    Ok(target)
}
/// Conservative whitespace normalization only; no substring, case folding or fuzzy match.
pub fn target_matches(expected: &str, observed: Option<&str>) -> bool {
    validate_text(expected, MAX_TARGET_BYTES).is_ok()
        && observed.is_some_and(|value| {
            validate_text(value, MAX_TARGET_BYTES).is_ok() && expected.trim() == value.trim()
        })
}
pub fn match_binding(binding: &Binding, snapshot: &SceneSnapshot) -> MatchDecision {
    match resolve_binding(binding, snapshot) {
        Ok(fields) if binding.target.is_some() && snapshot.origin == SceneOrigin::NativeDesktop => {
            MatchDecision::Exact(fields)
        }
        Ok(fields) => MatchDecision::ApplicationOnly(fields),
        Err(error) => MatchDecision::Rejected(error),
    }
}
fn resolve_binding(
    binding: &Binding,
    snapshot: &SceneSnapshot,
) -> Result<ResolvedFields, PolicyError> {
    binding.validate()?;
    snapshot.validate()?;
    if binding.app != snapshot.app {
        return Err(PolicyError::DifferentApplication);
    }
    if binding.scene_key != snapshot.scene_key {
        return Err(PolicyError::DifferentScene);
    }
    let password_id = unique_field(&binding.password, snapshot, true)?.field_id;
    let username_id = binding
        .username
        .as_ref()
        .map(|selector| unique_field(selector, snapshot, false).map(|field| field.field_id))
        .transpose()?;
    if username_id == Some(password_id) {
        return Err(PolicyError::InvalidField);
    }
    if let Some(condition) = &binding.target {
        let target = unique_target(&condition.selector, snapshot)?;
        if !target_matches(&condition.expected, Some(&target.value)) {
            return Err(PolicyError::TargetConflict);
        }
    }
    Ok(ResolvedFields {
        password_id,
        username_id,
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserFillAction {
    SelectedAccount,
    ConfirmedHint,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillTicket {
    sequence: u64,
    vault_epoch: u64,
    action: UserFillAction,
    binding: Binding,
    snapshot: SceneSnapshot,
}
impl FillTicket {
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    pub fn vault_epoch(&self) -> u64 {
        self.vault_epoch
    }
    pub fn window(&self) -> WindowIdentity {
        self.snapshot.window
    }
    pub fn action(&self) -> UserFillAction {
        self.action
    }
}
#[derive(Default)]
pub struct FillGate {
    sequence: u64,
    active: Option<FillTicket>,
}
impl FillGate {
    /// Call only for an explicit user fill action, never an observation.
    /// A new attempt revokes the previous permission even if this attempt fails.
    pub fn begin(
        &mut self,
        action: UserFillAction,
        settings: &AccountFillSettings,
        vault_epoch: u64,
        unlocked: bool,
        binding: &Binding,
        snapshot: &SceneSnapshot,
    ) -> Result<FillTicket, PolicyError> {
        self.revoke();
        validate_access(settings, unlocked)?;
        let decision = match_binding(binding, snapshot);
        match decision {
            MatchDecision::Rejected(error) => return Err(error),
            MatchDecision::ApplicationOnly(_) if action == UserFillAction::ConfirmedHint => {
                return Err(PolicyError::InvalidBinding);
            }
            _ => {}
        }
        self.sequence = self.sequence.checked_add(1).ok_or(PolicyError::Exhausted)?;
        let ticket = FillTicket {
            sequence: self.sequence,
            vault_epoch,
            action,
            binding: binding.clone(),
            snapshot: snapshot.clone(),
        };
        self.active = Some(ticket.clone());
        Ok(ticket)
    }
    pub fn revoke(&mut self) {
        self.active = None;
    }
    /// Supply a newly collected scene immediately before every actual UIA write.
    /// This does not replace native foreground/window/process checks.
    pub fn validate(
        &self,
        ticket: &FillTicket,
        settings: &AccountFillSettings,
        vault_epoch: u64,
        unlocked: bool,
        snapshot: &SceneSnapshot,
    ) -> Result<ResolvedFields, PolicyError> {
        validate_access(settings, unlocked)?;
        if self.active.as_ref() != Some(ticket)
            || ticket.vault_epoch != vault_epoch
            || &ticket.snapshot != snapshot
        {
            return Err(PolicyError::StaleTicket);
        }
        resolve_binding(&ticket.binding, snapshot)
    }
    /// Take permission once immediately before the final (or only) write.
    /// For username + password, validate before username then consume before password.
    /// Native must still check foreground/cancellation directly at each actual write.
    /// Failed consumption revokes too; never queue another write after consumption.
    pub fn consume(
        &mut self,
        ticket: &FillTicket,
        settings: &AccountFillSettings,
        vault_epoch: u64,
        unlocked: bool,
        snapshot: &SceneSnapshot,
    ) -> Result<ResolvedFields, PolicyError> {
        let result = self.validate(ticket, settings, vault_epoch, unlocked, snapshot);
        self.revoke();
        result
    }
}
fn validate_access(settings: &AccountFillSettings, unlocked: bool) -> Result<(), PolicyError> {
    settings.validate()?;
    if !settings.enabled {
        return Err(PolicyError::Disabled);
    }
    if !unlocked {
        return Err(PolicyError::Locked);
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeachingStep {
    Password,
    Username,
    Target,
    Ready,
    Cancelled,
}
/// Only non-secret selections are retained. Completing this session produces a
/// draft binding; persisting it requires the later vault save receipt.
pub struct TeachingSession {
    snapshot: SceneSnapshot,
    password: Option<FieldSelector>,
    username: Option<FieldSelector>,
    target: Option<TargetCondition>,
    step: TeachingStep,
}
impl TeachingSession {
    pub fn begin(snapshot: &SceneSnapshot) -> Result<Self, PolicyError> {
        snapshot.validate()?;
        Ok(Self {
            snapshot: snapshot.clone(),
            password: None,
            username: None,
            target: None,
            step: TeachingStep::Password,
        })
    }
    pub fn step(&self) -> TeachingStep {
        self.step
    }
    fn require_step(&self, expected: TeachingStep) -> Result<(), PolicyError> {
        if self.step == TeachingStep::Cancelled {
            return Err(PolicyError::Cancelled);
        }
        if self.step != expected {
            return Err(PolicyError::IncompleteTeaching);
        }
        Ok(())
    }
    pub fn select_password(&mut self, field_id: u64) -> Result<(), PolicyError> {
        self.require_step(TeachingStep::Password)?;
        let field = self
            .snapshot
            .fields
            .iter()
            .find(|f| f.field_id == field_id)
            .ok_or(PolicyError::InvalidField)?;
        unique_field(&field.selector, &self.snapshot, true)?;
        self.password = Some(field.selector.clone());
        self.step = TeachingStep::Username;
        Ok(())
    }
    pub fn select_username(&mut self, field_id: Option<u64>) -> Result<(), PolicyError> {
        self.require_step(TeachingStep::Username)?;
        let selector = field_id
            .map(|id| {
                let field = self
                    .snapshot
                    .fields
                    .iter()
                    .find(|f| f.field_id == id)
                    .ok_or(PolicyError::InvalidField)?;
                unique_field(&field.selector, &self.snapshot, false)?;
                Ok(field.selector.clone())
            })
            .transpose()?;
        self.username = selector;
        self.step = TeachingStep::Target;
        Ok(())
    }
    pub fn select_target(&mut self, selector: Option<&FieldSelector>) -> Result<(), PolicyError> {
        self.require_step(TeachingStep::Target)?;
        let target = selector
            .map(|selector| {
                selector.validate()?;
                let observed = unique_target(selector, &self.snapshot)?;
                Ok(TargetCondition {
                    selector: selector.clone(),
                    expected: observed.value.trim().to_owned(),
                })
            })
            .transpose()?;
        self.target = target;
        self.step = TeachingStep::Ready;
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.password = None;
        self.username = None;
        self.target = None;
        self.step = TeachingStep::Cancelled;
    }
    pub fn finish(self, binding_id: String, account_id: String) -> Result<Binding, PolicyError> {
        self.require_step(TeachingStep::Ready)?;
        if self.snapshot.origin == SceneOrigin::UnverifiedBrowser {
            return Err(PolicyError::UnverifiedBrowser);
        }
        let binding = Binding {
            binding_id,
            account_id,
            app: self.snapshot.app.clone(),
            scene_key: self.snapshot.scene_key.clone(),
            password: self.password.ok_or(PolicyError::IncompleteTeaching)?,
            username: self.username,
            target: self.target,
        };
        resolve_binding(&binding, &self.snapshot)?;
        Ok(binding)
    }
}
