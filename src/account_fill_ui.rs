//! Application-owned account lifecycle; used by native windows and portable consumers.
use crate::{
    account_fill::WindowIdentity, account_vault::VaultSnapshot, account_vault_jobs::VaultController,
};
use std::sync::Arc;
#[derive(Debug, PartialEq, Eq)]
pub enum Access {
    Disabled,
    Reading,
    Uninitialized,
    Locked,
    Busy,
    Ready,
    Failed,
}
pub struct AccountFlow {
    pub vault: VaultController,
    pub snapshot: Option<VaultSnapshot>,
    pub loaded: bool,
    pub target: Option<WindowIdentity>,
    pub error: Option<String>,
    pub exiting: bool,
    read: Option<(u64, u64)>,
    sequence: u64,
}
impl Default for AccountFlow {
    fn default() -> Self {
        Self::new()
    }
}
impl AccountFlow {
    pub fn new() -> Self {
        Self {
            vault: VaultController::new(Arc::new(|| {})),
            snapshot: None,
            loaded: false,
            target: None,
            error: None,
            exiting: false,
            read: None,
            sequence: 0,
        }
    }
    pub fn next_id(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }
    pub fn read_request(&mut self) -> u64 {
        let id = self.next_id();
        self.read = Some((id, self.vault.session().epoch()));
        id
    }
    pub fn loaded(&mut self, id: u64, result: Result<Option<VaultSnapshot>, String>) -> bool {
        if self.read != Some((id, self.vault.session().epoch())) {
            return false;
        }
        self.read = None;
        match result {
            Ok(snapshot) => {
                self.snapshot = snapshot;
                self.loaded = true;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
    pub fn capture(&mut self, target: Option<WindowIdentity>, own: bool) {
        if !own {
            self.target = target;
        }
    }
    pub fn dismiss(&mut self) {
        self.target = None;
        if !self.vault.session().is_unlocked() {
            self.lock();
        }
    }
    pub fn lock(&mut self) {
        self.vault.lock();
        self.target = None;
        self.read = None;
        self.snapshot = None;
        self.loaded = false;
        self.error = None;
    }
    pub fn request_exit(&mut self) {
        self.exiting = true;
        self.lock();
    }
    pub fn shutdown_ready(&self) -> bool {
        self.exiting && self.vault.can_shutdown()
    }
    pub fn access(&self, enabled: bool) -> Access {
        if !enabled {
            Access::Disabled
        } else if self.error.is_some() || self.vault.error().is_some() {
            Access::Failed
        } else if self.vault.busy() || !self.vault.can_shutdown() {
            Access::Busy
        } else if self.vault.session().is_unlocked() {
            Access::Ready
        } else if !self.loaded {
            Access::Reading
        } else if self.snapshot.is_none() {
            Access::Uninitialized
        } else {
            Access::Locked
        }
    }
}
/// Bounded event-driven hint candidates. A different selector requires manual entry.
pub fn hint_candidates(
    bindings: &[crate::account_fill::Binding],
    scene: &crate::account_fill::SceneSnapshot,
) -> Vec<crate::account_fill::Binding> {
    use crate::account_fill::{SceneOrigin, SceneScope};
    if !scene.complete
        || scene.origin != SceneOrigin::NativeDesktop
        || scene.scope != SceneScope::Window
    {
        return vec![];
    }
    let candidates: Vec<_> = bindings
        .iter()
        .filter(|b| b.app == scene.app && b.scene_key == scene.scene_key && b.target.is_some())
        .take(9)
        .cloned()
        .collect();
    if candidates.len() > 8 {
        return vec![];
    }
    if let Some(first) = candidates.first()
        && candidates.iter().any(|b| {
            b.target.as_ref().map(|t| &t.selector) != first.target.as_ref().map(|t| &t.selector)
        })
    {
        return vec![];
    }
    candidates
}
pub fn scene_message(scene: &crate::account_fill::SceneSnapshot) -> &'static str {
    use crate::account_fill::{SceneOrigin, SceneScope};
    if scene.origin == SceneOrigin::UnverifiedBrowser {
        "网站未验证；请点选密码框，只能本次填入，不保存网站关联"
    } else if scene.scope == SceneScope::SelectedContext {
        "此页面较复杂，目前只能本次填入，暂不能记住位置；仅填写所选范围内的字段"
    } else {
        "仅按应用推荐；请确认当前连接，或指定区分目标"
    }
}
pub fn fill_message(outcome: &crate::account_fill_execution::FillOutcome) -> String {
    let fields = match (outcome.username_written, outcome.password_written) {
        (true, true) => "用户名和密码已填入",
        (false, true) => "仅密码已填入",
        (true, false) => "用户名已填入；密码未确认完成",
        (false, false) => "未确认填入完成",
    };
    match outcome.error {
        Some(error) => format!("{fields}。{}", error.message()),
        None => format!("{fields}；未提交登录"),
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum FormProgress {
    Pending,
    Complete,
    Failed(String),
    Cancelled,
}
pub fn form_progress(
    expected_epoch: u64,
    epoch: u64,
    started: u64,
    completed: u64,
    submitted: bool,
    error: Option<&str>,
) -> FormProgress {
    if epoch != expected_epoch {
        FormProgress::Cancelled
    } else if !submitted {
        FormProgress::Pending
    } else if completed != started {
        FormProgress::Complete
    } else if let Some(error) = error {
        FormProgress::Failed(error.into())
    } else {
        FormProgress::Pending
    }
}

/// Shared by the WinEvent producer, hint consumer and MTA final write boundary.
/// A WinEvent is a new observation version even when HWND/object/child repeat:
/// virtual UIA elements do not necessarily have distinct native event identities.
#[derive(Clone)]
pub struct HintClock(Arc<std::sync::atomic::AtomicU64>);
impl Default for HintClock {
    fn default() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicU64::new(1)))
    }
}
impl HintClock {
    pub fn event(&self) {
        let _ = self.0.fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |v| Some(v.saturating_add(1)),
        );
    }
    pub fn version(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn current(&self, version: u64) -> bool {
        version != u64::MAX && self.version() == version
    }
}
/// Non-cloneable, one-click grant. Hiding the native window does not revoke this
/// already consumed grant; subsequent external focus/layout events do.
pub struct HintPermit {
    clock: HintClock,
    version: u64,
}
impl HintPermit {
    pub fn current(&self) -> bool {
        self.clock.current(self.version)
    }
    pub fn matches_focused_password(&self, expected: u64, focused: Option<u64>) -> bool {
        self.current() && focused == Some(expected)
    }
}
pub struct HintChecks {
    clock: HintClock,
    last_check: u64,
    offered: Option<HintPermit>,
}
impl HintChecks {
    pub fn new(clock: HintClock) -> Self {
        Self {
            clock,
            last_check: 0,
            offered: None,
        }
    }
    /// Drain at most one latest version when the actual worker slot is available.
    pub fn take_check(&mut self) -> Option<u64> {
        let v = self.clock.version();
        if v == self.last_check || v == u64::MAX {
            return None;
        }
        self.last_check = v;
        self.offered = None;
        Some(v)
    }
    pub fn offer(&mut self, version: u64) -> bool {
        if self.last_check != version || !self.clock.current(version) {
            return false;
        }
        self.offered = Some(HintPermit {
            clock: self.clock.clone(),
            version,
        });
        true
    }
    pub fn hide(&mut self) {
        self.offered = None;
    }
    pub fn consume(&mut self, clicked_version: u64) -> Option<HintPermit> {
        if self.offered.as_ref()?.version != clicked_version {
            return None;
        }
        let permit = self.offered.take()?;
        permit.current().then_some(permit)
    }
}
