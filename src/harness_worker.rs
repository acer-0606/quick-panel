//! Bounded process bridge. Native consumers never wait for a process or HTTP.
use crate::{
    model::{Command, Event},
    organizer::Work,
    pi_harness::*,
};
use std::{
    collections::VecDeque,
    io::{BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command as Process, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct RuntimePaths {
    pub node: PathBuf,
    pub module: PathBuf,
}
impl RuntimePaths {
    pub fn bundled() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|_| "无法定位程序目录")?;
        let root = exe.parent().ok_or("无法定位程序目录")?;
        Ok(Self {
            node: root.join("runtime/node.exe"),
            module: root.join("runtime/pi-harness.mjs"),
        })
    }
}
// At most four relay slots; keep Command unboxed to avoid per-message allocation.
#[allow(clippy::large_enum_variant)]
pub enum Output {
    Actor(Command),
    Health(Result<(), String>),
}
type Transport = dyn Fn(&Work, ModelRequest) -> Result<ModelResponse, ToolError> + Send + Sync;
enum Awaiting {
    Context(u64),
    Tool(u64, String),
    Finish(u64),
}
impl Awaiting {
    fn matches(&self, event: &Event) -> bool {
        match (self, event) {
            (
                Self::Context(expected),
                Event::HarnessContext {
                    token, request_id, ..
                },
            ) => token == expected && request_id == expected,
            (Self::Tool(expected, call), Event::HarnessToolResult { token, call_id, .. }) => {
                token == expected && call_id == call
            }
            (
                Self::Finish(expected),
                Event::HarnessFinished {
                    token, request_id, ..
                },
            ) => token == expected && request_id == expected,
            _ => false,
        }
    }
}
struct State {
    awaiting: Mutex<Option<Awaiting>>,
    busy: AtomicBool,
    cancelled: AtomicBool,
    timed_out: AtomicBool,
    exiting: AtomicBool,
    child: Mutex<Option<OwnedChild>>,
    output: Mutex<VecDeque<Output>>,
    checkpoint: Mutex<Option<Command>>,
    reply: Mutex<Option<SyncSender<Event>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
pub struct HarnessWorker {
    paths: RuntimePaths,
    transport: Arc<Transport>,
    state: Arc<State>,
    timeout: Duration,
}
impl HarnessWorker {
    pub fn new(
        paths: RuntimePaths,
        transport: impl Fn(&Work, ModelRequest) -> Result<ModelResponse, ToolError>
        + Send
        + Sync
        + 'static,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            paths,
            transport: Arc::new(transport),
            timeout: Duration::from_millis(TASK_DEADLINE_MS.into()),
            state: Arc::new(State {
                busy: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                timed_out: AtomicBool::new(false),
                exiting: AtomicBool::new(false),
                awaiting: Mutex::new(None),
                child: Mutex::new(None),
                output: Mutex::new(VecDeque::new()),
                checkpoint: Mutex::new(None),
                reply: Mutex::new(None),
                notify,
            }),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout.min(Duration::from_millis(TASK_DEADLINE_MS.into()));
        self
    }
    pub fn busy(&self) -> bool {
        self.state.busy.load(Ordering::Acquire)
    }
    /// A cancelled exit may replace this instance only after real cleanup.
    pub fn retired_and_idle(&self) -> bool {
        self.state.exiting.load(Ordering::Acquire) && !self.busy()
    }

