//! Application-lifetime vault and UIA consumers. Native windows own only display drafts.
use super::{
    account_hint::Hint,
    account_picker::{self, PickEvent, PickRole, Picker},
    account_target,
    account_worker::{AccountResult, AccountWorker, FillCommand, Inspection},
    dialogs::{self, AccountFormProgress, AccountFormSignal, text, wide},
    theme,
};
use crate::{
    account_fill::*,
    account_fill_execution::*,
    account_fill_ui::{AccountFlow, HintPermit},
    account_vault::{AccountDraft, Secret, VaultDocument},
    account_vault_jobs::{SaveState, VaultNotice},
    model::{Command, Event, Theme},
};
use std::{cell::RefCell, rc::Rc, time::Instant};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};
pub const TIMER: usize = 794;
#[derive(Clone)]
enum Purpose {
    Manual,
    Saved(Box<Binding>),
    HintBase(u64),
    HintBinding(u64, Vec<Binding>),
}
impl Purpose {
    fn hint_version(&self) -> Option<u64> {
        match self {
            Self::HintBase(version) | Self::HintBinding(version, _) => Some(*version),
            _ => None,
        }
    }
}
struct AfterSave {
    binding: Binding,
    inspection: u64,
    generation: u64,
    epoch: u64,
}
pub struct Runtime {
    pub flow: AccountFlow,
    worker: Option<AccountWorker>,
    pub settings: AccountFillSettings,
    pub appearance: Theme,
    pub owner: HWND,
    pub view: HWND,
    pub open: bool,
    fill_mode: bool,
    pub notice: String,
    pub revision: u64,
    pub completed: u64,
    operation_error: Option<String>,
    attention: Option<String>,
    pub last_active: Instant,
    inspection: Option<(u64, u64, Inspection)>,
    teaching_context: Option<(u64, u64)>,
    purpose: Purpose,
    pending: Option<u64>,
    queued_inspect: Option<Purpose>,
    last_fill: Option<(u64, u64)>,
    selected: Option<String>,
    binding: Option<Binding>,
    password: Option<u64>,
    username: Option<u64>,
    target_selector: Option<FieldSelector>,
    picker: Option<Picker>,
    transfer: bool,
    after_save: Option<AfterSave>,
    hint: Hint,
    hint_binding: Option<Binding>,
    queued: Option<Command>,
    pub shutdown_requested: bool,
    pub shutdown_sent: bool,
    pub shutdown_failed: bool,
}
impl Runtime {
    pub fn new() -> Self {
        Self {
            flow: AccountFlow::new(),
            worker: None,
            settings: AccountFillSettings::default(),
            appearance: Theme::System,
            owner: HWND::default(),
            view: HWND::default(),
            open: false,
            fill_mode: false,
            notice: String::new(),
            revision: 0,
            completed: 0,
            operation_error: None,
            attention: None,
            last_active: Instant::now(),
            inspection: None,
            teaching_context: None,
            purpose: Purpose::Manual,
            pending: None,
            queued_inspect: None,
            last_fill: None,
            selected: None,
            binding: None,
            password: None,
            username: None,
            target_selector: None,
            picker: None,
            transfer: false,
            after_save: None,
            hint: Hint::default(),
            hint_binding: None,
            queued: None,
            shutdown_requested: false,
            shutdown_sent: false,
            shutdown_failed: false,
        }
    }
    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn configure(&mut self, settings: AccountFillSettings, appearance: Theme) {
        if self.settings.enabled && !settings.enabled {
            self.lock();
        }
        if self.settings.show_hint && !settings.show_hint {
            self.hint.stop();
            self.revoke();
        }
        self.settings = settings;
        self.appearance = appearance;
        self.changed();
    }
    pub fn capture(&mut self, target: Option<WindowIdentity>, own: bool) {
        if !own {
            self.revoke();
            self.flow.capture(target, false);
        }
    }
    pub fn target_cancelled(&mut self, generation: u64) {
        if self
            .worker
            .as_ref()
            .is_none_or(|w| w.control().generation() != generation)
        {
            return;
        }
        self.revoke();
        self.flow.target = None;
        self.notice = "目标已切换，请重新呼出或指定密码框".into();
    }
    pub fn revoke(&mut self) {
        self.hint
            .guard(None, self.flow.vault.session().epoch(), None);
        if let Some(worker) = &self.worker {
            worker
                .control()
                .invalidate(self.flow.vault.session().epoch());
        }
        self.picker = None;
        self.inspection = None;
        self.teaching_context = None;
        self.pending = None;
        self.queued_inspect = None;
        self.password = None;
        self.username = None;
        self.target_selector = None;
        self.binding = None;
        self.after_save = None;
        self.hint_binding = None;
        self.hint.hide();
        self.transfer = false;
        self.changed();
    }
    pub fn dismiss(&mut self) {
        self.hint.request_check();
        self.revoke();
        self.flow.dismiss();
        self.selected = None;
        self.open = false;
        self.changed();
    }
    pub fn lock(&mut self) {
        self.flow.lock();
        unsafe {
            if !self.view.is_invalid() {
                if let Ok(list) = GetDlgItem(Some(self.view), 11) {
                    SendMessageW(list, LB_RESETCONTENT, None, None);
                }
                if let Ok(search) = GetDlgItem(Some(self.view), 10) {
                    let _ = SetWindowTextW(search, w!(""));
                }
            }
        }
        self.revoke();
        self.selected = None;
        self.hint.stop();
        self.notice = "保险箱已锁定".into();
        self.changed();
    }
    pub fn exit(&mut self) {
        if !self.flow.exiting {
            self.flow.request_exit();
            self.shutdown_failed = matches!(
                self.flow.vault.state(),
                SaveState::FailedPreparation | SaveState::FailedWrite
            );
            self.revoke();
            self.hint.stop();
            unsafe {
                if !self.view.is_invalid() {
                    if let Ok(list) = GetDlgItem(Some(self.view), 11) {
                        SendMessageW(list, LB_RESETCONTENT, None, None);
                    }
                    if let Ok(search) = GetDlgItem(Some(self.view), 10) {
                        let _ = SetWindowTextW(search, w!(""));
                    }
                }
            }
            self.changed();
        }
    }
    pub fn cancel_exit(&mut self) {
        self.flow.exiting = false;
        self.shutdown_requested = false;
        self.shutdown_sent = false;
        self.shutdown_failed = false;
        self.changed();
    }
    pub fn queue_read(&mut self) {
        self.queued = self.read();
        self.changed();
    }
    fn read(&mut self) -> Option<Command> {
        if self.flow.vault.session().is_unlocked() || !self.flow.vault.can_shutdown() {
            return None;
        }
        let id = self.flow.read_request();
        self.notice = "正在读取保险箱…".into();
        Some(Command::ReadAccountVault { request_id: id })
    }
    pub fn event(&mut self, event: Event) {
        if let Event::AccountVaultLoaded { request_id, result } = event {
            if self.flow.loaded(request_id, result) {
                self.notice = self.flow.error.clone().unwrap_or_else(|| {
                    if self.flow.snapshot.is_none() {
                        "尚未创建保险箱；主密码无法找回".into()
                    } else {
                        "输入主密码解锁".into()
                    }
                });
                self.changed();
            }
            return;
        }
        if let Some(notice) = self.flow.vault.handle_event(&event) {
            self.vault_notice(notice);
        }
    }
    fn vault_notice(&mut self, notice: VaultNotice) {
        match notice {
            VaultNotice::Unlocked => {
                self.completed += 1;
                self.last_active = Instant::now();
                self.notice = "已解锁；请选择账号并确认目标".into();
                if self.fill_mode {
                    self.start_inspect(Purpose::Manual);
                }
            }
            VaultNotice::Saved => {
                self.completed += 1;
                self.last_active = Instant::now();
                self.notice = "已保存".into();
                if let Some(continuation) = self.after_save.take()
                    && self.flow.vault.session().is_unlocked()
                    && self.flow.vault.session().epoch() == continuation.epoch
                    && self
                        .worker
                        .as_ref()
                        .is_some_and(|w| w.control().generation() == continuation.generation)
                    && self
                        .inspection
                        .as_ref()
                        .is_some_and(|(id, _, _)| *id == continuation.inspection)
                {
                    self.do_fill(
                        FillChoice::Saved(continuation.binding),
                        UserFillAction::SelectedAccount,
                        None,
                    );
                } else if self.fill_mode && self.open && self.inspection.is_none() {
                    self.start_inspect(Purpose::Manual);
                }
            }
            VaultNotice::Failed(message) => {
                self.operation_error = Some(message.clone());
                self.notice = message;
                self.shutdown_failed = self.flow.exiting;
            }
            VaultNotice::Prepared | VaultNotice::Stale => (),
        }
        self.changed();
    }
    /// Called on the application's timer even while Exiting and from native modal pumps.
    pub fn tick(&mut self, mut send: impl FnMut(Command) -> Result<(), String>) {
        if let Some(command) = self.queued.take()
            && let Err(e) = send(command)
        {
            self.notice = e.clone();
            self.flow.error = Some(e);
            self.changed();
        }
        while let Some(notice) = self.flow.vault.poll() {
            self.vault_notice(notice);
        }
        if self.flow.vault.state() == SaveState::Ready
            && let Err(message) = self.flow.vault.dispatch_save(&mut send)
        {
            self.notice = message;
            self.shutdown_failed = self.flow.exiting;
            self.changed();
        }
        if self.flow.vault.session().is_unlocked()
            && self.last_active.elapsed().as_secs() >= u64::from(self.settings.lock_minutes) * 60
        {
            self.lock();
        }
        while let Some(reply) = self.worker.as_ref().and_then(AccountWorker::try_recv) {
            if self.pending != Some(reply.request_id) {
                if self.last_fill == Some((reply.request_id, reply.generation))
                    && let AccountResult::Filled(outcome) = reply.result
                {
                    self.last_fill = None;
                    self.present_fill(&outcome, true);
                }
                continue;
            }
            self.pending = None;
            match reply.result {
                AccountResult::Filled(outcome) => {
                    self.last_fill = None;
                    self.present_fill(&outcome, false);
                    self.revoke();
                    if outcome.error.is_none() && outcome.password_written {
                        self.open = false;
                        self.flow.target = None;
                        self.selected = None;
                    } else {
                        self.transfer = false;
                    }
                }
                AccountResult::Failed(error) => {
                    if self.last_fill == Some((reply.request_id, reply.generation))
                        && !self.open
                        && !matches!(error, FillError::Cancelled | FillError::TargetChanged)
                    {
                        self.attention = Some(format!("本次账号填入未完成：{}", error.message()));
                    }
                    // A failed bounded sample is never an inspection or a fill grant.
                    // Retain its identity solely so explicit teaching can narrow that context.
                    self.teaching_context = (error == FillError::BudgetExceeded
                        && self
                            .worker
                            .as_ref()
                            .is_some_and(|w| w.control().generation() == reply.generation))
                    .then_some((reply.request_id, reply.generation));
                    self.inspection = None;
                    self.binding = None;
                    self.password = None;
                    self.username = None;
                    self.notice = error.message().into();
                    self.transfer = false;
                }
                AccountResult::Inspected(inspection) => {
                    self.inspected(reply.request_id, reply.generation, inspection, None)
                }
                AccountResult::Taught(point) => self.inspected(
                    reply.request_id,
                    reply.generation,
                    point.inspection,
                    Some((point.role, point.field_id)),
                ),
            }
            self.changed();
        }
        if self.pending.is_none()
            && self.worker.as_ref().is_none_or(|w| !w.is_busy())
            && let Some(purpose) = self.queued_inspect.take()
        {
            self.start_inspect(purpose);
        }
        if let Some(picker) = self.picker.as_mut() {
            match picker.poll() {
                PickEvent::Waiting => (),
                PickEvent::Cancelled => {
                    self.picker = None;
                    self.revoke();
                    self.notice = "已取消指定位置".into();
                }
                PickEvent::Selected(point) => {
                    self.picker = None;
                    self.picked(point);
                }
            }
        }
        if self.flow.target.is_some()
            && (self.inspection.is_some()
                || self.teaching_context.is_some()
                || self.pending.is_some()
                || self.picker.is_some()
                || self.after_save.is_some())
        {
            let foreground = unsafe { GetForegroundWindow() };
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(foreground, Some(&mut pid));
            }
            let own = pid == unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
            let target = self.flow.target.unwrap();
            if account_target::window_identity(target.handle) != Ok(target)
                || (!own && !account_target::foreground_matches(target))
            {
                self.revoke();
                self.flow.target = None;
                self.notice = "目标已切换，请重新呼出账号填充".into();
            }
        }
        let observing = self
            .settings
            .observe_enabled(self.flow.vault.session().is_unlocked())
            && !self.flow.exiting;
        let watching = self.settings.enabled
            && self.flow.vault.session().is_unlocked()
            && !self.flow.exiting
            && (observing || self.open || self.pending.is_some() || self.picker.is_some());
        self.hint.observe(self.owner, watching);
        self.hint.guard(
            self.worker.as_ref().map(AccountWorker::control),
            self.flow.vault.session().epoch(),
            self.flow.target,
        );
        if self.hint.changed() {
            self.hint.hide();
            self.hint_binding = None;
        }
        if observing
            && !self.open
            && self.pending.is_none()
            && self.picker.is_none()
            && self.worker.as_ref().is_none_or(|w| !w.is_busy())
            && let Some(version) = self.hint.begin_check()
            && let Ok(target) = account_target::capture_foreground(unsafe {
                windows::Win32::System::Threading::GetCurrentProcessId()
            })
        {
            let focus = account_target::focus_handle(target).unwrap_or(0) as usize;
            self.hint.target(target.handle, focus);
            self.revoke();
            self.flow.target = Some(target);
            self.start_inspect(Purpose::HintBase(version));
        }
        if self.shutdown_requested && self.flow.shutdown_ready() && !self.shutdown_sent {
            match send(Command::Shutdown) {
                Ok(()) => self.shutdown_sent = true,
                Err(e) => {
                    self.notice = e;
                    self.shutdown_failed = true;
                    self.changed();
                }
            }
        }
    }
    fn ensure_worker(&mut self) -> bool {
        if self.worker.is_none() {
            match AccountWorker::new(self.flow.vault.session().epoch()) {
                Ok(worker) => self.worker = Some(worker),
                Err(e) => {
                    self.notice = e.message().into();
                    return false;
                }
            }
        }
        true
    }
    fn start_inspect(&mut self, purpose: Purpose) {
        if purpose
            .hint_version()
            .is_some_and(|v| !self.hint.current(v))
        {
            return;
        }
        if !self.settings.enabled
            || !self.flow.vault.session().is_unlocked()
            || !self.ensure_worker()
        {
            return;
        }
        let Some(target) = self.flow.target else {
            self.notice = "请点击“重新指定密码框”，再点一下要填写的输入框".into();
            return;
        };
        if self.pending.is_some() || self.worker.as_ref().is_some_and(|w| w.is_busy()) {
            self.queued_inspect = Some(purpose);
            self.notice = "正在等待上一项识别完成…".into();
            self.changed();
            return;
        }
        let worker = self.worker.as_mut().unwrap();
        let result = match &purpose {
            Purpose::Saved(binding) => worker.inspect_binding(
                self.flow.vault.session().epoch(),
                target,
                (**binding).clone(),
            ),
            Purpose::HintBinding(_, bindings) => worker.inspect_binding(
                self.flow.vault.session().epoch(),
                target,
                bindings[0].clone(),
            ),
            _ => worker.inspect(self.flow.vault.session().epoch(), target),
        };
        match result {
            Ok(id) => {
                self.inspection = None;
                self.teaching_context = None;
                self.password = None;
                self.username = None;
                self.binding = None;
                self.pending = Some(id);
                self.purpose = purpose;
                self.notice = "正在识别目标…".into();
            }
            Err(FillError::Busy) => {
                self.queued_inspect = Some(purpose);
                self.notice = "正在等待识别槽释放…".into();
            }
            Err(e) => self.notice = e.message().into(),
        }
        self.changed();
    }
    fn inspected(
        &mut self,
        id: u64,
        generation: u64,
        inspection: Inspection,
        picked: Option<(PickRole, u64)>,
    ) {
        if self
            .worker
            .as_ref()
            .is_none_or(|w| w.control().generation() != generation)
        {
            return;
        }
        if picked.is_none()
            && self
                .purpose
                .hint_version()
                .is_some_and(|v| !self.hint.current(v))
        {
            return;
        }
        if let Some((role, field)) = picked {
            match role {
                PickRole::Password => {
                    self.password = Some(field);
                    self.username = inspection.suggested_username;
                    self.target_selector = None;
                    self.binding = None;
                }
                PickRole::Username => self.username = Some(field),
                PickRole::Target => {
                    self.target_selector = inspection
                        .snapshot
                        .targets
                        .first()
                        .map(|t| t.selector.clone())
                }
            }
        } else {
            self.password = inspection.suggested_password;
            self.username = inspection.suggested_username;
            self.target_selector = None;
        }
        self.notice = crate::account_fill_ui::scene_message(&inspection.snapshot).into();
        self.teaching_context = None;
        self.inspection = Some((id, generation, inspection.clone()));
        self.transfer = false;
        if picked.is_some() {
            return;
        }
        match self.purpose.clone() {
            Purpose::Manual => {}
            Purpose::Saved(binding) => match match_binding(&binding, &inspection.snapshot) {
                MatchDecision::Exact(fields) => {
                    self.password = Some(fields.password_id);
                    self.username = fields.username_id;
                    self.notice = "已核验应用与目标".into();
                    self.target_selector = binding.target.as_ref().map(|t| t.selector.clone());
                    self.binding = Some(*binding);
                }
                MatchDecision::ApplicationOnly(fields) => {
                    self.password = Some(fields.password_id);
                    self.username = fields.username_id;
                    self.notice = "仅按应用推荐，请确认当前连接".into();
                    self.target_selector = binding.target.as_ref().map(|t| t.selector.clone());
                    self.binding = Some(*binding);
                }
                MatchDecision::Rejected(_) => {
                    self.binding = None;
                    self.notice = "关联与当前目标不符；请重新指定位置".into();
                }
            },
            Purpose::HintBase(version) => {
                if let Ok(document) = self.flow.vault.session().document() {
                    let candidates = crate::account_fill_ui::hint_candidates(
                        &document.bindings,
                        &inspection.snapshot,
                    );
                    if !candidates.is_empty() {
                        self.start_inspect(Purpose::HintBinding(version, candidates));
                    }
                }
            }
            Purpose::HintBinding(version, bindings) => {
                let matches: Vec<_> = bindings
                    .into_iter()
                    .filter_map(|b| match match_binding(&b, &inspection.snapshot) {
                        MatchDecision::Exact(fields) => Some((b, fields)),
                        _ => None,
                    })
                    .collect();
                if let [(binding, fields)] = matches.as_slice()
                    && let Some(bounds) = inspection.password_bounds
                    && bounds.field_id == fields.password_id
                    && self
                        .settings
                        .observe_enabled(self.flow.vault.session().is_unlocked())
                    && account_target::foreground_matches(inspection.snapshot.window)
                    && self.hint.show(bounds, self.appearance, version)
                {
                    self.hint_binding = Some(binding.clone());
                }
            }
        }
    }
    pub fn take_attention(&mut self) -> Option<String> {
        self.attention.take()
    }
    fn present_fill(&mut self, outcome: &FillOutcome, stale: bool) {
        let wrote = outcome.username_written || outcome.password_written;
        let uncertain = outcome.error == Some(FillError::WriteUncertain);
        if stale {
            if wrote || uncertain {
                self.attention = Some(format!(
                    "之前已取消的账号填入：{}",
                    crate::account_fill_ui::fill_message(outcome)
                ));
            }
            return;
        }
        self.notice = crate::account_fill_ui::fill_message(outcome);
        let cancelled_without_write = !wrote
            && matches!(
                outcome.error,
                Some(FillError::Cancelled | FillError::TargetChanged)
            );
        if !self.open
            && !cancelled_without_write
            && (outcome.error.is_some() || !outcome.password_written)
        {
            self.attention = Some(format!("本次账号填入：{}", self.notice));
        }
    }
    pub fn hint_click(&mut self, clicked_version: u64) {
        if !self
            .settings
            .observe_enabled(self.flow.vault.session().is_unlocked())
        {
            return;
        }
        let Some(permit) = self.hint.consume(clicked_version) else {
            return;
        };
        if let Some(binding) = self.hint_binding.take()
            && self.inspection.as_ref().is_some_and(|(_, g, i)| {
                self.worker
                    .as_ref()
                    .is_some_and(|w| w.control().generation() == *g)
                    && account_target::foreground_matches(i.snapshot.window)
                    && matches!(
                        match_binding(&binding, &i.snapshot),
                        MatchDecision::Exact(_)
                    )
            })
        {
            self.selected = Some(binding.account_id.clone());
            self.do_fill(
                FillChoice::Saved(binding),
                UserFillAction::ConfirmedHint,
                Some(permit),
            );
        }
    }
    fn select(&mut self, id: String) {
        self.last_active = Instant::now();
        self.selected = Some(id.clone());
        self.binding = None;
        if let Some((_, _, inspection)) = &self.inspection
            && let Ok(document) = self.flow.vault.session().document()
        {
            let candidates: Vec<_> = document
                .bindings
                .iter()
                .filter(|b| {
                    b.account_id == id
                        && b.app == inspection.snapshot.app
                        && b.scene_key == inspection.snapshot.scene_key
                })
                .cloned()
                .collect();
            if let [binding] = candidates.as_slice() {
                self.start_inspect(Purpose::Saved(Box::new(binding.clone())));
            } else {
                self.notice = crate::account_fill_ui::scene_message(&inspection.snapshot).into();
            }
        }
        self.changed();
    }
    fn teaching(&self) -> Result<TeachingSession, String> {
        let (_, _, inspection) = self.inspection.as_ref().ok_or("请先指定密码框")?;
        let mut teaching =
            TeachingSession::begin(&inspection.snapshot).map_err(|_| "此目标无法安全识别")?;
        teaching
            .select_password(self.password.ok_or("请先指定密码框")?)
            .map_err(|_| "密码框不唯一，请重新指定")?;
        teaching
            .select_username(self.username)
            .map_err(|_| "用户名框已改变，请重新指定")?;
        teaching
            .select_target(self.target_selector.as_ref())
            .map_err(|_| "区分目标已改变，请重新指定")?;
        Ok(teaching)
    }
    fn fill(&mut self, remember: bool) {
        let result = (|| -> Result<(), String> {
            if !self.settings.enabled
                || !self.flow.vault.session().is_unlocked()
                || self.pending.is_some()
                || !self.flow.vault.can_shutdown()
            {
                return Err("请先启用、解锁并等待当前操作完成".into());
            }
            let account = self.selected.clone().ok_or("请选择账号")?;
            let id = self
                .binding
                .as_ref()
                .map(|b| b.binding_id.clone())
                .unwrap_or_else(|| format!("binding-{}-{}", timestamp(), self.flow.next_id()));
            if remember {
                let binding = self
                    .teaching()?
                    .finish(id, account)
                    .map_err(|e| FillError::Policy(e).message())?;
                if let Some(confirmed) = self
                    .flow
                    .vault
                    .session()
                    .document()?
                    .equivalent_binding(&binding)
                    .cloned()
                {
                    self.do_fill(
                        FillChoice::Saved(confirmed),
                        UserFillAction::SelectedAccount,
                        None,
                    );
                    return Ok(());
                }
                let mut document = self.flow.vault.session().document()?.clone();
                document.upsert_binding(binding.clone())?;
                let request = self.flow.next_id();
                self.flow.vault.accept_save(request, document)?;
                let (inspection, generation, _) = self.inspection.as_ref().ok_or("目标已失效")?;
                self.after_save = Some(AfterSave {
                    binding,
                    inspection: *inspection,
                    generation: *generation,
                    epoch: self.flow.vault.session().epoch(),
                });
                self.notice = "正在保存关联；保存成功后填入仍有效的目标…".into();
            } else {
                let choice = if let Some(binding) = &self.binding {
                    FillChoice::Saved(binding.clone())
                } else {
                    FillChoice::Once(
                        self.teaching()?
                            .finish_once(id, account)
                            .map_err(|e| FillError::Policy(e).message())?,
                    )
                };
                self.do_fill(choice, UserFillAction::SelectedAccount, None);
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.notice = e;
        }
        self.changed();
    }
    fn do_fill(&mut self, choice: FillChoice, action: UserFillAction, permit: Option<HintPermit>) {
        let result = (|| -> Result<(), String> {
            if !self.settings.enabled || !self.flow.vault.session().is_unlocked() {
                return Err("保险箱已锁定或填充已停用".into());
            }
            let (id, _, inspection) = self.inspection.as_ref().ok_or("目标已失效")?;
            let (username, password) = self
                .flow
                .vault
                .session()
                .credentials(self.selected.as_deref().ok_or("请选择账号")?)?;
            self.hint.hide();
            self.transfer = true;
            if action != UserFillAction::ConfirmedHint {
                unsafe {
                    if !self.view.is_invalid() {
                        let _ = ShowWindow(self.view, SW_HIDE);
                    }
                    let _ = ShowWindow(self.owner, SW_HIDE);
                    let _ = SetForegroundWindow(HWND(inspection.snapshot.window.handle as *mut _));
                }
            }
            if !account_target::foreground_matches(inspection.snapshot.window) {
                return Err("无法返回原目标，请重新呼出".into());
            }
            let command = FillCommand {
                inspection_id: *id,
                expected_window: inspection.snapshot.window,
                choice,
                settings: self.settings.clone(),
                action,
                username,
                password,
            };
            let worker = self.worker.as_mut().ok_or("无法识别输入框")?;
            let request = if let Some(permit) = permit {
                worker.fill_hint(self.flow.vault.session().epoch(), command, permit)
            } else {
                worker.fill(self.flow.vault.session().epoch(), command)
            }
            .map_err(|e| e.message())?;
            self.last_fill = Some((
                request,
                self.worker.as_ref().unwrap().control().generation(),
            ));
            self.pending = Some(request);
            self.last_active = Instant::now();
            self.notice = "正在填入，不会提交登录…".into();
            Ok(())
        })();
        if let Err(e) = result {
            self.transfer = false;
            self.notice = e;
        }
        self.changed();
    }
    fn picked(&mut self, point: account_picker::PickPoint) {
        if self.teaching_context.is_some_and(|(_, generation)| {
            self.worker
                .as_ref()
                .is_none_or(|w| w.control().generation() != generation)
        }) {
            self.revoke();
            self.notice = FillError::TargetChanged.message().into();
            return;
        }
        self.flow.target = Some(point.window);
        unsafe {
            let _ = SetForegroundWindow(HWND(point.window.handle as *mut _));
        }
        let id = self
            .inspection
            .as_ref()
            .map(|(id, _, _)| *id)
            .or_else(|| self.teaching_context.map(|(id, _)| id));
        if let Some(worker) = &mut self.worker {
            match worker.teach_point(self.flow.vault.session().epoch(), id, point) {
                Ok(request) => {
                    self.pending = Some(request);
                    self.notice = "正在确认所选输入框…".into();
                }
                Err(error) => {
                    self.notice = error.message().into();
                    self.transfer = false;
                }
            }
        }
        self.changed();
    }
    fn pick(&mut self, role: PickRole) {
        if !self.settings.enabled
            || !self.flow.vault.session().is_unlocked()
            || self.pending.is_some()
            || !self.ensure_worker()
        {
            return;
        }
        self.after_save = None;
        self.binding = None;
        self.hint.hide();
        self.transfer = true;
        unsafe {
            let _ = ShowWindow(self.view, SW_HIDE);
            let _ = ShowWindow(self.owner, SW_HIDE);
            if let Some(target) = self.flow.target {
                let _ = SetForegroundWindow(HWND(target.handle as *mut _));
            }
        }
        match account_picker::pick(self.owner.0 as u64, self.flow.target, role, self.appearance) {
            Ok(picker) => {
                self.picker = Some(picker);
                self.notice = "点选目标；Esc 或右键取消".into();
            }
            Err(e) => {
                self.notice = e.message().into();
                self.transfer = false;
            }
        }
        self.changed();
    }
    pub fn retry(&mut self) {
        match self.flow.vault.retry() {
            Ok(()) => {
                self.shutdown_failed = false;
                self.notice = "正在重试原保存请求…".into();
            }
            Err(e) => {
                self.notice = e;
                self.shutdown_failed = self.flow.exiting;
            }
        }
        self.changed();
    }
    pub fn abandon(&mut self) {
        match self.flow.vault.discard_failed_save() {
            Ok(()) => {
                self.revoke();
                self.flow.snapshot = None;
                self.flow.loaded = false;
                self.notice = "已放弃失败的修改，请重新解锁".into();
            }
            Err(e) => self.notice = e,
        }
        self.changed();
    }
    fn title(&self) -> String {
        if let Some((_, _, i)) = &self.inspection {
            let app = i
                .snapshot
                .app
                .canonical_path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or("目标应用");
            format!(
                "将填入：{app} / {}",
                if self.binding.as_ref().is_some_and(|b| b.target.is_some()) {
                    "已核验目标"
                } else {
                    "请确认当前页面"
                }
            )
        } else if self.flow.target.is_some() {
            "将填入：已捕获的应用 / 正在确认目标".into()
        } else {
            "将填入：尚未指定，请点选密码框".into()
        }
    }
}
fn timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
#[derive(Clone)]
enum EditKind {
    Unlock,
    Create,
    Account(Option<String>),
    Master,
}
fn operation(
    runtime: &Rc<RefCell<Runtime>>,
    kind: &EditKind,
    values: &[String],
) -> Result<(), String> {
    let mut r = runtime.borrow_mut();
    r.last_active = Instant::now();
    r.operation_error = None;
    if r.flow.exiting {
        return Err("正在退出，未接收新修改".into());
    }
    if matches!(
        r.flow.vault.state(),
        SaveState::FailedWrite | SaveState::FailedPreparation
    ) {
        r.flow.vault.retry()?;
        r.notice = "正在重试原保存请求…".into();
        return Ok(());
    }
    if matches!(kind, EditKind::Create | EditKind::Master) {
        let count = values[0].chars().count();
        if !(12..=256).contains(&count) || values[0].len() > 1024 {
            return Err("主密码须为12–256字符，且不超过1024字节".into());
        }
        if values[0] != values[1] {
            return Err("两次主密码不一致".into());
        }
    }
    let id = r.flow.next_id();
    match kind {
        EditKind::Unlock => {
            let snapshot = r.flow.snapshot.clone().ok_or("尚未读取保险箱")?;
            let epoch = r.flow.vault.session().epoch();
            r.flow
                .vault
                .begin_unlock(epoch, Secret::new(values[0].clone()), snapshot)?;
            r.notice = "正在解锁…".into();
        }
        EditKind::Create => {
            r.flow.vault.accept_create(
                id,
                Secret::new(values[0].clone()),
                Secret::new(values[1].clone()),
                VaultDocument::default(),
            )?;
            r.notice = "正在创建并保存保险箱…".into();
        }
        EditKind::Master => {
            r.flow.vault.accept_change_master(
                id,
                Secret::new(values[0].clone()),
                Secret::new(values[1].clone()),
            )?;
            r.notice = "正在更改主密码…".into();
        }
        EditKind::Account(account) => {
            let mut document = r.flow.vault.session().document()?.clone();
            document.upsert_account(AccountDraft {
                id: account
                    .clone()
                    .unwrap_or_else(|| format!("account-{}-{id}", timestamp())),
                name: values[0].clone(),
                username: values[1].clone(),
                password: Secret::new(values[2].clone()),
            })?;
            r.flow.vault.accept_save(id, document)?;
            r.notice = "正在加密并保存账号…".into();
        }
    }
    r.changed();
    Ok(())
}
unsafe fn edit(runtime: &Rc<RefCell<Runtime>>, owner: HWND, kind: EditKind) {
    unsafe {
        let (appearance, epoch, completed) = {
            let r = runtime.borrow();
            (r.appearance, r.flow.vault.session().epoch(), r.completed)
        };
        let mut fields: Vec<(&str, String, bool)> = match &kind {
            EditKind::Unlock => vec![("主密码", String::new(), true)],
            EditKind::Create | EditKind::Master => vec![
                ("主密码（12–256 字符，无法找回）", String::new(), true),
                ("再次输入主密码", String::new(), true),
            ],
            EditKind::Account(id) => {
                let r = runtime.borrow();
                let account = r
                    .flow
                    .vault
                    .session()
                    .document()
                    .ok()
                    .and_then(|d| d.accounts.iter().find(|a| Some(&a.id) == id.as_ref()));
                vec![
                    (
                        "账号名称（1–96 字符）",
                        account.map(|a| a.name.clone()).unwrap_or_default(),
                        false,
                    ),
                    (
                        "用户名（可留空）",
                        account.map(|a| a.username.clone()).unwrap_or_default(),
                        false,
                    ),
                    ("密码（编辑时留空保留原密码）", String::new(), true),
                ]
            }
        };
        let title = match kind {
            EditKind::Unlock => "解锁账号保险箱",
            EditKind::Create => "创建账号保险箱",
            EditKind::Master => "更改主密码",
            EditKind::Account(_) => "编辑账号",
        };
        let mut submitted = false;
        dialogs::account_form(owner, title, &fields, appearance, |signal| {
            if runtime.borrow().flow.vault.session().epoch() != epoch {
                return AccountFormProgress::Cancelled;
            }
            if matches!(signal, AccountFormSignal::Activity) {
                runtime.borrow_mut().last_active = Instant::now();
            }
            if let AccountFormSignal::Submit(values) = signal {
                match operation(runtime, &kind, values) {
                    Ok(()) => submitted = true,
                    Err(e) => return AccountFormProgress::Failed(e),
                }
            }
            let r = runtime.borrow();
            crate::account_fill_ui::form_progress(
                epoch,
                r.flow.vault.session().epoch(),
                completed,
                r.completed,
                submitted,
                r.operation_error
                    .as_deref()
                    .or_else(|| r.flow.vault.error()),
            )
        });
        for (_, value, _) in &mut fields {
            zeroize::Zeroize::zeroize(value);
        }
        if matches!(kind, EditKind::Unlock | EditKind::Create)
            && !runtime.borrow().flow.vault.session().is_unlocked()
        {
            runtime.borrow_mut().lock();
        }
    }
}
struct WindowState {
    done: bool,
    action: usize,
    changed: bool,
    resize: bool,
    dpi: u32,
    colors: theme::Colors,
    font: HFONT,
    suggested: Option<RECT>,
}
impl Drop for WindowState {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                (*(lp.0 as *const CREATESTRUCTW)).lpCreateParams as isize,
            );
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<WindowState>;
        if !ptr.is_null() {
            match msg {
                WM_ACTIVATE if (wp.0 & 0xffff) == WA_INACTIVE as usize => {
                    (*ptr).borrow_mut().action = 6;
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    (*ptr).borrow_mut().done = true;
                    return LRESULT(0);
                }
                WM_COMMAND => {
                    let id = wp.0 & 0xffff;
                    let code = (wp.0 >> 16) as u32;
                    let mut s = (*ptr).borrow_mut();
                    if id == 10 && code == EN_CHANGE {
                        s.changed = true;
                    } else if id == 11 && code == LBN_SELCHANGE {
                        s.action = 11;
                    } else if (1..=5).contains(&id) {
                        s.action = id;
                    }
                    return LRESULT(0);
                }
                WM_SIZE => {
                    (*ptr).borrow_mut().resize = true;
                    return LRESULT(0);
                }
                WM_DPICHANGED => {
                    let mut s = (*ptr).borrow_mut();
                    s.dpi = (wp.0 & 0xffff) as u32;
                    s.suggested = Some(*(lp.0 as *const RECT));
                    s.resize = true;
                    return LRESULT(0);
                }
                WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                    let s = (*ptr).borrow();
                    SetTextColor(HDC(wp.0 as *mut _), s.colors.palette.text);
                    SetBkColor(HDC(wp.0 as *mut _), s.colors.palette.background);
                    return LRESULT(s.colors.background.0 as isize);
                }
                WM_ERASEBKGND => {
                    let s = (*ptr).borrow();
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(HDC(wp.0 as *mut _), &rect, s.colors.background);
                    return LRESULT(1);
                }
                _ => (),
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
unsafe fn control(hwnd: HWND, class: PCWSTR, label: &str, id: usize, style: WINDOW_STYLE) -> HWND {
    unsafe {
        let label = wide(label);
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(label.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            1,
            1,
            Some(hwnd),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )
        .unwrap_or_default()
    }
}
/// Own modal pump; App/Runtime are never borrowed across dispatch or form callbacks.
pub unsafe fn show(owner: HWND, runtime: Rc<RefCell<Runtime>>, fill_mode: bool) {
    unsafe {
        let appearance = runtime.borrow().appearance;
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("QuickPanelAccounts");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        });
        let dpi = GetDpiForWindow(owner).max(96);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
            &mut info,
        );
        let width = (640 * dpi as i32 / 96).min(info.rcWork.right - info.rcWork.left);
        let height = (560 * dpi as i32 / 96).min(info.rcWork.bottom - info.rcWork.top);
        let state = Box::new(RefCell::new(WindowState {
            done: false,
            action: 0,
            changed: true,
            resize: true,
            dpi,
            colors: theme::Colors::new(appearance),
            font: theme::font(dpi),
            suggested: None,
        }));
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            class,
            w!("账号保险箱"),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            info.rcWork.left + (info.rcWork.right - info.rcWork.left - width) / 2,
            info.rcWork.top + (info.rcWork.bottom - info.rcWork.top - height) / 2,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            Some((&*state as *const RefCell<WindowState>).cast()),
        ) else {
            return;
        };
        let header = control(
            hwnd,
            w!("STATIC"),
            "将填入：正在确认",
            12,
            WINDOW_STYLE::default(),
        );
        let status = control(
            hwnd,
            w!("EDIT"),
            "",
            13,
            WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE(ES_READONLY as u32 | ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32),
        );
        let search = control(
            hwnd,
            w!("EDIT"),
            "",
            10,
            WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        );
        SendMessageW(
            search,
            windows::Win32::UI::Controls::EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(w!("在保险箱内搜索账号或用户名").as_ptr() as isize)),
        );
        let list = control(
            hwnd,
            w!("LISTBOX"),
            "",
            11,
            WS_TABSTOP | WS_VSCROLL | WS_BORDER | WINDOW_STYLE(LBS_NOTIFY as u32),
        );
        let more = control(hwnd, w!("BUTTON"), "账号与位置…", 3, WS_TABSTOP);
        let unlock = control(hwnd, w!("BUTTON"), "解锁 / 创建", 4, WS_TABSTOP);
        let lock = control(hwnd, w!("BUTTON"), "锁定", 5, WS_TABSTOP);
        let once = control(hwnd, w!("BUTTON"), "仅填这次", 1, WS_TABSTOP);
        let remember = control(hwnd, w!("BUTTON"), "填充并记住", 2, WS_TABSTOP);
        let controls = [
            header, status, search, list, more, unlock, lock, once, remember,
        ];
        {
            let mut r = runtime.borrow_mut();
            r.view = hwnd;
            r.open = true;
            r.fill_mode = fill_mode;
            r.last_active = Instant::now();
            if fill_mode {
                r.start_inspect(Purpose::Manual);
            } else {
                r.notice = "管理账号；填入前请返回目标应用并重新呼出".into();
            }
            r.changed();
        }
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(search));
        let mut ids: Vec<String> = vec![];
        let mut revision = u64::MAX;
        let mut quit = None;
        let mut msg = MSG::default();
        loop {
            if state.borrow().done || !runtime.borrow().open || runtime.borrow().flow.exiting {
                break;
            }
            let (resize, dpi, suggested) = {
                let mut s = state.borrow_mut();
                let values = (s.resize, s.dpi, s.suggested.take());
                s.resize = false;
                values
            };
            if resize {
                if let Some(r) = suggested {
                    runtime.borrow_mut().revoke();
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let font = {
                    let mut s = state.borrow_mut();
                    let _ = DeleteObject(s.font.into());
                    s.font = theme::font(dpi);
                    s.font
                };
                for c in controls {
                    SendMessageW(
                        c,
                        WM_SETFONT,
                        Some(WPARAM(font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                let scale = |x| x * dpi as i32 / 96;
                let pad = scale(12);
                let w = (rect.right - 2 * pad).max(1);
                let row = scale(32);
                let footer = rect.bottom - scale(90);
                let header_height = scale(28);
                let status_y = pad + header_height + scale(4);
                let status_height =
                    scale(66).min((footer - status_y - row * 2 - pad * 3).max(scale(20)));
                let search_y = status_y + status_height + scale(6);
                let y = search_y + row + scale(4);
                for (control, x, y, w, h) in [
                    (header, pad, pad, w, header_height),
                    (status, pad, status_y, w, status_height),
                    (search, pad, search_y, w, row),
                    (list, pad, y, w, (footer - y - pad).max(1)),
                    (more, pad, footer, w / 2 - pad, row),
                    (unlock, pad + w / 2, footer, w / 3, row),
                    (lock, pad + w * 5 / 6, footer, w / 6, row),
                    (once, pad, footer + row + pad, w / 2 - pad, row),
                    (remember, pad + w / 2, footer + row + pad, w / 2, row),
                ] {
                    let _ = MoveWindow(control, x, y, w, h, true);
                }
            }
            let changed = state.borrow().changed || revision != runtime.borrow().revision;
            if changed {
                state.borrow_mut().changed = false;
                let query = text(search).to_lowercase();
                let (
                    title,
                    notice,
                    rows,
                    selected,
                    unlocked,
                    enabled,
                    busy,
                    can_remember,
                    transfer,
                    rev,
                ) = {
                    let r = runtime.borrow();
                    let rows = r
                        .flow
                        .vault
                        .session()
                        .summaries()
                        .into_iter()
                        .filter(|a| {
                            a.name.to_lowercase().contains(&query)
                                || a.username.to_lowercase().contains(&query)
                        })
                        .collect::<Vec<_>>();
                    (
                        r.title(),
                        format!(
                            "{}{}",
                            if !r.settings.enabled {
                                "账号填充已停用；可管理保险箱。\r\n"
                            } else {
                                ""
                            },
                            r.notice
                        ),
                        rows,
                        r.selected.clone(),
                        r.flow.vault.session().is_unlocked(),
                        r.settings.enabled,
                        r.pending.is_some() || !r.flow.vault.can_shutdown() || r.flow.vault.busy(),
                        r.inspection
                            .as_ref()
                            .is_some_and(|(_, _, i)| !i.manual_only),
                        r.transfer,
                        r.revision,
                    )
                };
                revision = rev;
                let _ = SetWindowTextW(header, PCWSTR(wide(&title).as_ptr()));
                let _ = SetWindowTextW(status, PCWSTR(wide(&notice).as_ptr()));
                SendMessageW(list, LB_RESETCONTENT, None, None);
                ids.clear();
                for row in rows {
                    let label = wide(&format!("{}  ·  {}", row.name, row.username));
                    SendMessageW(
                        list,
                        LB_ADDSTRING,
                        None,
                        Some(LPARAM(label.as_ptr() as isize)),
                    );
                    ids.push(row.id.clone());
                }
                if let Some(index) = ids.iter().position(|id| Some(id) == selected.as_ref()) {
                    SendMessageW(list, LB_SETCURSEL, Some(WPARAM(index)), None);
                }
                let _ = EnableWindow(search, unlocked);
                let _ = EnableWindow(list, unlocked && !busy);
                let _ = EnableWindow(once, fill_mode && enabled && unlocked && !busy);
                let _ = EnableWindow(
                    remember,
                    fill_mode && enabled && unlocked && !busy && can_remember,
                );
                let _ = EnableWindow(unlock, !unlocked && !busy);
                let _ = EnableWindow(
                    more,
                    unlocked && !busy
                        || matches!(
                            runtime.borrow().flow.vault.state(),
                            SaveState::FailedWrite | SaveState::FailedPreparation
                        ),
                );
                let _ = EnableWindow(lock, unlocked || busy);
                if !unlocked {
                    let _ = SetWindowTextW(search, w!(""));
                }
                if !transfer && !IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(hwnd);
                }
            }
            let action = {
                let mut s = state.borrow_mut();
                std::mem::take(&mut s.action)
            };
            match action {
                1 | 2 => runtime.borrow_mut().fill(action == 2),
                6 => {
                    let foreground = GetForegroundWindow();
                    let mut pid = 0;
                    GetWindowThreadProcessId(foreground, Some(&mut pid));
                    if pid != windows::Win32::System::Threading::GetCurrentProcessId()
                        && !runtime.borrow().transfer
                    {
                        let mut r = runtime.borrow_mut();
                        r.revoke();
                        r.flow.target = None;
                        r.notice = "目标已离开；请重新呼出或指定密码框".into();
                    }
                }
                4 => {
                    if !runtime.borrow().flow.loaded
                        || matches!(
                            runtime.borrow().flow.access(true),
                            crate::account_fill_ui::Access::Reading
                        )
                    {
                        let mut r = runtime.borrow_mut();
                        r.queued = r.read();
                        continue;
                    }
                    let kind = if runtime.borrow().flow.snapshot.is_some() {
                        EditKind::Unlock
                    } else {
                        EditKind::Create
                    };
                    edit(&runtime, hwnd, kind);
                }
                5 => runtime.borrow_mut().lock(),
                11 => {
                    let index = SendMessageW(list, LB_GETCURSEL, None, None).0;
                    if index >= 0
                        && let Some(id) = ids.get(index as usize)
                    {
                        runtime.borrow_mut().select(id.clone());
                    }
                }
                3 => {
                    let menu = CreatePopupMenu().unwrap_or_default();
                    let actions = [
                        (20, "新增账号"),
                        (21, "编辑选中账号"),
                        (22, "删除选中账号及其关联"),
                        (23, "移除选中账号的全部关联"),
                        (24, "更改主密码"),
                        (25, "重新指定密码框"),
                        (26, "指定用户名框"),
                        (27, "指定区分目标（服务器 / 连接名）"),
                        (28, "跳过用户名，只填密码"),
                        (29, "换账号（清除选择）"),
                        (30, "重试失败保存"),
                        (31, "放弃失败修改并重新解锁"),
                    ];
                    for (id, label) in actions {
                        let r = runtime.borrow();
                        let failed = matches!(
                            r.flow.vault.state(),
                            SaveState::FailedWrite | SaveState::FailedPreparation
                        );
                        let allowed = if id >= 30 {
                            failed
                        } else {
                            r.flow.vault.session().is_unlocked()
                                && r.flow.vault.can_shutdown()
                                && r.pending.is_none()
                                && (!matches!(id, 21 | 22 | 23 | 29) || r.selected.is_some())
                                && (!(25..=28).contains(&id) || (r.settings.enabled && fill_mode))
                        };
                        let flags = if allowed {
                            MF_STRING
                        } else {
                            MF_STRING | MF_GRAYED
                        };
                        drop(r);
                        let _ = AppendMenuW(menu, flags, id, PCWSTR(wide(label).as_ptr()));
                    }
                    let mut point = POINT::default();
                    let _ = GetCursorPos(&mut point);
                    let choice = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_NONOTIFY,
                        point.x,
                        point.y,
                        None,
                        hwnd,
                        None,
                    )
                    .0;
                    let _ = DestroyMenu(menu);
                    match choice {
                        20 => edit(&runtime, hwnd, EditKind::Account(None)),
                        21 => {
                            let id = runtime.borrow().selected.clone();
                            if let Some(id) = id {
                                edit(&runtime, hwnd, EditKind::Account(Some(id)));
                            }
                        }
                        22 | 23 => {
                            let selected_id = runtime.borrow().selected.clone();
                            if let Some(id) = selected_id {
                                let answer = MessageBoxW(
                                    Some(hwnd),
                                    w!("确认对当前选中的账号执行此删除操作？保存成功后生效。"),
                                    w!("确认删除"),
                                    MB_YESNO | MB_ICONWARNING,
                                );
                                if answer == IDYES {
                                    let mut r = runtime.borrow_mut();
                                    let result = (|| -> Result<(), String> {
                                        let mut document =
                                            r.flow.vault.session().document()?.clone();
                                        if choice == 22 {
                                            document.delete_account(&id);
                                        } else {
                                            let binding_ids: Vec<_> = document
                                                .bindings
                                                .iter()
                                                .filter(|b| b.account_id == id)
                                                .map(|b| b.binding_id.clone())
                                                .collect();
                                            for id in binding_ids {
                                                document.delete_binding(&id);
                                            }
                                        }
                                        let request = r.flow.next_id();
                                        r.flow.vault.accept_save(request, document)
                                    })();
                                    r.notice =
                                        result.err().unwrap_or_else(|| "正在保存删除…".into());
                                    r.changed();
                                }
                            }
                        }
                        24 => edit(&runtime, hwnd, EditKind::Master),
                        25 => runtime.borrow_mut().pick(PickRole::Password),
                        26 => runtime.borrow_mut().pick(PickRole::Username),
                        27 => runtime.borrow_mut().pick(PickRole::Target),
                        28 => {
                            let mut r = runtime.borrow_mut();
                            r.username = None;
                            r.binding = None;
                            r.notice = "仅填写密码；用户名保持原样".into();
                            r.changed();
                        }
                        29 => {
                            let mut r = runtime.borrow_mut();
                            r.selected = None;
                            r.binding = None;
                            r.changed();
                        }
                        30 => runtime.borrow_mut().retry(),
                        31 if MessageBoxW(
                            Some(hwnd),
                            w!("仅放弃已经失败的未保存修改，并锁定保险箱。继续？"),
                            w!("重新解锁"),
                            MB_YESNO | MB_ICONWARNING,
                        ) == IDYES =>
                        {
                            let mut r = runtime.borrow_mut();
                            r.abandon();
                            r.queued = r.read();
                        }
                        _ => (),
                    }
                }
                _ => (),
            }
            if action != 0 {
                continue;
            }
            if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                quit = Some(msg.wParam.0 as i32);
                break;
            }
            if (msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd).as_bool())
                && matches!(msg.message, WM_KEYDOWN | WM_LBUTTONDOWN | WM_MOUSEWHEEL)
            {
                runtime.borrow_mut().last_active = Instant::now();
            }
            let context = windows::Win32::UI::Input::Ime::ImmGetContext(msg.hwnd);
            let composing = if !context.is_invalid() {
                let active = windows::Win32::UI::Input::Ime::ImmGetCompositionStringW(
                    context,
                    windows::Win32::UI::Input::Ime::GCS_COMPSTR,
                    None,
                    0,
                ) > 0;
                let _ = windows::Win32::UI::Input::Ime::ImmReleaseContext(msg.hwnd, context);
                active
            } else {
                false
            };
            if msg.message == WM_KEYDOWN && msg.wParam.0 == 27 && !composing {
                break;
            }
            if composing || !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        runtime.borrow_mut().dismiss();
        runtime.borrow_mut().view = HWND::default();
        SendMessageW(list, LB_RESETCONTENT, None, None);
        let _ = SetWindowTextW(search, w!(""));
        let _ = DestroyWindow(hwnd);
        let _ = EnableWindow(owner, true);
        // Successful fill leaves the destination foreground; closing manually restores the owner.
        if runtime.borrow().notice.contains("未提交登录") {
        } else {
            let _ = SetForegroundWindow(owner);
        }
        if let Some(code) = quit {
            PostQuitMessage(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Synthetic {
        root: HWND,
        password: HWND,
        owner: HWND,
    }
    impl Synthetic {
        unsafe fn new(nodes: usize) -> Self {
            unsafe {
                let root = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("synthetic-runtime"),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    50,
                    50,
                    500,
                    240,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let owner = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("synthetic-owner"),
                    WS_POPUP,
                    0,
                    0,
                    10,
                    10,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let password = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    w!(""),
                    WS_CHILD | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(ES_PASSWORD as u32),
                    20,
                    40,
                    280,
                    30,
                    Some(root),
                    Some(HMENU(100usize as *mut _)),
                    None,
                    None,
                )
                .unwrap();
                for i in 0..nodes {
                    CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        w!("STATIC"),
                        w!("synthetic-label"),
                        WS_CHILD | WS_VISIBLE,
                        350,
                        130,
                        2,
                        2,
                        Some(root),
                        Some(HMENU((200 + i) as *mut _)),
                        None,
                        None,
                    )
                    .unwrap();
                }
                assert!(SetForegroundWindow(root).as_bool());
                let _ = SetFocus(Some(password));
                Self {
                    root,
                    password,
                    owner,
                }
            }
        }
        fn identity(&self) -> WindowIdentity {
            account_target::window_identity(self.root.0 as u64).unwrap()
        }
        fn point(&self) -> account_picker::PickPoint {
            let mut rect = RECT::default();
            unsafe {
                GetWindowRect(self.password, &mut rect).unwrap();
            }
            account_picker::PickPoint {
                window: self.identity(),
                x: rect.left + 5,
                y: rect.top + 5,
                role: PickRole::Password,
            }
        }
    }
    impl Drop for Synthetic {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.root);
                let _ = DestroyWindow(self.owner);
            }
        }
    }
    fn pump_runtime(runtime: &mut Runtime) {
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while runtime.pending.is_some()
            || runtime.queued_inspect.is_some()
            || runtime.worker.as_ref().is_some_and(|w| w.is_busy())
            || !runtime.flow.vault.can_shutdown()
        {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let mut receipt = None;
            runtime.tick(|command| {
                if let Command::WriteAccountVault {
                    request_id,
                    expected_revision,
                    ..
                } = command
                {
                    receipt = Some((request_id, expected_revision + 1));
                }
                Ok(())
            });
            if let Some((request_id, revision)) = receipt {
                runtime.event(Event::AccountVaultWritten {
                    request_id,
                    result: Ok(revision),
                });
            }
            assert!(Instant::now() < deadline, "synthetic runtime timeout");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    fn unlocked_runtime(fixture: &Synthetic) -> Runtime {
        let mut runtime = Runtime::new();
        runtime.owner = fixture.owner;
        runtime.settings.enabled = true;
        runtime.settings.show_hint = false;
        runtime.open = true;
        runtime.flow.target = Some(fixture.identity());
        let master = Secret::new("synthetic-master-123".into());
        let document = VaultDocument {
            accounts: vec![crate::account_vault::Account {
                id: "account".into(),
                name: "synthetic".into(),
                username: String::new(),
                password: Secret::new("synthetic-password".into()),
            }],
            bindings: vec![],
        };
        let create_id = runtime.flow.next_id();
        runtime
            .flow
            .vault
            .accept_create(create_id, master.clone(), master, document)
            .unwrap();
        pump_runtime(&mut runtime);
        runtime.selected = Some("account".into());
        runtime
    }
    #[test]
    fn runtime_budget_failure_teaches_local_once_and_rejects_old_or_other_target_tickets() {
        unsafe {
            let fixture = Synthetic::new(270);
            let mut runtime = unlocked_runtime(&fixture);
            runtime.start_inspect(Purpose::Manual);
            pump_runtime(&mut runtime);
            assert!(runtime.inspection.is_none());
            assert!(runtime.notice.contains("页面较复杂"));
            assert!(
                runtime.teaching_context.is_some(),
                "failed complete capture must retain a teaching-only ticket"
            );
            runtime.picked(fixture.point());
            pump_runtime(&mut runtime);
            let (id, _, inspection) = runtime
                .inspection
                .as_ref()
                .expect("Runtime must reach a complete local scope");
            assert!(inspection.snapshot.complete);
            assert_eq!(inspection.snapshot.scope, SceneScope::SelectedContext);
            let local_binding = runtime
                .teaching()
                .unwrap()
                .finish_once("local".into(), "account".into())
                .unwrap()
                .into_binding();
            assert!(
                crate::account_fill_ui::hint_candidates(
                    std::slice::from_ref(&local_binding),
                    &inspection.snapshot
                )
                .is_empty()
            );
            assert!(matches!(
                match_binding(&local_binding, &inspection.snapshot),
                MatchDecision::ApplicationOnly(_)
            ));
            assert_eq!(
                runtime
                    .teaching()
                    .unwrap()
                    .finish("b".into(), "account".into()),
                Err(PolicyError::OneTimeOnly)
            );
            let old_id = *id;
            let mut wrong_point = fixture.point();
            wrong_point.window = account_target::window_identity(fixture.owner.0 as u64).unwrap();
            let worker = runtime.worker.as_mut().unwrap();
            let request = worker
                .teach_point(
                    runtime.flow.vault.session().epoch(),
                    Some(old_id),
                    wrong_point,
                )
                .unwrap();
            let deadline = Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Some(reply) = worker.try_recv() {
                    assert_eq!(reply.request_id, request);
                    assert!(matches!(
                        reply.result,
                        AccountResult::Failed(FillError::TargetChanged)
                    ));
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            runtime.fill(true);
            assert!(runtime.after_save.is_none());
            assert!(runtime.notice.contains("只能本次"));
            runtime.fill(false);
            pump_runtime(&mut runtime);
            let mut text = [0u16; 64];
            let count = GetWindowTextW(fixture.password, &mut text);
            assert_eq!(
                String::from_utf16_lossy(&text[..count as usize]),
                "synthetic-password"
            );
            // Successful fill revokes the generation. A supplied old ticket must never create a fresh context.
            let cleanup_deadline = Instant::now() + std::time::Duration::from_secs(10);
            while runtime.worker.as_ref().unwrap().is_busy() {
                assert!(Instant::now() < cleanup_deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let worker = runtime.worker.as_mut().unwrap();
            let request = worker
                .teach_point(
                    runtime.flow.vault.session().epoch(),
                    Some(old_id),
                    fixture.point(),
                )
                .unwrap();
            let deadline = Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Some(reply) = worker.try_recv() {
                    assert_eq!(reply.request_id, request);
                    assert!(matches!(
                        reply.result,
                        AccountResult::Failed(FillError::TargetChanged)
                    ));
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(runtime.teaching_context.is_none());
        }
    }
    #[test]
    fn runtime_equal_remember_fills_without_save_but_pending_and_failed_save_still_block() {
        unsafe {
            let fixture = Synthetic::new(0);
            let mut runtime = unlocked_runtime(&fixture);
            runtime.start_inspect(Purpose::Manual);
            pump_runtime(&mut runtime);
            let binding = runtime
                .teaching()
                .unwrap()
                .finish("stable-id".into(), "account".into())
                .unwrap();
            let mut document = runtime.flow.vault.session().document().unwrap().clone();
            document.upsert_binding(binding.clone()).unwrap();
            let request = runtime.flow.next_id();
            runtime.flow.vault.accept_save(request, document).unwrap();
            pump_runtime(&mut runtime);
            runtime.binding = Some(binding.clone());
            runtime.fill(true);
            assert!(runtime.after_save.is_none());
            assert!(runtime.flow.vault.can_shutdown());
            assert!(runtime.pending.is_some());
            pump_runtime(&mut runtime);
            let document = runtime.flow.vault.session().document().unwrap();
            assert_eq!(document.bindings.len(), 1);
            assert_eq!(document.bindings[0].binding_id, "stable-id");
            runtime.open = true;
            runtime.flow.target = Some(fixture.identity());
            runtime.selected = Some("account".into());
            runtime.start_inspect(Purpose::Manual);
            pump_runtime(&mut runtime);
            let request = runtime.flow.next_id();
            let document = runtime.flow.vault.session().document().unwrap().clone();
            runtime.flow.vault.accept_save(request, document).unwrap();
            runtime.fill(true);
            assert!(runtime.pending.is_none());
            assert!(runtime.after_save.is_none());
            let deadline = Instant::now() + std::time::Duration::from_secs(30);
            loop {
                let mut written = false;
                runtime.tick(|command| {
                    if matches!(command, Command::WriteAccountVault { .. }) {
                        written = true;
                    }
                    Ok(())
                });
                if written {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            runtime.event(Event::AccountVaultWritten {
                request_id: request,
                result: Err("synthetic-save-failure".into()),
            });
            runtime.fill(true);
            assert!(runtime.pending.is_none());
            assert!(runtime.after_save.is_none());
            assert!(!runtime.flow.vault.can_shutdown());
        }
    }
    #[test]
    fn app_consumer_keeps_pumping_preparation_and_receipts_before_shutdown() {
        let mut runtime = Runtime::new();
        let id = runtime.flow.next_id();
        let master = Secret::new("synthetic-master-123".into());
        runtime
            .flow
            .vault
            .accept_create(id, master.clone(), master, VaultDocument::default())
            .unwrap();
        runtime.exit();
        runtime.shutdown_requested = true;
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        let mut write = false;
        while !write {
            runtime.tick(|command| {
                match command {
                    Command::WriteAccountVault { request_id, .. } => {
                        assert_eq!(request_id, id);
                        write = true;
                    }
                    Command::Shutdown => panic!("shutdown before correlated vault receipt"),
                    _ => (),
                }
                Ok(())
            });
            assert!(Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        runtime.event(Event::AccountVaultWritten {
            request_id: id + 1,
            result: Ok(1),
        });
        runtime.tick(|command| {
            assert!(!matches!(command, Command::Shutdown));
            Ok(())
        });
        runtime.event(Event::AccountVaultWritten {
            request_id: id,
            result: Ok(1),
        });
        let mut shutdown = false;
        runtime.tick(|command| {
            shutdown |= matches!(command, Command::Shutdown);
            Ok(())
        });
        assert!(shutdown);
        assert!(!runtime.flow.vault.session().is_unlocked());
    }
    #[test]
    fn unlock_failure_is_delivered_to_the_still_open_form() {
        use crate::account_vault::{VaultSession, VaultSnapshot};
        let master = Secret::new("synthetic-master-123".into());
        let session = VaultSession::create(&master, &master, VaultDocument::default()).unwrap();
        let mut runtime = Runtime::new();
        let epoch = runtime.flow.vault.session().epoch();
        runtime
            .flow
            .vault
            .begin_unlock(
                epoch,
                Secret::new("synthetic-wrong-master".into()),
                VaultSnapshot {
                    revision: 1,
                    envelope: session.encrypt().unwrap(),
                },
            )
            .unwrap();
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while runtime.operation_error.is_none() {
            runtime.tick(|_| Ok(()));
            assert!(Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(
            crate::account_fill_ui::form_progress(
                epoch,
                runtime.flow.vault.session().epoch(),
                0,
                runtime.completed,
                true,
                runtime.operation_error.as_deref()
            ),
            AccountFormProgress::Failed(_)
        ));
        assert!(!runtime.flow.vault.session().is_unlocked());
    }
    #[test]
    fn hidden_fill_reports_partial_but_silent_cancel_and_background_checks_do_not_alert() {
        let mut runtime = Runtime::new();
        runtime.present_fill(
            &FillOutcome {
                username_written: false,
                password_written: false,
                error: Some(FillError::Cancelled),
            },
            false,
        );
        assert!(runtime.take_attention().is_none());
        runtime.present_fill(
            &FillOutcome {
                username_written: true,
                password_written: false,
                error: Some(FillError::WriteUncertain),
            },
            false,
        );
        assert!(runtime.take_attention().unwrap().contains("密码未确认完成"));
        runtime.notice = "新目标正在识别".into();
        let target = WindowIdentity {
            handle: 101,
            pid: 202,
            process_start: 303,
        };
        runtime.flow.target = Some(target);
        runtime.present_fill(
            &FillOutcome {
                username_written: false,
                password_written: true,
                error: None,
            },
            true,
        );
        assert!(runtime.take_attention().unwrap().contains("之前已取消"));
        assert_eq!(runtime.notice, "新目标正在识别");
        assert_eq!(runtime.flow.target, Some(target));
        assert!(!runtime.flow.vault.session().is_unlocked());
    }
}
