//! One active AI call and one replaceable pending job, independent of persistence.
use crate::{
    ai::{AiPlan, AiSettings, FunctionDescriptor},
    model::{ItemSummary, Scope},
    ui_state::UiState,
};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
};

#[derive(Clone, Debug)]
pub struct AiRequest {
    pub settings: AiSettings,
    pub query: String,
    pub scope: Scope,
    pub functions: Vec<FunctionDescriptor>,
}
pub struct AiCompletion {
    pub id: u64,
    pub result: Result<AiPlan, String>,
}
#[derive(Default)]
struct Queue {
    pending: Option<(u64, AiRequest)>,
    result: Option<AiCompletion>,
}
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    generation: AtomicU64,
    stopped: AtomicBool,
}
pub struct AiWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct AiResultGate {
    active: Option<(u64, String, Scope)>,
}
impl AiResultGate {
    pub fn begin(&mut self, id: u64, query: &str, scope: Scope) {
        self.active = Some((id, query.into(), scope));
    }
    pub fn cancel(&mut self) {
        self.active = None;
    }
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }
    pub fn finish(&mut self, id: u64, query: &str, scope: Scope, active: bool) -> bool {
        let Some((expected, text, expected_scope)) = self.active.as_ref() else {
            return false;
        };
        if *expected != id {
            return false;
        }
        let current = active && text == query && *expected_scope == scope;
        self.active = None;
        current
    }
}

/// Clear an adopted local query mode separately from cancelling an in-flight HTTP call.
pub fn revoke_adopted_plan(
    plan: &mut Option<AiPlan>,
    state: &mut UiState,
    saved_items: &mut Vec<ItemSummary>,
    saved_ready: &mut bool,
) -> bool {
    if plan.take().is_none() {
        return false;
    }
    // HTTP may already have finished while the separate local AiSearch is still queued.
    state.next_request();
    saved_items.clear();
    *saved_ready = false;
    state.replace_ids(Vec::new());
    true
}

impl AiWorker {
    pub fn start(
        interpret: impl Fn(AiRequest) -> Result<AiPlan, String> + Send + 'static,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
        });
        let state = shared.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let job = {
                    let mut queue = state.queue.lock().unwrap();
                    while queue.pending.is_none() && !state.stopped.load(Ordering::Acquire) {
                        queue = state.wake.wait(queue).unwrap();
                    }
                    if state.stopped.load(Ordering::Acquire) {
                        return;
                    }
                    queue.pending.take().unwrap()
                };
                let result = interpret(job.1);
                let published = {
                    let mut queue = state.queue.lock().unwrap();
                    if state.stopped.load(Ordering::Acquire)
                        || state.generation.load(Ordering::Acquire) != job.0
                    {
                        false
                    } else {
                        queue.result = Some(AiCompletion { id: job.0, result });
                        true
                    }
                };
                if published
                    && !state.stopped.load(Ordering::Acquire)
                    && state.generation.load(Ordering::Acquire) == job.0
                {
                    notify();
                }
            }
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }
    pub fn submit(&self, request: AiRequest) -> Result<u64, String> {
        if !request.settings.enabled
            || request.settings.model.trim().is_empty()
            || request.settings.encrypted_key.is_empty()
        {
            return Err("请先启用并配置 AI 检索".into());
        }
        if request.settings.endpoint.len() > 2048
            || request.settings.model.chars().count() > 256
            || request.settings.encrypted_key.len() > 16 * 1024
        {
            return Err("AI 配置超过预算".into());
        }
        if request.query.trim().is_empty() || request.query.chars().count() > 2048 {
            return Err("请输入不超过 2048 字的检索描述".into());
        }
        if request.functions.len() > 32
            || request.functions.iter().any(|function| {
                function.id.len() > 128
                    || function.title.chars().count() > 256
                    || function.description.chars().count() > 1024
            })
        {
            return Err("AI 功能目录超过预算".into());
        }
        let id = self
            .shared
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let mut queue = self.shared.queue.lock().unwrap();
        queue.pending = Some((id, request));
        queue.result = None;
        self.shared.wake.notify_one();
        Ok(id)
    }
    pub fn cancel(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let mut queue = self.shared.queue.lock().unwrap();
        queue.pending = None;
        queue.result = None;
    }
    pub fn try_result(&self) -> Option<AiCompletion> {
        self.shared.queue.lock().unwrap().result.take()
    }
}
impl Drop for AiWorker {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.cancel();
        self.shared.wake.notify_all();
        // In-flight interpretation is non-durable; process exit never waits on it.
        self.thread.take();
    }
}