    pub fn start(&self, work: Work) -> Result<(), String> {
        if !work.organizer_settings.enabled {
            return Err("AI 管家尚未启用".into());
        }
        self.launch(Some(work))
    }
    /// Only the version handshake: no context, credentials or model request.
    pub fn health(&self) -> Result<(), String> {
        self.launch(None)
    }
    fn launch(&self, work: Option<Work>) -> Result<(), String> {
        if self.state.exiting.load(Ordering::Acquire)
            || self
                .state
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err("上一次 Pi 后台任务仍在结束，请稍后重试".into());
        }
        self.state.cancelled.store(false, Ordering::Release);
        self.state.timed_out.store(false, Ordering::Release);
        let (tx, rx) = mpsc::sync_channel(1);
        *self.state.reply.lock().unwrap() = Some(tx);
        let state = self.state.clone();
        let paths = self.paths.clone();
        let transport = self.transport.clone();
        let timeout = self.timeout;
        let spawn = thread::Builder::new()
            .name("pi-driver".into())
            .spawn(move || {
                let stopped = Arc::new(AtomicBool::new(false));
                let watcher_state = state.clone();
                let watcher_stopped = stopped.clone();
                let deadline = Instant::now() + timeout;
                let watchdog = thread::Builder::new()
                    .name("pi-deadline".into())
                    .spawn(move || {
                        while !watcher_stopped.load(Ordering::Acquire) {
                            if Instant::now() >= deadline {
                                let mut child = watcher_state.child.lock().unwrap();
                                // Recheck under the child lock: an old watchdog cannot kill a later job.
                                if !watcher_stopped.load(Ordering::Acquire) {
                                    watcher_state.timed_out.store(true, Ordering::Release);
                                    if let Some(child) = child.as_mut() {
                                        let _ = child.child.kill();
                                    }
                                }
                                break;
                            }
                            thread::sleep(Duration::from_millis(10));
                        }
                    });
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if watchdog.is_err() {
                        return Err(error("RUNTIME_UNAVAILABLE", "无法建立 Pi 任务时限"));
                    }
                    run(&state, &paths, work.as_ref(), &transport, rx, timeout)
                }))
                .unwrap_or_else(|_| Err(error("runtime", "Pi 后台任务异常，可重试")));
                stopped.store(true, Ordering::Release);
                let result = if state.timed_out.load(Ordering::Acquire) {
                    Err(error("deadline", "整理达到执行时限，可重试"))
                } else {
                    result
                };
                // A synchronous HTTP invocation has returned before reaching this point.
                // Process wait is strictly confined to this background thread.
                let owned_child = state.child.lock().unwrap().take();
                if let Some(mut child) = owned_child {
                    let _ = child.child.kill();
                    let _ = child.child.wait();
                }
                *state.reply.lock().unwrap() = None;
                let mut outputs = state.output.lock().unwrap();
                if !state.exiting.load(Ordering::Acquire) {
                    let output = if let Some(work) = work {
                        match result {
                            Ok(Some(proposals)) => Output::Actor(Command::OrganizerComplete {
                                token: work.token,
                                result: Ok(proposals),
                            }),
                            Ok(None) => Output::Actor(failed(
                                work.token,
                                error("incomplete", "Pi 未完成全部整理建议"),
                            )),
                            Err(err) => Output::Actor(failed(work.token, err)),
                        }
                    } else {
                        Output::Health(result.map(|_| ()).map_err(|e| e.message))
                    };
                    // Flush checkpoint before terminal in the same ordered relay.
                    if let Some(cp) = state.checkpoint.lock().unwrap().take() {
                        outputs.push_back(Output::Actor(cp));
                    }
                    outputs.push_back(output);
                }
                state.busy.store(false, Ordering::Release);
                drop(outputs);
                (state.notify)();
            });
        if spawn.is_err() {
            self.state.busy.store(false, Ordering::Release);
            *self.state.reply.lock().unwrap() = None;
            return Err("无法创建 Pi 后台任务".into());
        }
        Ok(())
    }
    pub fn reply(&self, event: Event) -> bool {
        if self.state.cancelled.load(Ordering::Acquire) {
            return false;
        }
        let mut awaiting = self.state.awaiting.lock().unwrap();
        if !awaiting.as_ref().is_some_and(|key| key.matches(&event)) {
            return false;
        }
        let accepted = self
            .state
            .reply
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|tx| tx.try_send(event).is_ok());
        if accepted {
            *awaiting = None;
        }
        accepted
    }
    pub fn take(&self) -> Option<Output> {
        let mut outputs = self.state.output.lock().unwrap();
        outputs.pop_front().or_else(|| {
            self.state
                .checkpoint
                .lock()
                .unwrap()
                .take()
                .map(Output::Actor)
        })
    }
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        if let Some(child) = self.state.child.lock().unwrap().as_mut() {
            let _ = child.child.kill();
        }
    }
    /// Revoke terminal delivery and take already-received checkpoint before actor shutdown.
    pub fn shutdown(&self) -> Option<Command> {
        self.state.exiting.store(true, Ordering::Release);
        self.cancel();
        let mut outputs = self.state.output.lock().unwrap();
        let mut cp = None;
        while let Some(output) = outputs.pop_front() {
            if let Output::Actor(command @ Command::HarnessCheckpoint { .. }) = output {
                cp = Some(command);
            }
        }
        self.state.checkpoint.lock().unwrap().take().or(cp)
    }
}
impl Drop for HarnessWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn error(code: &str, message: &str) -> ToolError {
    ToolError::new(code, message, false)
}
fn failed(token: u64, err: ToolError) -> Command {
    Command::HarnessFailed {
        token,
        code: err.code,
        message: err.message,
        retryable: err.retryable,
    }
}
fn check(state: &State, deadline: Instant) -> Result<(), ToolError> {
    if state.cancelled.load(Ordering::Acquire) {
        return Err(error("cancelled", "整理已取消"));
    }
    if Instant::now() >= deadline {
        return Err(error("deadline", "整理达到执行时限，可重试"));
    }
    Ok(())
}
fn emit(state: &State, command: Command) -> Result<(), ToolError> {
    if state.exiting.load(Ordering::Acquire) {
        return Err(error("cancelled", "正在退出"));
    }
    let mut output = state.output.lock().unwrap();
    if output.len() >= 4 {
        return Err(error("protocol", "Pi 消息队列已满"));
    }
    output.push_back(Output::Actor(command));
    drop(output);
    (state.notify)();
    Ok(())
}
fn reply(
    state: &State,
    rx: &Receiver<Event>,
    deadline: Instant,
    matches: impl Fn(&Event) -> bool,
) -> Result<Event, ToolError> {
    loop {
        check(state, deadline)?;
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(event) if matches(&event) => return Ok(event),
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err(error("runtime", "Pi 本地回复通道已关闭")),
        }
    }
}
struct OwnedChild {
    child: Child,
    #[cfg(windows)]
    _job: Job,
}
#[cfg(windows)]
struct Job(windows::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
fn spawn(paths: &RuntimePaths) -> Result<OwnedChild, ToolError> {
    if !paths.node.is_absolute() || !paths.module.is_absolute() {
        return Err(error(
            "RUNTIME_UNAVAILABLE",
            "Pi 运行时路径无效，请重新解压完整便携包",
        ));
    }
    let mut command = Process::new(&paths.node);
    command
        .arg("--no-addons")
        .arg(&paths.module)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Explicit OS-only allowlist. Never inherit HOME, PATH, NODE_*, API keys or config.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        for name in ["SystemRoot", "WINDIR", "SystemDrive"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
    let child = command.spawn().map_err(|_| {
        error(
            "RUNTIME_UNAVAILABLE",
            "无法启动 Pi，请重新解压包含 runtime 的完整便携包",
        )
    })?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{Foundation::HANDLE, System::JobObjects::*};
        let mut child = child;
        let job = unsafe { CreateJobObjectW(None, None) }
            .map(Job)
            .map_err(|_| {
                let _ = child.kill();
                let _ = child.wait();
                error("RUNTIME_UNAVAILABLE", "无法约束 Pi 子进程生命周期")
            })?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let setup = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            )
            .and_then(|_| AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())))
        };
        if setup.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error("RUNTIME_UNAVAILABLE", "无法约束 Pi 子进程生命周期"));
        }
        Ok(OwnedChild { child, _job: job })
    }
    #[cfg(not(windows))]
    {
        Ok(OwnedChild { child })
    }
}
fn send(input: &mut ChildStdin, frame: HostFrame) -> Result<(), ToolError> {
    let bytes =
        encode_host_frame(&frame).map_err(|_| error("protocol", "Pi 请求格式或容量无效"))?;
    input
        .write_all(&bytes)
        .and_then(|_| input.flush())
        .map_err(|_| error("eof", "Pi 输入通道已关闭"))
}
fn frame(
    state: &State,
    rx: &Receiver<Result<Option<RuntimeFrame>, ToolError>>,
    deadline: Instant,
) -> Result<RuntimeFrame, ToolError> {
    loop {
        check(state, deadline)?;
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Ok(Some(frame))) => return Ok(frame),
            Ok(Ok(None)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(error("eof", "Pi 运行时提前结束"));
            }
            Ok(Err(err)) => return Err(err),
            Err(mpsc::RecvTimeoutError::Timeout) => (),
        }
    }
}
fn run(
    state: &Arc<State>,
    paths: &RuntimePaths,
    work: Option<&Work>,
    transport: &Arc<Transport>,
    replies: Receiver<Event>,
    timeout: Duration,
) -> Result<Option<Vec<crate::organizer::Proposal>>, ToolError> {
    let deadline = Instant::now() + timeout;
    let context = if let Some(work) = work {
        *state.awaiting.lock().unwrap() = Some(Awaiting::Context(work.token));
        emit(
            state,
            Command::HarnessContext {
                token: work.token,
                request_id: work.token,
            },
        )?;
        let Event::HarnessContext { result, .. } = reply(
            state,
            &replies,
            deadline,
            |e| matches!(e, Event::HarnessContext { token, request_id, .. } if *token == work.token && *request_id == work.token),
        )?
        else {
            unreachable!()
        };
        Some(result?)
    } else {
        None
    };
    check(state, deadline)?;
    let mut child = spawn(paths)?;
    let mut input = child.child.stdin.take().unwrap();
    let stdout = child.child.stdout.take().unwrap();
    let stderr = child.child.stderr.take().unwrap();
    *state.child.lock().unwrap() = Some(child);
    check(state, deadline)?;
    let (frames_tx, frames) = mpsc::sync_channel(1);
    let stderr_tx = frames_tx.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let value = read_runtime_frame(&mut reader)
                .map_err(|_| error("protocol", "Pi 返回了无效或过大的消息"));
            let terminal = !matches!(value, Ok(Some(_)));
            if frames_tx.send(value).is_err() || terminal {
                break;
            }
        }
    });
    thread::spawn(move || {
        let mut reader = stderr;
        let mut bytes = [0; 4096];
        let mut count = 0;
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            count += n;
            if count > 64 * 1024 {
                let _ = stderr_tx.send(Err(error("response_limit", "Pi 错误输出超过容量")));
                break;
            }
        }
    });
    let ready = frame(
        state,
        &frames,
        deadline.min(Instant::now() + Duration::from_secs(5)),
    )
    .map_err(|_| error("RUNTIME_UNAVAILABLE", "Pi 启动测试失败，请检查完整便携包"))?;
    if !matches!(
        ready,
        RuntimeFrame::Ready {
            protocol: PROTOCOL_VERSION
        }
    ) {
        return Err(error("RUNTIME_UNAVAILABLE", "Pi 协议版本不兼容"));
    }
    let Some(work) = work else { return Ok(None) };
    let context = context.unwrap();
    let effort = match work.ai_settings.effort {
        crate::ai::Effort::Default => PublicEffort::Default,
        crate::ai::Effort::Low => PublicEffort::Low,
        crate::ai::Effort::Medium => PublicEffort::Medium,
        crate::ai::Effort::High => PublicEffort::High,
        crate::ai::Effort::XHigh => PublicEffort::XHigh,
        crate::ai::Effort::Max => PublicEffort::Max,
    };
    if context.model != work.ai_settings.model
        || context.effort != effort
        || context.max_rounds != work.organizer_settings.max_rounds
    {
        return Err(error("protocol", "Pi 上下文与已派发配置不匹配"));
    }
    let request_id = work.token.to_string();
    let expected_model = context.model.clone();
    let expected_effort = context.effort;
    let receipt = CompletionReceipt {
        accepted: true,
        submitted_indices: context.item_refs.iter().map(|i| i.index).collect(),
    };
    send(
        &mut input,
        HostFrame::Start {
            protocol: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            model: context.model,
            effort: context.effort,
            instructions: context.instructions,
            item_refs: context.item_refs,
            max_rounds: context.max_rounds,
            checkpoint: context.checkpoint,
        },
    )?;
    let mut finished = None;
    let mut calls = std::collections::HashSet::new();
    loop {
        let received = frame(state, &frames, deadline)?;
        if calls.len() > MAX_TOOL_CALLS as usize + 8 {
            return Err(error("call_limit", "Pi 调用次数达到上限"));
        }
        match received {
            RuntimeFrame::Llm {
                request_id: id,
                call_id,
                model,
                effort,
                request,
            } if id == request_id && finished.is_none() => {
                if model != expected_model
                    || effort != expected_effort
                    || !calls.insert(call_id.clone())
                {
                    return Err(error("protocol", "Pi 模型或调用标识不匹配"));
                }
                let result = transport(work, request);
                check(state, deadline)?;
                let result = match result {
                    Ok(response) => {
                        response
                            .validate()
                            .map_err(|_| error("protocol", "模型返回格式无效"))?;
                        HostResult::success(response)
                    }
                    Err(err) => HostResult::failure(err),
                };
                send(
                    &mut input,
                    HostFrame::LlmResult {
                        request_id: id,
                        call_id,
                        result,
                    },
                )?;
            }
            RuntimeFrame::Tool {
                request_id: id,
                call_id,
                name,
                arguments,
            } if id == request_id && finished.is_none() => {
                if !calls.insert(call_id.clone()) {
                    return Err(error("protocol", "Pi 重复调用标识"));
                }
                *state.awaiting.lock().unwrap() = Some(Awaiting::Tool(work.token, call_id.clone()));
                emit(
                    state,
                    Command::HarnessTool {
                        token: work.token,
                        call_id: call_id.clone(),
                        name,
                        arguments,
                    },
                )?;
                let Event::HarnessToolResult { result, .. } = reply(
                    state,
                    &replies,
                    deadline,
                    |e| matches!(e, Event::HarnessToolResult { token, call_id: cid, .. } if *token == work.token && *cid == call_id),
                )?
                else {
                    unreachable!()
                };
                send(
                    &mut input,
                    HostFrame::ToolResult {
                        request_id: id,
                        call_id,
                        result,
                    },
                )?;
            }
            RuntimeFrame::Checkpoint {
                request_id: id,
                checkpoint,
            } if id == request_id => {
                let mut latest = state.checkpoint.lock().unwrap();
                if !state.exiting.load(Ordering::Acquire) {
                    *latest = Some(Command::HarnessCheckpoint {
                        token: work.token,
                        checkpoint,
                    });
                }
                drop(latest);
                (state.notify)();
            }
            RuntimeFrame::Finish {
                request_id: id,
                call_id,
            } if id == request_id && finished.is_none() => {
                let cp = state.checkpoint.lock().unwrap().take();
                if let Some(cp) = cp {
                    emit(state, cp)?;
                }
                *state.awaiting.lock().unwrap() = Some(Awaiting::Finish(work.token));
                emit(
                    state,
                    Command::HarnessFinish {
                        token: work.token,
                        request_id: work.token,
                    },
                )?;
                let Event::HarnessFinished { result, .. } = reply(
                    state,
                    &replies,
                    deadline,
                    |e| matches!(e, Event::HarnessFinished { token, request_id, .. } if *token == work.token && *request_id == work.token),
                )?
                else {
                    unreachable!()
                };
                let result = match result {
                    Ok(proposals) => {
                        finished = Some(proposals);
                        HostResult::success(receipt.clone())
                    }
                    Err(_) => {
                        HostResult::failure(error("incomplete", "来源已变化或整理未覆盖全部内容"))
                    }
                };
                send(
                    &mut input,
                    HostFrame::FinishResult {
                        request_id: id,
                        call_id,
                        result,
                    },
                )?;
            }
            RuntimeFrame::Completed {
                request_id: id,
                receipt: actual,
            } if id == request_id && actual == receipt && finished.is_some() => {
                return Ok(finished);
            }
            RuntimeFrame::Failed {
                request_id: Some(id),
                error,
            } if id == request_id => return Err(localize_failure(error)),
            _ => return Err(error("protocol", "Pi 返回了过期或顺序无效的消息")),
        }
    }
}

