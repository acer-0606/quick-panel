//! One active embedding call, one replaceable pending batch and one result slot.
use crate::{embedding, semantic::EmbeddingSettings};
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
};

#[derive(Clone, Debug)]
pub struct EmbeddingRequest {
    pub settings: EmbeddingSettings,
    pub inputs: Vec<String>,
}
pub struct EmbeddingCompletion {
    pub id: u64,
    pub result: Result<Vec<Vec<f32>>, String>,
}
#[derive(Default)]
struct Queue {
    pending: Option<(u64, EmbeddingRequest)>,
    result: Option<EmbeddingCompletion>,
}
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    generation: AtomicU64,
    stopped: AtomicBool,
}
pub struct EmbeddingWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl EmbeddingWorker {
    pub fn start(
        run: impl Fn(EmbeddingRequest) -> Result<Vec<Vec<f32>>, String> + Send + 'static,
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
                let result = run(job.1);
                let published = {
                    let mut queue = state.queue.lock().unwrap();
                    if state.stopped.load(Ordering::Acquire)
                        || state.generation.load(Ordering::Acquire) != job.0
                    {
                        false
                    } else {
                        queue.result = Some(EmbeddingCompletion { id: job.0, result });
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
    pub fn submit(&self, request: EmbeddingRequest) -> Result<u64, String> {
        embedding::validate_request(&request.settings, &request.inputs)?;
        if request.settings.encrypted_key.is_empty()
            || request.settings.encrypted_key.len() > 16 * 1024
        {
            return Err("请先配置有效的 Embedding API 密钥".into());
        }
        // Serialize generation changes with queue publication and other submitters.
        let mut queue = self.shared.queue.lock().unwrap();
        let id = self
            .shared
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        queue.pending = Some((id, request));
        queue.result = None;
        self.shared.wake.notify_one();
        Ok(id)
    }
    pub fn cancel(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        queue.pending = None;
        queue.result = None;
    }
    pub fn try_result(&self) -> Option<EmbeddingCompletion> {
        self.shared.queue.lock().unwrap().result.take()
    }
}
impl Drop for EmbeddingWorker {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        self.cancel();
        self.shared.wake.notify_all();
        // In-flight embedding is non-durable; process exit never waits on it.
        self.thread.take();
    }
}
