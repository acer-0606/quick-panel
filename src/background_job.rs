//! Owned bounded jobs: one running job and one replaceable slot; drop never joins.
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

struct Slots<J, R> {
    job: Option<J>,
    result: Option<R>,
    stop: bool,
}
struct Shared<J, R> {
    slots: Mutex<Slots<J, R>>,
    wake: Condvar,
    occupied: AtomicBool,
}
pub struct BackgroundWorker<J, R> {
    shared: Arc<Shared<J, R>>,
}
impl<J: Send + 'static, R: Send + 'static> BackgroundWorker<J, R> {
    pub fn new(
        mut run: impl FnMut(J) -> R + Send + 'static,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let shared = Arc::new(Shared {
            slots: Mutex::new(Slots {
                job: None,
                result: None,
                stop: false,
            }),
            wake: Condvar::new(),
            occupied: AtomicBool::new(false),
        });
        let worker = shared.clone();
        std::thread::spawn(move || {
            loop {
                let job = {
                    let Ok(mut slots) = worker.slots.lock() else {
                        return;
                    };
                    while slots.job.is_none() && !slots.stop {
                        let Ok(next) = worker.wake.wait(slots) else {
                            return;
                        };
                        slots = next;
                    }
                    if slots.stop {
                        return;
                    }
                    slots.job.take().unwrap()
                };
                let result = run(job);
                let Ok(mut slots) = worker.slots.lock() else {
                    return;
                };
                if slots.stop {
                    return;
                }
                slots.result = Some(result);
                drop(slots);
                notify();
            }
        });
        Self { shared }
    }
    /// Latest preview replaces the sole waiting job. Never blocks on the handler.
    pub fn replace(&self, job: J) {
        if let Ok(mut slots) = self.shared.slots.lock()
            && !slots.stop
        {
            slots.job = Some(job);
            self.shared.wake.notify_one();
        }
    }
    /// Reliable action admission is held until its receipt is consumed.
    pub fn submit(&self, job: J) -> bool {
        if self.shared.occupied.swap(true, Ordering::AcqRel) {
            return false;
        }
        if let Ok(mut slots) = self.shared.slots.lock()
            && !slots.stop
        {
            slots.job = Some(job);
            self.shared.wake.notify_one();
            return true;
        }
        self.shared.occupied.store(false, Ordering::Release);
        false
    }
    pub fn busy(&self) -> bool {
        self.shared.occupied.load(Ordering::Acquire)
    }
    pub fn cancel_pending(&self) {
        if let Ok(mut slots) = self.shared.slots.lock() {
            slots.job = None;
        }
    }
    pub fn try_result(&self) -> Option<R> {
        let result = self.shared.slots.lock().ok()?.result.take()?;
        self.shared.occupied.store(false, Ordering::Release);
        Some(result)
    }
}
impl<J, R> Drop for BackgroundWorker<J, R> {
    fn drop(&mut self) {
        if let Ok(mut slots) = self.shared.slots.lock() {
            slots.stop = true;
            slots.job = None;
            slots.result = None;
        }
        self.shared.wake.notify_one();
    }
}
/// Commit linearizes execution immediately before the external effect. Cancellation
/// revokes pending tickets, while successful execution receipts remain observable.
#[derive(Default)]
pub struct ActionPermit {
    current: Mutex<Option<ActionTicket>>,
}
#[derive(Clone)]
pub struct ActionTicket {
    pub generation: u64,
    state: Arc<AtomicU8>,
}
impl ActionPermit {
    pub fn begin(&self, generation: u64) -> ActionTicket {
        self.cancel();
        let ticket = ActionTicket {
            generation,
            state: Arc::new(AtomicU8::new(0)),
        };
        if let Ok(mut current) = self.current.lock() {
            *current = Some(ticket.clone());
        }
        ticket
    }
    pub fn cancel(&self) {
        if let Ok(mut current) = self.current.lock()
            && let Some(ticket) = current.take()
        {
            let _ = ticket
                .state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}
impl ActionTicket {
    pub fn current(&self) -> bool {
        self.state.load(Ordering::Acquire) == 0
    }
    pub fn commit(&self) -> bool {
        self.state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub fn executed(&self) -> bool {
        self.state.load(Ordering::Acquire) == 1
    }
}

/// Engine Load admission remains occupied through cancellation until its actual receipt.
#[derive(Default)]
pub struct ActionLoads {
    inflight: Option<u64>,
}
impl ActionLoads {
    pub fn begin(&mut self, id: u64) -> bool {
        if self.busy() {
            return false;
        }
        self.inflight = Some(id);
        true
    }
    pub fn complete(&mut self, id: u64) {
        if self.inflight == Some(id) {
            self.inflight = None;
        }
    }
    pub fn busy(&self) -> bool {
        self.inflight.is_some()
    }
}
