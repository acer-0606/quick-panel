//! A single actual crypto slot and one accepted save, retained through UI close.
//! Keep the controller at application scope and pump it before Engine Shutdown.
use crate::{
    account_vault::*,
    model::{Command, Event},
};
use std::sync::{Arc, Mutex};

type Random = fn(&mut [u8]) -> Result<(), String>;
enum Job {
    Unlock(Secret, VaultSnapshot),
    Create(Secret, Secret, VaultDocument),
    Save(VaultSession),
    Change(VaultSession, Secret, Secret),
}
struct Completion {
    epoch: u64,
    unlock: bool,
    candidate: Option<VaultSession>,
    envelope: Result<Vec<u8>, String>,
}
struct Slot {
    epoch: u64,
    busy: bool,
    closed: bool,
    result: Option<Completion>,
}
/// No replacement queue; invalidation never releases a running slot. Drop never joins.
pub struct VaultWorker {
    slot: Arc<Mutex<Slot>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    random: Random,
}
impl VaultWorker {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_random(notify, crate::account_vault::random)
    }
    fn with_random(notify: Arc<dyn Fn() + Send + Sync>, random: Random) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Slot {
                epoch: 0,
                busy: false,
                closed: false,
                result: None,
            })),
            notify,
            random,
        }
    }
    pub fn busy(&self) -> bool {
        self.slot.lock().unwrap_or_else(|p| p.into_inner()).busy
    }
    fn submit(&self, epoch: u64, job: Job) -> Result<(), String> {
        let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
        if slot.busy || slot.closed {
            return Err("账号保险箱正在处理，请稍候".into());
        }
        slot.epoch = epoch;
        slot.busy = true;
        let shared = self.slot.clone();
        let notify = self.notify.clone();
        let random = self.random;
        let spawned = std::thread::Builder::new()
            .name("account-vault".into())
            .spawn(move || {
                let unlock = matches!(job, Job::Unlock(..));
                let result = match job {
                    Job::Unlock(password, snapshot) => VaultSession::unlock(&password, &snapshot),
                    Job::Create(password, confirmation, document) => {
                        VaultSession::create(&password, &confirmation, document)
                    }
                    Job::Save(session) => Ok(session),
                    Job::Change(session, password, confirmation) => {
                        session.change_master(&password, &confirmation)
                    }
                };
                let (candidate, envelope) = match result {
                    Ok(session) => {
                        let envelope = if unlock {
                            Ok(Vec::new())
                        } else {
                            session.encrypt_with(random)
                        };
                        (Some(session), envelope)
                    }
                    Err(error) => (None, Err(error)),
                };
                let mut completion = Completion {
                    epoch,
                    unlock,
                    candidate,
                    envelope,
                };
                let mut slot = shared.lock().unwrap_or_else(|p| p.into_inner());
                // Invalidation and completion publish under the same mutex; even a
                // completed result waiting for the UI cannot retain secrets after lock.
                if slot.closed || slot.epoch != epoch {
                    completion.candidate = None;
                }
                if slot.closed {
                    return;
                }
                slot.result = Some(completion);
                drop(slot);
                notify();
            });
        if spawned.is_err() {
            slot.busy = false;
            return Err("无法启动账号保险箱后台任务".into());
        }
        Ok(())
    }
    /// Revoke candidates, including an already completed but unconsumed result.
    pub fn invalidate(&self, epoch: u64) {
        let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
        slot.epoch = epoch;
        if let Some(result) = &mut slot.result {
            result.candidate = None;
        }
    }
    fn try_result(&self) -> Option<Completion> {
        let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
        let result = slot.result.take()?;
        slot.busy = false;
        Some(result)
    }
}
impl Drop for VaultWorker {
    fn drop(&mut self) {
        let mut slot = self.slot.lock().unwrap_or_else(|p| p.into_inner());
        slot.closed = true;
        slot.result = None;
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    Idle,
    Preparing,
    Ready,
    AwaitingReceipt,
    FailedPreparation,
    FailedWrite,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultNotice {
    Unlocked,
    Prepared,
    Saved,
    Failed(String),
    Stale,
}
struct Pending {
    id: u64,
    expected: u64,
    epoch: u64,
    state: SaveState,
    envelope: Option<Vec<u8>>,
    candidate: Option<VaultSession>,
    error: Option<String>,
}
pub struct VaultController {
    session: VaultSession,
    worker: VaultWorker,
    pending: Option<Pending>,
    last_id: u64,
}
impl VaultController {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            session: VaultSession::default(),
            worker: VaultWorker::new(notify),
            pending: None,
            last_id: 0,
        }
    }
    pub fn session(&self) -> &VaultSession {
        &self.session
    }
    pub fn state(&self) -> SaveState {
        self.pending.as_ref().map_or(SaveState::Idle, |p| p.state)
    }
    pub fn error(&self) -> Option<&str> {
        self.pending.as_ref().and_then(|p| p.error.as_deref())
    }
    pub fn pending_document(&self) -> Option<&VaultDocument> {
        self.pending.as_ref()?.candidate.as_ref()?.document().ok()
    }
    pub fn busy(&self) -> bool {
        self.worker.busy()
    }
    /// False for preparing, in-flight and failed saves. An unlock may be canceled.
    pub fn can_shutdown(&self) -> bool {
        self.pending.is_none()
    }
    /// Management may unlock even when filling is disabled; idle policy still applies.
    pub fn lock_if_idle(
        &mut self,
        settings: &crate::account_fill::AccountFillSettings,
        idle: std::time::Duration,
    ) -> Result<bool, String> {
        settings
            .validate()
            .map_err(|_| "账号保险箱闲置锁定设置无效")?;
        if self.session.is_unlocked()
            && idle >= std::time::Duration::from_secs(u64::from(settings.lock_minutes) * 60)
        {
            self.lock();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn begin_unlock(
        &mut self,
        expected_epoch: u64,
        password: Secret,
        snapshot: VaultSnapshot,
    ) -> Result<(), String> {
        if self.pending.is_some()
            || self.session.is_unlocked()
            || self.session.epoch() != expected_epoch
        {
            return Err("账号保险箱请求已失效或正在保存".into());
        }
        self.worker
            .submit(expected_epoch, Job::Unlock(password, snapshot))
    }
    fn accept(&mut self, id: u64, expected: u64, job: Job) -> Result<(), String> {
        if self.pending.is_some() || id <= self.last_id {
            return Err("账号保险箱保存请求重复或正在保存".into());
        }
        let epoch = self.session.epoch();
        self.worker.submit(epoch, job)?;
        self.last_id = id;
        self.pending = Some(Pending {
            id,
            expected,
            epoch,
            state: SaveState::Preparing,
            envelope: None,
            candidate: None,
            error: None,
        });
        Ok(())
    }
    pub fn accept_create(
        &mut self,
        id: u64,
        password: Secret,
        confirmation: Secret,
        document: VaultDocument,
    ) -> Result<(), String> {
        if self.session.is_unlocked() {
            return Err("账号保险箱已建立".into());
        }
        self.accept(id, 0, Job::Create(password, confirmation, document))
    }
    pub fn accept_save(&mut self, id: u64, document: VaultDocument) -> Result<(), String> {
        let candidate = self.session.candidate(document)?;
        self.accept(
            id,
            self.session.revision().ok_or("保险箱已锁定")?,
            Job::Save(candidate),
        )
    }
    pub fn accept_change_master(
        &mut self,
        id: u64,
        password: Secret,
        confirmation: Secret,
    ) -> Result<(), String> {
        self.accept(
            id,
            self.session.revision().ok_or("保险箱已锁定")?,
            Job::Change(self.session.clone(), password, confirmation),
        )
    }
    /// Use for manual/idle/session lock, feature disable, close, and exit.
    /// Accepted encryption continues; UI must retain this controller while closing.
    pub fn lock(&mut self) {
        self.session.lock();
        self.worker.invalidate(self.session.epoch());
        if let Some(p) = &mut self.pending {
            p.candidate = None;
        }
    }
    pub fn poll(&mut self) -> Option<VaultNotice> {
        let mut result = self.worker.try_result()?;
        if result.unlock {
            if result.epoch != self.session.epoch() {
                return Some(VaultNotice::Stale);
            }
            return Some(match result.envelope {
                Ok(_) => {
                    if let Some(candidate) = result.candidate {
                        let revision = candidate.revision().unwrap();
                        self.session.install(candidate, revision);
                        VaultNotice::Unlocked
                    } else {
                        VaultNotice::Stale
                    }
                }
                Err(error) => VaultNotice::Failed(error),
            });
        }
        let p = self.pending.as_mut()?;
        if p.state != SaveState::Preparing {
            return Some(VaultNotice::Stale);
        }
        if result.epoch != self.session.epoch() {
            result.candidate = None;
        }
        p.candidate = result.candidate;
        Some(match result.envelope {
            Ok(envelope) => {
                p.envelope = Some(envelope);
                p.state = SaveState::Ready;
                VaultNotice::Prepared
            }
            Err(error) => {
                p.state = SaveState::FailedPreparation;
                p.error = Some(error.clone());
                VaultNotice::Failed(error)
            }
        })
    }
    /// Call after Prepared, using Engine::send. A send failure retains this exact envelope.
    pub fn dispatch_save(
        &mut self,
        send: impl FnOnce(Command) -> Result<(), String>,
    ) -> Result<(), String> {
        let p = self.pending.as_mut().ok_or("没有待保存操作")?;
        if p.state != SaveState::Ready {
            return Err("账号保险箱尚未准备好保存".into());
        }
        let result = send(Command::WriteAccountVault {
            request_id: p.id,
            expected_revision: p.expected,
            envelope: p.envelope.as_ref().ok_or("没有待保存密文")?.clone(),
        });
        match &result {
            Ok(()) => p.state = SaveState::AwaitingReceipt,
            Err(error) => {
                p.state = SaveState::FailedWrite;
                p.error = Some(error.clone());
            }
        }
        result
    }
    pub fn handle_event(&mut self, event: &Event) -> Option<VaultNotice> {
        let Event::AccountVaultWritten { request_id, result } = event else {
            return None;
        };
        let p = self.pending.as_mut()?;
        if p.id != *request_id || p.state != SaveState::AwaitingReceipt {
            return None;
        }
        match result {
            Ok(revision) if p.expected.checked_add(1) == Some(*revision) => {
                let mut p = self.pending.take().unwrap();
                if p.epoch == self.session.epoch()
                    && let Some(candidate) = p.candidate.take()
                {
                    self.session.install(candidate, *revision);
                }
                Some(VaultNotice::Saved)
            }
            _ => {
                let error = result
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "账号保险箱保存回执无效".into());
                p.error = Some(error.clone());
                p.state = SaveState::FailedWrite;
                Some(VaultNotice::Failed(error))
            }
        }
    }
    /// Write retries retain request ID, expected revision and byte-identical ciphertext.
    pub fn retry(&mut self) -> Result<(), String> {
        let p = self.pending.as_mut().ok_or("没有待重试操作")?;
        match p.state {
            SaveState::FailedWrite => {
                p.state = SaveState::Ready;
                p.error = None;
                Ok(())
            }
            SaveState::FailedPreparation => {
                let candidate = p
                    .candidate
                    .as_ref()
                    .ok_or("准备失败且保险箱已锁定，请明确放弃本次修改后重新解锁并编辑")?
                    .clone();
                self.worker.submit(p.epoch, Job::Save(candidate))?;
                p.state = SaveState::Preparing;
                p.error = None;
                Ok(())
            }
            _ => Err("当前操作不能重试".into()),
        }
    }
    /// Explicit user abandonment of a terminal failure only. Always lock and
    /// advance epoch, so reloading cannot preserve an earlier fill permission.
    pub fn discard_failed_save(&mut self) -> Result<(), String> {
        if !matches!(
            self.state(),
            SaveState::FailedWrite | SaveState::FailedPreparation
        ) {
            return Err("只能明确放弃已失败的保存".into());
        }
        self.lock();
        self.pending = None;
        Ok(())
    }
    /// Only invoke after explicit user acknowledgement; never auto-discard on close.
    pub fn discard_failed_preparation(&mut self) -> Result<(), String> {
        if self.state() != SaveState::FailedPreparation {
            return Err("不能放弃进行中或已生成密文的保存".into());
        }
        self.discard_failed_save()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn fail_random(_: &mut [u8]) -> Result<(), String> {
        Err("无法取得安全随机数".into())
    }
    fn poll(c: &mut VaultController) -> VaultNotice {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(n) = c.poll() {
                return n;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn locked_preparation_failure_requires_explicit_acknowledgement() {
        let mut c = VaultController::new(Arc::new(|| {}));
        c.worker.random = fail_random;
        c.accept_create(
            1,
            Secret::new("Synthetic master password".into()),
            Secret::new("Synthetic master password".into()),
            VaultDocument::default(),
        )
        .unwrap();
        c.lock();
        assert!(matches!(poll(&mut c), VaultNotice::Failed(_)));
        assert_eq!(c.state(), SaveState::FailedPreparation);
        assert!(!c.can_shutdown());
        assert!(c.pending_document().is_none());
        assert!(c.retry().is_err());
        c.discard_failed_preparation().unwrap();
        assert!(c.can_shutdown());
        assert!(!c.session().is_unlocked());
    }
    #[test]
    fn unlocked_preparation_failure_retains_candidate_for_same_operation_retry() {
        let mut c = VaultController::new(Arc::new(|| {}));
        c.worker.random = fail_random;
        c.accept_create(
            7,
            Secret::new("Synthetic master password".into()),
            Secret::new("Synthetic master password".into()),
            VaultDocument::default(),
        )
        .unwrap();
        assert!(matches!(poll(&mut c), VaultNotice::Failed(_)));
        assert!(c.pending_document().is_some());
        c.worker.random = crate::account_vault::random;
        c.retry().unwrap();
        assert_eq!(poll(&mut c), VaultNotice::Prepared);
        c.dispatch_save(|command| {
            assert!(matches!(
                command,
                Command::WriteAccountVault {
                    request_id: 7,
                    expected_revision: 0,
                    ..
                }
            ));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            c.handle_event(&Event::AccountVaultWritten {
                request_id: 7,
                result: Ok(1)
            }),
            Some(VaultNotice::Saved)
        );
        assert!(c.session().is_unlocked());
    }
}