/// Exact Messages envelope uses only the actor's dispatched settings.
pub fn prepare_model(
    work: &Work,
    key: &str,
    request: &ModelRequest,
) -> Result<crate::claude::HttpRequest, ToolError> {
    request
        .validate()
        .map_err(|_| error("protocol", "模型请求格式无效"))?;
    crate::claude::validate_model(&work.ai_settings.model)
        .map_err(|_| error("configuration", "模型配置无效"))?;
    let endpoint = crate::claude::parse_endpoint(&work.ai_settings.endpoint)
        .map_err(|_| error("configuration", "API 地址无效"))?;
    let headers = crate::claude::request_headers(&work.ai_settings, key)
        .map_err(|_| error("AUTH", "API 凭据无效"))?;
    let mut body =
        serde_json::to_value(request).map_err(|_| error("protocol", "无法编码模型请求"))?;
    body["model"] = work.ai_settings.model.clone().into();
    body["stream"] = false.into();
    if let Some(effort) = work.ai_settings.effort.wire_name() {
        body["output_config"] = serde_json::json!({"effort":effort});
    }
    let body = serde_json::to_vec(&body).map_err(|_| error("protocol", "无法编码模型请求"))?;
    if body.len() > MAX_LLM_REQUEST_BYTES {
        return Err(error("request_limit", "模型请求超过容量"));
    }
    Ok(crate::claude::HttpRequest {
        endpoint,
        headers,
        body,
    })
}
pub fn normalize_model(status: u16, body: &[u8]) -> Result<ModelResponse, ToolError> {
    if matches!(status, 401 | 403) {
        return Err(error("AUTH", "API 认证失败，请检查密钥和权限"));
    }
    if matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529) {
        return Err(ToolError::new(
            "TRANSIENT_NETWORK",
            "模型服务暂时不可用，将稍后重试",
            true,
        ));
    }
    if !(200..300).contains(&status) {
        return Err(error("http", "模型服务拒绝请求，请检查配置"));
    }
    if body.len() > MAX_LLM_RESPONSE_BYTES {
        return Err(error("response_limit", "模型响应超过 64 KiB"));
    }
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| error("protocol", "模型响应不是有效 JSON"))?;
    // Do not pass provider envelope fields (usage/id/etc.) through to the child.
    let response: ModelResponse = serde_json::from_value(
        serde_json::json!({"content":value["content"],"stop_reason":value["stop_reason"]}),
    )
    .map_err(|_| error("protocol", "模型响应结构无效"))?;
    response
        .validate()
        .map_err(|_| error("protocol", "模型响应内容无效"))?;
    Ok(response)
}

