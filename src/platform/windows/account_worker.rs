//! One actual slot on one persistent, windowless MTA. Cancellation never joins it.
use super::{
    account_picker::{PickPoint, PickRole},
    account_target::{Automation, Context, Fingerprints, focus_handle},
};
use crate::{
    account_fill::*, account_fill_execution::*, account_fill_ui::HintPermit, account_vault::Secret,
};
use std::{
    cell::Cell,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};
use windows::Win32::System::Com::*;

#[derive(Clone, Debug)]
pub struct Inspection {
    pub password_bounds: Option<super::account_target::PasswordBounds>,
    pub snapshot: SceneSnapshot,
    pub suggested_password: Option<u64>,
    pub suggested_username: Option<u64>,
    pub teaching_required: bool,
    pub manual_only: bool,
}
#[derive(Clone, Debug)]
pub struct TaughtPoint {
    pub inspection: Inspection,
    pub field_id: u64,
    pub role: PickRole,
}
#[derive(Clone, Debug)]
pub enum AccountResult {
    Inspected(Inspection),
    Taught(TaughtPoint),
    Filled(FillOutcome),
    Failed(FillError),
}
#[derive(Clone, Debug)]
pub struct AccountReply {
    pub request_id: u64,
    pub generation: u64,
    pub result: AccountResult,
}
/// Secret-bearing command intentionally has no Debug or Clone.
pub struct FillCommand {
    pub inspection_id: u64,
    pub expected_window: WindowIdentity,
    pub choice: FillChoice,
    pub settings: AccountFillSettings,
    pub action: UserFillAction,
    pub username: Secret,
    pub password: Secret,
}
enum Operation {
    Inspect {
        target: WindowIdentity,
        binding: Option<Box<Binding>>,
    },
    Point {
        inspection_id: Option<u64>,
        point: PickPoint,
    },
    Fill(Box<FillCommand>, u64, Option<HintPermit>),
}
enum Message {
    Job(Box<Job>),
    Invalidate,
}
struct Job {
    id: u64,
    lease: FillLease,
    operation: Operation,
}
pub struct AccountWorker {
    tx: Arc<SyncSender<Message>>,
    rx: Receiver<AccountReply>,
    control: FillControl,
    next_id: u64,
    pending: Cell<Option<u64>>,
    cleanup_requested: Arc<AtomicBool>,
}
/// Cloneable cancellation handle. Does not keep the worker thread alive.
#[derive(Clone)]
pub struct AccountControl {
    control: FillControl,
    tx: Weak<SyncSender<Message>>,
    cleanup_requested: Arc<AtomicBool>,
}
impl AccountControl {
    pub fn invalidate(&self, epoch: u64) {
        self.cleanup_requested.store(true, Ordering::SeqCst);
        self.control.invalidate(epoch);
        if let Some(tx) = self.tx.upgrade() {
            let _ = tx.try_send(Message::Invalidate);
        }
    }
    pub fn generation(&self) -> u64 {
        self.control.generation()
    }
    pub fn is_busy(&self) -> bool {
        self.control.is_busy() || self.cleanup_requested.load(Ordering::SeqCst)
    }
}
impl AccountWorker {
    pub fn new(epoch: u64) -> Result<Self, FillError> {
        let lifetime = WorkerLifetime::acquire()?;
        let (tx, jobs) = mpsc::sync_channel::<Message>(1);
        let (replies, rx) = mpsc::sync_channel(1);
        let control = FillControl::new(epoch);
        let cleanup_requested = Arc::new(AtomicBool::new(false));
        let cleanup_on_worker = cleanup_requested.clone();
        std::thread::Builder::new()
            .name("account-uia".into())
            .spawn(move || {
                let _lifetime = lifetime;
                run(jobs, replies, cleanup_on_worker)
            })
            .map_err(|_| FillError::QueryFailed)?;
        Ok(Self {
            tx: Arc::new(tx),
            rx,
            control,
            next_id: 0,
            pending: Cell::new(None),
            cleanup_requested,
        })
    }
    pub fn control(&self) -> AccountControl {
        AccountControl {
            control: self.control.clone(),
            tx: Arc::downgrade(&self.tx),
            cleanup_requested: self.cleanup_requested.clone(),
        }
    }
    pub fn invalidate(&self, epoch: u64) {
        self.control().invalidate(epoch)
    }
    pub fn is_busy(&self) -> bool {
        self.control.is_busy() || self.cleanup_requested.load(Ordering::SeqCst)
    }
    pub fn inspect(&mut self, epoch: u64, target: WindowIdentity) -> Result<u64, FillError> {
        self.submit(
            epoch,
            Operation::Inspect {
                target,
                binding: None,
            },
        )
    }
    /// Re-sample a previously user-designated discriminator only after checking
    /// the binding's application identity. Does not authorize filling.
    pub fn inspect_binding(
        &mut self,
        epoch: u64,
        target: WindowIdentity,
        binding: Binding,
    ) -> Result<u64, FillError> {
        binding.validate()?;
        self.submit(
            epoch,
            Operation::Inspect {
                target,
                binding: Some(Box::new(binding)),
            },
        )
    }
    pub fn teach_point(
        &mut self,
        epoch: u64,
        inspection_id: Option<u64>,
        point: PickPoint,
    ) -> Result<u64, FillError> {
        self.submit(
            epoch,
            Operation::Point {
                inspection_id,
                point,
            },
        )
    }
    /// Only call from an explicit fill click, after restoring target foreground.
    pub fn fill(&mut self, epoch: u64, command: FillCommand) -> Result<u64, FillError> {
        self.submit_fill(epoch, command, None)
    }
    /// A hint click carries its already-consumed event version to the final MTA write.
    pub fn fill_hint(
        &mut self,
        epoch: u64,
        command: FillCommand,
        permit: HintPermit,
    ) -> Result<u64, FillError> {
        self.submit_fill(epoch, command, Some(permit))
    }
    fn submit_fill(
        &mut self,
        epoch: u64,
        command: FillCommand,
        permit: Option<HintPermit>,
    ) -> Result<u64, FillError> {
        if (command.action == UserFillAction::ConfirmedHint) != permit.is_some()
            || permit.as_ref().is_some_and(|p| !p.current())
        {
            return Err(FillError::Cancelled);
        }
        let focus = focus_handle(command.expected_window).ok_or(FillError::TargetChanged)?;
        self.submit(epoch, Operation::Fill(Box::new(command), focus, permit))
    }
    fn submit(&mut self, epoch: u64, operation: Operation) -> Result<u64, FillError> {
        if self.pending.get().is_some() || self.cleanup_requested.load(Ordering::SeqCst) {
            return Err(FillError::Busy);
        }
        let lease = self.control.reserve(epoch)?;
        self.next_id = self.next_id.checked_add(1).ok_or(FillError::Cancelled)?;
        self.tx
            .try_send(Message::Job(Box::new(Job {
                id: self.next_id,
                lease,
                operation,
            })))
            .map_err(|error| match error {
                TrySendError::Full(_) => FillError::Busy,
                TrySendError::Disconnected(_) => FillError::QueryFailed,
            })?;
        self.pending.set(Some(self.next_id));
        Ok(self.next_id)
    }
    /// Stale replies cannot revive UI state. A cancelled in-flight write can have
    /// completed; callers may receive its nonsecret partial result for reporting.
    pub fn try_recv(&self) -> Option<AccountReply> {
        while let Ok(reply) = self.rx.try_recv() {
            if self.pending.get() == Some(reply.request_id) {
                self.pending.set(None);
            }
            if reply.generation == self.control.generation()
                || matches!(reply.result, AccountResult::Filled(_))
            {
                return Some(reply);
            }
        }
        None
    }
}
impl Drop for AccountWorker {
    fn drop(&mut self) {
        self.invalidate(u64::MAX)
    }
}
struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
struct Session {
    id: u64,
    generation: u64,
    context: Context,
    snapshot: Option<SceneSnapshot>,
}
fn inspection(
    snapshot: SceneSnapshot,
    manual_only: bool,
    password_bounds: Option<super::account_target::PasswordBounds>,
) -> Inspection {
    let passwords: Vec<_> = snapshot.fields.iter().filter(|f| f.is_password).collect();
    let usernames: Vec<_> = snapshot.fields.iter().filter(|f| !f.is_password).collect();
    let eligible = |f: &&LiveField| {
        f.visible
            && f.enabled
            && f.writable
            && snapshot
                .fields
                .iter()
                .filter(|candidate| f.selector.matches(&candidate.selector))
                .count()
                == 1
    };
    let suggested_password = (passwords.len() == 1)
        .then(|| passwords[0])
        .filter(|f| eligible(f))
        .map(|f| f.field_id);
    let suggested_username = (usernames.len() == 1)
        .then(|| usernames[0])
        .filter(|f| eligible(f))
        .map(|f| f.field_id);
    let teaching_required = suggested_password.is_none() || usernames.len() > 1;
    Inspection {
        password_bounds,
        manual_only: manual_only
            || snapshot.origin == SceneOrigin::UnverifiedBrowser
            || snapshot.scope == SceneScope::SelectedContext,
        snapshot,
        suggested_password,
        suggested_username,
        teaching_required,
    }
}
fn run(jobs: Receiver<Message>, replies: SyncSender<AccountReply>, cleanup: Arc<AtomicBool>) {
    if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
        for message in jobs {
            let Message::Job(job) = message else {
                cleanup.store(false, Ordering::SeqCst);
                continue;
            };
            let Job {
                id,
                lease,
                operation,
            } = *job;
            let generation = lease.generation();
            drop(operation);
            drop(lease);
            cleanup.store(false, Ordering::SeqCst);
            let _ = replies.send(AccountReply {
                request_id: id,
                generation,
                result: AccountResult::Failed(FillError::QueryFailed),
            });
        }
        return;
    }
    let _com = Com;
    let automation = Automation::new();
    let mut cache = Fingerprints::default();
    let mut session: Option<Session> = None;
    let mut gate = FillGate::default();
    for message in jobs {
        let job = match message {
            Message::Job(job) => *job,
            Message::Invalidate => {
                gate.revoke();
                session = None;
                cleanup.store(false, Ordering::SeqCst);
                continue;
            }
        };
        let generation = job.lease.generation();
        let result = if !job.lease.is_current() {
            gate.revoke();
            session = None;
            drop(job.operation);
            AccountResult::Failed(FillError::Cancelled)
        } else {
            match &automation {
                Err(error) => {
                    drop(job.operation);
                    AccountResult::Failed(*error)
                }
                Ok(automation) => handle(
                    automation,
                    &mut cache,
                    &mut session,
                    &mut gate,
                    &job.lease,
                    job.id,
                    job.operation,
                )
                .unwrap_or_else(AccountResult::Failed),
            }
        };
        if !job.lease.is_current() {
            gate.revoke();
            session = None;
            cleanup.store(false, Ordering::SeqCst);
        }
        let reply = AccountReply {
            request_id: job.id,
            generation,
            result,
        };
        // Busy remains held until every provider call and secret cleanup has finished.
        drop(job.lease);
        let _ = replies.send(reply);
    }
}
fn handle(
    automation: &Automation,
    cache: &mut Fingerprints,
    session: &mut Option<Session>,
    gate: &mut FillGate,
    lease: &FillLease,
    id: u64,
    operation: Operation,
) -> Result<AccountResult, FillError> {
    let query_deadline = deadline();
    match operation {
        Operation::Inspect { target, binding } => {
            gate.revoke();
            *session = None;
            let mut context = automation.context(target, cache, query_deadline)?;
            if let Some(binding) = binding {
                automation.designate(&mut context, &binding)?;
            }
            *session = Some(Session {
                id,
                generation: lease.generation(),
                context,
                snapshot: None,
            });
            let current = session.as_mut().ok_or(FillError::Cancelled)?;
            let snapshot = automation.sample(&mut current.context, query_deadline)?;
            if !lease.is_current() {
                return Err(FillError::Cancelled);
            }
            current.snapshot = Some(snapshot.clone());
            Ok(AccountResult::Inspected(inspection(
                snapshot,
                false,
                current.context.password_bounds,
            )))
        }
        Operation::Point {
            inspection_id,
            point,
        } => {
            gate.revoke();
            if let Some(expected) = inspection_id {
                let current = session.as_ref().ok_or(FillError::TargetChanged)?;
                if current.id != expected
                    || current.generation != lease.generation()
                    || current.context.target != point.window
                {
                    return Err(FillError::TargetChanged);
                }
            } else {
                *session = Some(Session {
                    id,
                    generation: lease.generation(),
                    context: automation.context(point.window, cache, query_deadline)?,
                    snapshot: None,
                });
            }
            let current = session.as_mut().ok_or(FillError::Cancelled)?;
            // A failed new teaching request supersedes the old inspection too.
            current.id = id;
            current.snapshot = None;
            automation.verify_app(&current.context, cache, query_deadline)?;
            let (field_id, snapshot) = automation.point(
                &mut current.context,
                point.x,
                point.y,
                point.role,
                query_deadline,
            )?;
            if !lease.is_current() {
                return Err(FillError::Cancelled);
            }
            current.id = id;
            current.snapshot = Some(snapshot.clone());
            Ok(AccountResult::Taught(TaughtPoint {
                inspection: inspection(snapshot, false, current.context.password_bounds),
                field_id,
                role: point.role,
            }))
        }
        Operation::Fill(command, expected_focus, hint) => {
            let current = session.as_mut().ok_or(FillError::TargetChanged)?;
            if current.id != command.inspection_id
                || current.generation != lease.generation()
                || command.expected_window != current.context.target
            {
                return Err(FillError::TargetChanged);
            }
            let baseline = current.snapshot.take().ok_or(FillError::TargetChanged)?;
            let hint_password = if hint.is_some() {
                match match_binding(command.choice.binding(), &baseline) {
                    MatchDecision::Exact(fields) => Some(fields.password_id),
                    _ => return Err(FillError::TargetChanged),
                }
            } else {
                None
            };
            let browser_password_picked = current.context.picked_password.is_some_and(|id| {
                baseline
                    .fields
                    .iter()
                    .any(|f| f.field_id == id && f.selector == command.choice.binding().password)
            });
            let mut backend = NativeBackend {
                automation,
                context: &mut current.context,
                cache,
                lease,
                deadline: query_deadline,
                expected_focus,
                hint,
                hint_password,
            };
            let result = execute_fill(
                gate,
                &mut backend,
                lease,
                FillIntent {
                    settings: command.settings,
                    baseline,
                    choice: command.choice,
                    action: command.action,
                    browser_password_picked,
                    username: command.username,
                    password: command.password,
                },
            );
            *session = None;
            Ok(AccountResult::Filled(result))
        }
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_millis(UIA_QUERY_BUDGET_MS)
}
struct NativeBackend<'a> {
    automation: &'a Automation,
    context: &'a mut Context,
    cache: &'a mut Fingerprints,
    lease: &'a FillLease,
    deadline: Instant,
    expected_focus: u64,
    hint: Option<HintPermit>,
    hint_password: Option<u64>,
}
impl FillBackend for NativeBackend<'_> {
    fn sample(&mut self) -> Result<SceneSnapshot, FillError> {
        self.automation
            .verify_app(self.context, self.cache, self.deadline)?;
        let snapshot = self.automation.sample(self.context, self.deadline)?;
        if let (Some(permit), Some(expected)) = (&self.hint, self.hint_password) {
            let focused =
                self.automation
                    .focused_password(self.context, &snapshot, self.deadline)?;
            if !permit.matches_focused_password(expected, focused) {
                return Err(FillError::TargetChanged);
            }
        }
        Ok(snapshot)
    }
    fn foreground_matches(&self, window: WindowIdentity) -> bool {
        focus_handle(window) == Some(self.expected_focus)
            && self.hint.as_ref().is_none_or(HintPermit::current)
    }
    fn write(&mut self, field: u64, value: &Secret) -> Result<(), FillError> {
        self.automation.write(
            self.context,
            field,
            value,
            self.lease,
            self.deadline,
            self.expected_focus,
            self.hint.as_ref(),
        )
    }
}

// A dropped UI owner cannot replace a still occupied MTA with another thread.
static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);
struct WorkerLifetime;
impl WorkerLifetime {
    fn acquire() -> Result<Self, FillError> {
        WORKER_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| FillError::Busy)?;
        Ok(Self)
    }
}
impl Drop for WorkerLifetime {
    fn drop(&mut self) {
        WORKER_RUNNING.store(false, Ordering::SeqCst);
    }
}
