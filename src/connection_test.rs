//! Bounded, transient tests for settings forms, independent of application persistence.
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, PartialEq, Eq)]
pub enum CredentialSource {
    Entered,
    Saved,
}
/// Choose a credential without copying it or treating a cleared key as available.
pub fn credential_source(
    entered: bool,
    clear: bool,
    saved: bool,
) -> Result<CredentialSource, String> {
    if clear {
        return Err(if entered {
            "清除密钥与填写新密钥请选择其中一个操作"
        } else {
            "已选择清除密钥，请取消清除后再测试连接"
        }
        .into());
    }
    if entered {
        Ok(CredentialSource::Entered)
    } else if saved {
        Ok(CredentialSource::Saved)
    } else {
        Err("请填写 API Key 后再测试连接".into())
    }
}
pub struct ConnectionTestCompletion<Value> {
    pub id: u64,
    pub result: Result<Value, String>,
    pub elapsed: Duration,
}
struct State<Value> {
    next_id: u64,
    running: Option<u64>,
    cancelled: bool,
    closed: bool,
    result: Option<ConnectionTestCompletion<Value>>,
}
pub struct ConnectionTestWorker<Request, Value> {
    state: Arc<Mutex<State<Value>>>,
    run: Arc<dyn Fn(Request) -> Result<Value, String> + Send + Sync>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl<Request: Send + 'static, Value: Send + 'static> ConnectionTestWorker<Request, Value> {
    /// Construct without creating a thread or executing a request.
    pub fn new(
        run: impl Fn(Request) -> Result<Value, String> + Send + Sync + 'static,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                next_id: 0,
                running: None,
                cancelled: false,
                closed: false,
                result: None,
            })),
            run: Arc::new(run),
            notify,
        }
    }
    pub fn submit(&self, request: Request) -> Result<u64, String> {
        let mut state = self.state.lock().unwrap();
        if state.running.is_some() {
            return Err("上一次连接测试仍在进行，请稍后再试".into());
        }
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        state.running = Some(id);
        state.cancelled = false;
        state.result = None;
        let shared = self.state.clone();
        let run = self.run.clone();
        let notify = self.notify.clone();
        let spawn = std::thread::Builder::new()
            .name("connection-test".into())
            .spawn(move || {
                let start = Instant::now();
                let result = run(request);
                let elapsed = start.elapsed();
                {
                    let mut state = shared.lock().unwrap();
                    state.running = None;
                    if !state.cancelled && !state.closed {
                        state.result = Some(ConnectionTestCompletion {
                            id,
                            result,
                            elapsed,
                        });
                    }
                }
                // A completion notification owns no dialog pointer or window handle.
                notify();
            });
        if spawn.is_err() {
            state.running = None;
            return Err("无法启动连接测试，请稍后再试".into());
        }
        // Dropping JoinHandle detaches the single call; closing a form never joins it.
        Ok(id)
    }
    pub fn cancel(&self, id: u64) {
        let mut state = self.state.lock().unwrap();
        if state.running == Some(id) {
            state.cancelled = true;
        }
        if state.result.as_ref().is_some_and(|result| result.id == id) {
            state.result = None;
        }
    }
    pub fn busy(&self) -> bool {
        self.state.lock().unwrap().running.is_some()
    }
    pub fn try_result(&self) -> Option<ConnectionTestCompletion<Value>> {
        self.state.lock().unwrap().result.take()
    }
}
impl<Request, Value> Drop for ConnectionTestWorker<Request, Value> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.result = None;
    }
}
#[derive(Default)]
pub struct ConnectionTestGate {
    active: Option<u64>,
}
impl ConnectionTestGate {
    pub fn begin(&mut self, id: u64) {
        self.active = Some(id);
    }
    pub fn finish(&mut self, id: u64) -> bool {
        if self.active != Some(id) {
            return false;
        }
        self.active = None;
        true
    }
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }
    pub fn cancel(&mut self) -> Option<u64> {
        self.active.take()
    }
}