fn localize_failure(mut failure: ToolError) -> ToolError {
    failure.message = match failure.code.as_str() {
        "AUTH" => "API 认证失败，请检查密钥和权限",
        "RUNTIME_UNAVAILABLE" => "Pi 运行时不可用，请重新解压完整便携包",
        "TRANSIENT_NETWORK" => "网络或模型服务暂时不可用，将稍后重试",
        "round_limit" => "本批已达到模型轮次上限，可反馈或重试",
        "call_limit" => "本批已达到工具调用上限，可反馈或重试",
        "deadline" => "本批已达到执行时限，可反馈或重试",
        "incomplete" => "本批未完成全部整理建议，可反馈或重试",
        "cancelled" => "整理已取消",
        "frame_limit" | "request_limit" | "response_limit" | "tool_result_limit"
        | "checkpoint_limit" => "本批消息超过容量限制，可减少批次或重试",
        "STALE_SOURCE" | "STALE_RUN" => "来源或设置已变化，请使用新任务",
        "STORAGE" | "CAPACITY" => "本地整理状态无法保存，请检查存储或清理记忆",
        "protocol" => "Pi 返回了无效消息，请检查运行时或 API 兼容性",
        _ => "本批整理未完成，请检查配置后重试",
    }
    .into();
    failure
}
