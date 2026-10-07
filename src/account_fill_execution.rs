//! Shared execution boundary used by the native UIA worker. No OS input fallback.
use crate::{account_fill::*, account_vault::Secret};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillError {
    Policy(PolicyError),
    Busy,
    Cancelled,
    TargetChanged,
    Unsupported,
    WriteUncertain,
    QueryFailed,
    BudgetExceeded,
    TeachingRequired,
}
impl From<PolicyError> for FillError {
    fn from(value: PolicyError) -> Self {
        Self::Policy(value)
    }
}
impl FillError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unsupported | Self::Policy(PolicyError::InvalidField) => "不支持此输入框",
            Self::WriteUncertain => "无法确认写入结果，请检查目标输入框；不会自动重试",
            Self::Policy(PolicyError::OneTimeOnly) => {
                "此页面较复杂，目前只能本次填入，暂不能记住位置"
            }
            Self::Policy(PolicyError::UnverifiedBrowser) => {
                "网站未验证，请明确点选密码框，仅填这次"
            }
            Self::Busy => "正在识别，请稍后重试",
            Self::TeachingRequired => "请点一下要填写的输入框",
            Self::BudgetExceeded => "页面较复杂，请选择较小范围的输入框",
            Self::Cancelled => "操作已取消",
            _ => "目标已变化，请重新确认输入位置",
        }
    }
}
struct State {
    epoch: AtomicU64,
    generation: AtomicU64,
    busy: AtomicBool,
}
#[derive(Clone)]
pub struct FillControl(Arc<State>);
impl FillControl {
    pub fn new(epoch: u64) -> Self {
        Self(Arc::new(State {
            epoch: AtomicU64::new(epoch),
            generation: AtomicU64::new(1),
            busy: AtomicBool::new(false),
        }))
    }
    /// Synchronous, nonblocking revocation. Call for lock, close, third-party focus,
    /// target/connection change, disable and shutdown. Never frees the actual slot.
    pub fn invalidate(&self, epoch: u64) {
        self.0.generation.fetch_add(1, Ordering::SeqCst);
        self.0.epoch.store(epoch, Ordering::SeqCst);
    }
    pub fn is_busy(&self) -> bool {
        self.0.busy.load(Ordering::SeqCst)
    }
    pub fn reserve(&self, epoch: u64) -> Result<FillLease, FillError> {
        let generation = self.0.generation.load(Ordering::SeqCst);
        if self.0.epoch.load(Ordering::SeqCst) != epoch {
            return Err(FillError::Cancelled);
        }
        self.0
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| FillError::Busy)?;
        let lease = FillLease {
            control: self.clone(),
            epoch,
            generation,
        };
        if !lease.is_current() {
            return Err(FillError::Cancelled);
        }
        Ok(lease)
    }
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::SeqCst)
    }
}
pub struct FillLease {
    control: FillControl,
    epoch: u64,
    generation: u64,
}
impl FillLease {
    pub fn is_current(&self) -> bool {
        self.control.generation() == self.generation
            && self.control.0.epoch.load(Ordering::SeqCst) == self.epoch
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
impl Drop for FillLease {
    fn drop(&mut self) {
        self.control.0.busy.store(false, Ordering::SeqCst);
    }
}
/// Implementations perform a complete bounded sample and directed ValuePattern write.
/// write must recheck native foreground immediately before SetValue.
pub trait FillBackend {
    fn sample(&mut self) -> Result<SceneSnapshot, FillError>;
    fn foreground_matches(&self, window: WindowIdentity) -> bool;
    fn write(&mut self, field: u64, value: &Secret) -> Result<(), FillError>;
}
pub struct FillIntent {
    pub settings: AccountFillSettings,
    pub baseline: SceneSnapshot,
    pub choice: FillChoice,
    pub action: UserFillAction,
    pub browser_password_picked: bool,
    pub username: Secret,
    pub password: Secret,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FillOutcome {
    /// True only for a successful provider return; false does not recall a write.
    pub username_written: bool,
    pub password_written: bool,
    pub error: Option<FillError>,
}
/// Called only for an explicit click with owned temporary credentials. Gate lives
/// on the worker, never queued after consumption. An in-flight OS write cannot
/// be recalled; completed writes remain reported even if cancellation races it.
pub fn execute_fill(
    gate: &mut FillGate,
    backend: &mut impl FillBackend,
    lease: &FillLease,
    intent: FillIntent,
) -> FillOutcome {
    let mut outcome = FillOutcome {
        username_written: false,
        password_written: false,
        error: None,
    };
    let result = (|| -> Result<(), FillError> {
        if !lease.is_current() {
            return Err(FillError::Cancelled);
        }
        let password = intent.password.expose();
        let username = intent.username.expose();
        if password.is_empty()
            || password.len() > 4096
            || password.chars().count() > 1024
            || username.chars().count() > 256
            || password
                .chars()
                .chain(username.chars())
                .any(|c| matches!(c, '\0' | '\r' | '\n'))
        {
            return Err(FillError::Unsupported);
        }
        if intent.baseline.origin == SceneOrigin::UnverifiedBrowser
            && (!intent.browser_password_picked || intent.action != UserFillAction::SelectedAccount)
        {
            return Err(PolicyError::UnverifiedBrowser.into());
        }
        let binding = intent.choice.resolve(&intent.baseline)?;
        let ticket = gate.begin(
            intent.action,
            &intent.settings,
            lease.epoch(),
            true,
            &binding,
            &intent.baseline,
        )?;
        for password in [false, true] {
            if !password && (binding.username.is_none() || username.is_empty()) {
                continue;
            }
            if !lease.is_current() {
                return Err(FillError::Cancelled);
            }
            let sample = backend.sample()?;
            if !lease.is_current() {
                return Err(FillError::Cancelled);
            }
            if !backend.foreground_matches(ticket.window()) {
                return Err(FillError::TargetChanged);
            }
            let fields = if password {
                gate.consume(&ticket, &intent.settings, lease.epoch(), true, &sample)?
            } else {
                gate.validate(&ticket, &intent.settings, lease.epoch(), true, &sample)?
            };
            if !lease.is_current() {
                return Err(FillError::Cancelled);
            }
            if !backend.foreground_matches(ticket.window()) {
                return Err(FillError::TargetChanged);
            }
            if password {
                backend.write(fields.password_id, &intent.password)?;
                outcome.password_written = true;
            } else {
                backend.write(
                    fields.username_id.ok_or(FillError::Unsupported)?,
                    &intent.username,
                )?;
                outcome.username_written = true;
            }
        }
        Ok(())
    })();
    gate.revoke();
    outcome.error = result.err();
    outcome
}

/// Saved bindings and transient teaching results follow different native policies.
#[derive(Debug)]
pub enum FillChoice {
    Saved(Binding),
    Once(OneTimeBinding),
}
impl FillChoice {
    fn resolve(self, snapshot: &SceneSnapshot) -> Result<Binding, FillError> {
        match self {
            Self::Saved(binding) => {
                if snapshot.origin == SceneOrigin::UnverifiedBrowser {
                    return Err(PolicyError::UnverifiedBrowser.into());
                }
                if snapshot.scope == SceneScope::SelectedContext {
                    return Err(PolicyError::OneTimeOnly.into());
                }
                Ok(binding)
            }
            Self::Once(binding) => Ok(binding.into_binding()),
        }
    }
    #[cfg(windows)]
    pub(crate) fn binding(&self) -> &Binding {
        match self {
            Self::Saved(binding) => binding,
            Self::Once(binding) => binding.binding(),
        }
    }
}
