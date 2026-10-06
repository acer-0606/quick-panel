use quick_panel::harness_worker::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn paths() -> RuntimePaths {
    RuntimePaths {
        node: std::env::var_os("QP_TEST_NODE").map(std::path::PathBuf::from).unwrap_or_else(|| {
            if cfg!(windows) { std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dist/QuickPanel-Windows-x64/runtime/node.exe") }
            else { std::path::PathBuf::from("/Users/chuwujie/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node") }
        }),
        module: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("agent-runtime/dist/worker.mjs"),
    }
}
fn worker(paths: RuntimePaths) -> HarnessWorker {
    HarnessWorker::new(
        paths,
        |_, _| panic!("health must not call model"),
        Arc::new(|| {}),
    )
}
fn wait(worker: &HarnessWorker) -> Output {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(output) = worker.take() {
            return output;
        }
        assert!(Instant::now() < until, "worker did not terminate");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn real_runtime_health_handshake_never_calls_model() {
    let worker = worker(paths());
    assert!(!worker.busy());
    worker.health().unwrap();
    assert!(matches!(wait(&worker), Output::Health(Ok(()))));
    assert!(!worker.busy());
}
#[test]
fn missing_runtime_has_local_terminal_and_releases_actual_slot() {
    let mut p = paths();
    p.node = "/nonexistent/pi/node".into();
    let worker = worker(p);
    worker.health().unwrap();
    assert!(matches!(wait(&worker), Output::Health(Err(_))));
    assert!(!worker.busy());
}
#[test]
fn disabled_work_never_spawns() {
    let worker = worker(paths());
    let work = quick_panel::organizer::Work {
        token: 1,
        ai_settings: Default::default(),
        organizer_settings: Default::default(),
        candidates: vec![],
    };
    assert!(worker.start(work).is_err());
    assert!(!worker.busy());
    assert!(worker.take().is_none());
}

#[test]
fn real_pi_child_channels_and_actor_finish_before_completed_once() {
    use quick_panel::{engine::Engine, model::*, pi_harness::*};
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        EngineOptions::new(dir.path().join("fixture.db")),
        Arc::new(|| {}),
    )
    .unwrap();
    let mut settings = Settings::default();
    settings.organizer.enabled = true;
    settings.ai.model = "fixture".into();
    settings.ai.encrypted_key = b"synthetic".to_vec();
    engine.send(Command::SaveSettings(settings)).unwrap();
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: Some("synthetic-folder".into()),
            kind: ItemKind::Folder,
            title: "合成项目".into(),
            target: "C:/Synthetic/project".into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
    engine.send(Command::OrganizerTick).unwrap();
    let end = Instant::now() + Duration::from_secs(8);
    let work = loop {
        if let Ok(Event::OrganizerWork(work)) = engine.try_recv() {
            break work;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    };
    let version = work.candidates[0].expected_version.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let capture = calls.clone();
    let worker = HarnessWorker::new(
        paths(),
        move |_, request| {
            request.validate().unwrap();
            let n = capture.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(if n == 0 {
                ModelResponse {
                    content: vec![
                        json!({"type":"tool_use","id":"submit-1","name":"organizer_submit","input":{"index":0,"expected_version":version,"alias":"合成别名","tags":[],"reason":"合成测试"}}),
                    ],
                    stop_reason: "tool_use".into(),
                }
            } else {
                ModelResponse {
                    content: vec![json!({"type":"text","text":"完成"})],
                    stop_reason: "end_turn".into(),
                }
            })
        },
        Arc::new(|| {}),
    );
    worker.start(work).unwrap();
    let mut finish = false;
    let mut complete = 0;
    while complete == 0 {
        while let Some(output) = worker.take() {
            if let Output::Actor(command) = output {
                if let Command::HarnessTool { token, .. } = &command {
                    assert!(!worker.reply(Event::HarnessToolResult {
                        token: *token,
                        call_id: "late-call".into(),
                        result: HostResult::success(json!({}))
                    }));
                }
                if matches!(command, Command::HarnessFinish { .. }) {
                    finish = true;
                }
                if matches!(command, Command::OrganizerComplete { .. }) {
                    assert!(finish);
                    complete += 1;
                }
                if let Command::HarnessFailed { code, message, .. } = &command {
                    panic!("{code}: {message}");
                }
                engine.send(command).unwrap();
            }
        }
        while let Ok(event) = engine.try_recv() {
            worker.reply(event);
        }
        assert!(Instant::now() < end, "Pi bridge timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!worker.busy());
    assert_eq!(complete, 1);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    engine
        .send(Command::OrganizerSnapshot { request_id: 99 })
        .unwrap();
    loop {
        if let Ok(Event::OrganizerState { snapshot, .. }) = engine.try_recv() {
            assert_eq!(snapshot.proposals.len(), 1);
            assert_eq!(snapshot.proposals[0].alias.as_deref(), Some("合成别名"));
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    engine.send(Command::Shutdown).unwrap();
}

fn synthetic_work() -> quick_panel::organizer::Work {
    let mut work = quick_panel::organizer::Work {
        token: 91,
        ai_settings: Default::default(),
        organizer_settings: Default::default(),
        candidates: vec![],
    };
    work.organizer_settings.enabled = true;
    work.ai_settings.model = "fixture".into();
    work
}
fn context_reply(worker: &HarnessWorker) {
    use quick_panel::{
        harness_tools::PiContext,
        model::{Command, Event},
        pi_harness::*,
    };
    let Output::Actor(Command::HarnessContext { token, request_id }) = wait(worker) else {
        panic!("missing context")
    };
    assert!(
        !worker.reply(Event::HarnessContext {
            token: token + 1,
            request_id,
            result: Err(ToolError::new("stale", "stale", false))
        }),
        "late tokens must not occupy reply slot"
    );
    assert!(worker.reply(Event::HarnessContext {
        token,
        request_id,
        result: Ok(PiContext {
            model: "fixture".into(),
            effort: PublicEffort::Default,
            instructions: String::new(),
            item_refs: vec![ItemRef {
                index: 0,
                kind: "folder".into(),
                title: "合成".into(),
                version: "v1".into()
            }],
            max_rounds: 4,
            checkpoint: None
        })
    }));
}
#[test]
fn cancellation_keeps_slot_until_actual_transport_returns_and_shutdown_never_waits() {
    use quick_panel::pi_harness::*;
    let (entered, enter) = std::sync::mpsc::sync_channel(1);
    let (release, gate) = std::sync::mpsc::sync_channel(1);
    let gate = std::sync::Mutex::new(gate);
    let worker = HarnessWorker::new(
        paths(),
        move |_, _| {
            entered.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            Ok(ModelResponse {
                content: vec![],
                stop_reason: "end_turn".into(),
            })
        },
        Arc::new(|| {}),
    );
    worker.start(synthetic_work()).unwrap();
    context_reply(&worker);
    enter.recv_timeout(Duration::from_secs(3)).unwrap();
    worker.cancel();
    assert!(worker.busy());
    assert!(worker.start(synthetic_work()).is_err());
    let start = Instant::now();
    worker.shutdown();
    assert!(start.elapsed() < Duration::from_millis(100));
    assert!(worker.busy());
    assert!(!worker.retired_and_idle());
    release.send(()).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while worker.busy() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(worker.take().is_none());
    assert!(
        worker.retired_and_idle(),
        "cancelled exit can recreate an idle worker"
    );
    drop(worker);
    let replacement = HarnessWorker::new(
        paths(),
        |_, _| panic!("health must not call model"),
        Arc::new(|| {}),
    );
    replacement.health().unwrap();
    assert!(matches!(wait(&replacement), Output::Health(Ok(()))));
    assert!(!replacement.busy());
}
#[test]
fn controlled_child_timeout_eof_malformed_overflow_and_clean_environment() {
    for script in [
        "process.exit(0)",
        "console.log('not json')",
        "console.log('x'.repeat(262145))",
        "process.stderr.write('x'.repeat(70000));setInterval(()=>{},1000)",
        "setInterval(()=>{},1000)",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("fixture.mjs");
        std::fs::write(&module, script).unwrap();
        let mut p = paths();
        p.module = module;
        let worker = worker(p).with_timeout(Duration::from_millis(200));
        worker.health().unwrap();
        assert!(matches!(wait(&worker), Output::Health(Err(_))));
        assert!(!worker.busy());
    }
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("env.mjs");
    std::fs::write(&module,"if(Object.keys(process.env).some(k=>!['SystemRoot','WINDIR','SystemDrive','__CF_USER_TEXT_ENCODING'].includes(k)))process.exit(1);console.log(JSON.stringify({type:'ready',protocol:1}));setInterval(()=>{},1000)").unwrap();
    let mut p = paths();
    p.module = module;
    let worker = worker(p);
    worker.health().unwrap();
    assert!(matches!(wait(&worker), Output::Health(Ok(()))));
}
#[test]
fn model_normalization_preserves_signed_thinking_and_classifies_status_exactly() {
    use serde_json::json;
    let body = json!({"id":"provider-envelope","usage":{"secret":"ignored"},"content":[{"type":"thinking","thinking":"合成推理","signature":"signed"},{"type":"redacted_thinking","data":"redacted"},{"type":"tool_use","id":"c1","name":"organizer_submit","input":{"index":0}}],"stop_reason":"tool_use"});
    let response = normalize_model(200, &serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(
        response.content,
        body["content"].as_array().unwrap().clone()
    );
    assert_eq!(response.stop_reason, "tool_use");
    assert_eq!(normalize_model(401, b"ignored").unwrap_err().code, "AUTH");
    assert!(normalize_model(429, b"ignored").unwrap_err().retryable);
    assert_eq!(normalize_model(400, b"ignored").unwrap_err().code, "http");
    assert_eq!(normalize_model(200, b"bad").unwrap_err().code, "protocol");
    assert_eq!(
        normalize_model(200, &vec![b'x'; 65537]).unwrap_err().code,
        "response_limit"
    );
}
#[test]
fn final_settled_checkpoint_precedes_typed_failure_and_preserves_elapsed_budget() {
    use quick_panel::{model::Command, pi_harness::ToolError};
    let worker = HarnessWorker::new(
        paths(),
        |_, _| {
            std::thread::sleep(Duration::from_millis(90));
            Err(ToolError::new("TRANSIENT_NETWORK", "synthetic retry", true))
        },
        Arc::new(|| {}),
    );
    worker.start(synthetic_work()).unwrap();
    context_reply(&worker);
    let end = Instant::now() + Duration::from_secs(3);
    while worker.busy() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut last = None;
    let mut count = 0;
    while let Some(output) = worker.take() {
        match output {
            Output::Actor(Command::HarnessCheckpoint { checkpoint, .. }) => {
                checkpoint.validate().unwrap();
                last = Some(checkpoint);
                count += 1;
            }
            Output::Actor(Command::HarnessFailed {
                code, retryable, ..
            }) => {
                assert_eq!(code, "TRANSIENT_NETWORK");
                assert!(retryable);
                let cp = last.take().expect("checkpoint before failure");
                assert_eq!(cp.rounds, 1);
                assert!(cp.elapsed_ms >= 80);
            }
            _ => panic!("unexpected output"),
        }
    }
    assert_eq!(count, 1, "checkpoint relay must coalesce");
    assert!(last.is_none());
}
#[test]
fn post_handshake_eof_protocol_timeout_and_transport_panic_are_local_terminals() {
    use quick_panel::model::Command;
    for (script, expected) in [
        (
            "console.log('{\"type\":\"ready\",\"protocol\":1}');process.stdin.once('data',()=>process.exit(0))",
            "eof",
        ),
        (
            "console.log('{\"type\":\"ready\",\"protocol\":1}');process.stdin.once('data',()=>console.log('bad frame'))",
            "protocol",
        ),
        (
            "console.log('{\"type\":\"ready\",\"protocol\":1}');setInterval(()=>{},1000)",
            "deadline",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("running.mjs");
        std::fs::write(&module, script).unwrap();
        let mut p = paths();
        p.module = module;
        let worker = worker(p).with_timeout(Duration::from_millis(300));
        worker.start(synthetic_work()).unwrap();
        context_reply(&worker);
        let Output::Actor(Command::HarnessFailed {
            code, retryable, ..
        }) = wait(&worker)
        else {
            panic!("missing failure")
        };
        assert_eq!(code, expected);
        assert!(!retryable);
        assert!(!worker.busy());
    }
    let worker = HarnessWorker::new(
        paths(),
        |_, _| panic!("synthetic transport panic"),
        Arc::new(|| {}),
    );
    worker.start(synthetic_work()).unwrap();
    context_reply(&worker);
    loop {
        if let Output::Actor(Command::HarnessFailed { code, .. }) = wait(&worker) {
            assert_eq!(code, "runtime");
            break;
        }
    }
    assert!(!worker.busy());
}
#[cfg(unix)]
#[test]
fn owned_child_has_actually_exited_before_health_result_is_released() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let module = dir.path().join("owned.mjs");
    std::fs::write(&module,format!("import{{writeFileSync}}from'node:fs';writeFileSync({},String(process.pid));console.log('{{\"type\":\"ready\",\"protocol\":1}}');setInterval(()=>{{}},1000)",serde_json::to_string(&pid_file).unwrap())).unwrap();
    let mut p = paths();
    p.module = module;
    let worker = worker(p);
    worker.health().unwrap();
    assert!(matches!(wait(&worker), Output::Health(Ok(()))));
    let pid = std::fs::read_to_string(pid_file).unwrap();
    let exists = std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!exists);
    assert!(!worker.busy());
}
#[test]
fn deadline_interrupts_a_child_that_never_reads_a_large_start_frame() {
    use quick_panel::{
        harness_tools::PiContext,
        model::{Command, Event},
        pi_harness::*,
    };
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("blocked-input.mjs");
    std::fs::write(
        &module,
        "console.log('{\"type\":\"ready\",\"protocol\":1}');setInterval(()=>{},1000)",
    )
    .unwrap();
    let mut p = paths();
    p.module = module;
    let worker = worker(p).with_timeout(Duration::from_millis(200));
    worker.start(synthetic_work()).unwrap();
    let Output::Actor(Command::HarnessContext { token, request_id }) = wait(&worker) else {
        panic!()
    };
    let tools=["items_get_context","items_search","memory_recall","usage_get_summary","organizer_submit","memory_propose_rule"].into_iter().map(|name|serde_json::json!({"name":name,"description":"fixture","parameters":{"type":"object"}})).collect::<Vec<_>>();
    let checkpoint=Checkpoint{messages_json:serde_json::json!([{"role":"system","content":"x".repeat(100000),"toolsAdded":tools,"timestamp":0},{"role":"user","content":"fixture","timestamp":0}]).to_string(),rounds:0,calls:0,elapsed_ms:0};
    checkpoint.validate().unwrap();
    assert!(worker.reply(Event::HarnessContext {
        token,
        request_id,
        result: Ok(PiContext {
            model: "fixture".into(),
            effort: PublicEffort::Default,
            instructions: String::new(),
            item_refs: vec![ItemRef {
                index: 0,
                kind: "folder".into(),
                title: "合成".into(),
                version: "v1".into()
            }],
            max_rounds: 4,
            checkpoint: Some(checkpoint)
        })
    }));
    let end = Instant::now() + Duration::from_secs(2);
    while worker.busy() {
        assert!(Instant::now() < end, "stdin write ignored deadline");
        std::thread::sleep(Duration::from_millis(5));
    }
    let Output::Actor(Command::HarnessFailed { code, .. }) = wait(&worker) else {
        panic!()
    };
    assert_eq!(code, "deadline");
}

fn settled_runtime_checkpoint() -> quick_panel::pi_harness::Checkpoint {
    use quick_panel::{model::Command, pi_harness::ToolError};
    let worker = HarnessWorker::new(
        paths(),
        |_, _| Err(ToolError::new("TRANSIENT_NETWORK", "synthetic", true)),
        Arc::new(|| {}),
    );
    worker.start(synthetic_work()).unwrap();
    context_reply(&worker);
    let mut latest = None;
    loop {
        match wait(&worker) {
            Output::Actor(Command::HarnessCheckpoint { checkpoint, .. }) => {
                latest = Some(checkpoint)
            }
            Output::Actor(Command::HarnessFailed { .. }) => return latest.unwrap(),
            _ => panic!("unexpected output"),
        }
    }
}
fn resume_context(worker: &HarnessWorker, checkpoint: quick_panel::pi_harness::Checkpoint) {
    use quick_panel::{
        harness_tools::PiContext,
        model::{Command, Event},
        pi_harness::*,
    };
    let Output::Actor(Command::HarnessContext { token, request_id }) = wait(worker) else {
        panic!()
    };
    assert!(worker.reply(Event::HarnessContext {
        token,
        request_id,
        result: Ok(PiContext {
            model: "fixture".into(),
            effort: PublicEffort::Default,
            instructions: String::new(),
            item_refs: vec![ItemRef {
                index: 0,
                kind: "folder".into(),
                title: "合成".into(),
                version: "v1".into()
            }],
            max_rounds: 4,
            checkpoint: Some(checkpoint)
        })
    }));
}
#[test]
fn restored_near_deadline_collects_final_checkpoint_while_transport_is_still_blocked() {
    use quick_panel::{model::Command, pi_harness::ToolError};
    let mut cp = settled_runtime_checkpoint();
    cp.elapsed_ms = 119_750;
    cp.validate().unwrap();
    let (entered, enter) = std::sync::mpsc::sync_channel(1);
    let (release, gate) = std::sync::mpsc::sync_channel(1);
    let gate = std::sync::Mutex::new(gate);
    let worker = HarnessWorker::new(
        paths(),
        move |_, _| {
            entered.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            Err(ToolError::new(
                "TRANSIENT_NETWORK",
                "late synthetic transport error",
                true,
            ))
        },
        Arc::new(|| {}),
    );
    worker.start(synthetic_work()).unwrap();
    resume_context(&worker, cp);
    enter.recv_timeout(Duration::from_secs(3)).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(worker.busy());
    let mut checkpoints = vec![];
    while let Some(out) = worker.take() {
        if let Output::Actor(Command::HarnessCheckpoint { checkpoint, .. }) = out {
            checkpoints.push(checkpoint);
        }
    }
    let received_while_blocked = checkpoints.iter().any(|cp| cp.elapsed_ms == 120_000);
    release.send(()).unwrap();
    let failure = loop {
        match wait(&worker) {
            Output::Actor(Command::HarnessCheckpoint { checkpoint, .. }) => {
                checkpoints.push(checkpoint)
            }
            Output::Actor(Command::HarnessFailed {
                code, retryable, ..
            }) => break (code, retryable),
            _ => panic!("unexpected terminal"),
        }
    };
    assert_eq!(
        failure,
        ("deadline".into(), false),
        "Node deadline must outrank late HTTP/broken-pipe fallback"
    );
    let final_cp = checkpoints.last().unwrap();
    final_cp.validate().unwrap();
    assert_eq!(final_cp.elapsed_ms, 120_000);
    assert_eq!(final_cp.rounds, 2);
    assert!(
        received_while_blocked,
        "already-decoded checkpoint must not wait behind HTTP"
    );
    assert!(!worker.busy());
}
#[test]
fn independent_collector_rejects_foreign_and_unsettled_checkpoints() {
    use quick_panel::model::Command;
    let cp = settled_runtime_checkpoint();
    for foreign in [true, false] {
        let mut bad = cp.clone();
        if !foreign {
            let mut messages: Vec<serde_json::Value> =
                serde_json::from_str(&bad.messages_json).unwrap();
            messages.push(serde_json::json!({"role":"toolResult","toolCallId":"orphan","toolName":"items_get_context","content":[],"isError":false,"timestamp":0}));
            bad.messages_json = serde_json::to_string(&messages).unwrap();
            assert!(bad.validate().is_err());
        }
        let frame = serde_json::json!({"type":"checkpoint","request_id":if foreign {"foreign"}else{"91"},"checkpoint":bad});
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("invalid-cp.mjs");
        std::fs::write(&module,format!("console.log('{{\"type\":\"ready\",\"protocol\":1}}');process.stdin.once('data',()=>console.log(JSON.stringify({frame})));setInterval(()=>{{}},1000)")).unwrap();
        let mut p = paths();
        p.module = module;
        let worker = worker(p).with_timeout(Duration::from_secs(2));
        worker.start(synthetic_work()).unwrap();
        context_reply(&worker);
        let Output::Actor(Command::HarnessFailed {
            code, retryable, ..
        }) = wait(&worker)
        else {
            panic!("invalid checkpoint must never reach actor")
        };
        assert_eq!(code, "protocol");
        assert!(!retryable);
        assert!(worker.take().is_none());
    }
}

// This harness uses the same portable receipt/session policy as the native relay.
struct MemoryRetryFlow {
    engine: quick_panel::engine::Engine,
    worker: HarnessWorker,
    session: quick_panel::organizer_ui::OrganizerSession,
    memory: quick_panel::organizer_ui::MemoryUiState,
    works: Vec<quick_panel::organizer::Work>,
    receipts: Vec<(bool, bool)>,
    checkpoints: std::collections::HashMap<u64, quick_panel::pi_harness::Checkpoint>,
    completed: Vec<u64>,
    events: Vec<quick_panel::model::Event>,
}
impl MemoryRetryFlow {
    fn pump(&mut self) {
        use quick_panel::model::{Command, Event};
        while let Some(Output::Actor(command)) = self.worker.take() {
            if let Command::HarnessCheckpoint { token, checkpoint } = &command {
                self.checkpoints.insert(*token, checkpoint.clone());
            }
            let terminal = match &command {
                Command::OrganizerComplete { token, .. } | Command::HarnessFailed { token, .. } => {
                    Some(*token)
                }
                _ => None,
            };
            if terminal.is_none_or(|token| self.session.finish(token).is_some()) {
                if let Command::OrganizerComplete { token, .. } = &command {
                    self.completed.push(*token);
                }
                self.engine.send(command).unwrap();
            }
        }
        while let Ok(event) = self.engine.try_recv() {
            match event {
                Event::OrganizerWork(work) => {
                    self.worker.start(work.clone()).unwrap();
                    self.session.begin(work.token, work.token);
                    self.works.push(work);
                }
                Event::HarnessMemoryReceipt {
                    request_id,
                    result,
                    pending_save,
                } => {
                    self.receipts.push((result.is_ok(), pending_save));
                    if self.memory.receipt_for_session(
                        request_id,
                        result,
                        pending_save,
                        &mut self.session,
                    ) == Some(true)
                    {
                        self.worker.cancel();
                    }
                }
                event @ (Event::HarnessContext { .. }
                | Event::HarnessToolResult { .. }
                | Event::HarnessFinished { .. }) => {
                    if !self.worker.reply(event.clone()) {
                        self.events.push(event);
                    }
                }
                event => self.events.push(event),
            }
        }
    }
    fn until(&mut self, mut predicate: impl FnMut(&Self) -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            self.pump();
            if predicate(self) {
                return;
            }
            assert!(
                Instant::now() < end,
                "memory retry bridge did not progress; works={:?} receipts={:?} checkpoints={:?} completed={:?}",
                self.works.iter().map(|w| w.token).collect::<Vec<_>>(),
                self.receipts,
                self.checkpoints.keys(),
                self.completed
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn send_memory(&mut self, command: quick_panel::model::Command) {
        self.memory.bind_work(&self.session);
        self.engine.send(command).unwrap();
    }
}
#[test]
fn memory_save_retry_after_old_worker_exit_keeps_new_actor_run_and_budget() {
    use quick_panel::{engine::Engine, harness_tools::*, model::*, pi_harness::*};
    use serde_json::json;
    // Cover feedback/edit/delete, and both successful and still-failing persistence retries.
    for operation in ["feedback", "edit", "delete"] {
        for retry_success in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("memory-retry.db");
            let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
            let (entered, enter) = std::sync::mpsc::channel();
            let (release, gate) = std::sync::mpsc::channel();
            let gate = std::sync::Mutex::new(gate);
            let worker = HarnessWorker::new(
                paths(),
                move |work, _| {
                    entered.send(work.token).unwrap();
                    gate.lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(6))
                        .unwrap();
                    Ok(ModelResponse {
                        content: vec![
                            json!({"type":"tool_use","id":"submit","name":"organizer_submit","input":{"index":0,"expected_version":work.candidates[0].expected_version,"alias":"新名称","tags":[],"reason":"合成"}}),
                        ],
                        stop_reason: "tool_use".into(),
                    })
                },
                Arc::new(|| {}),
            );
            let mut flow = MemoryRetryFlow {
                engine,
                worker,
                session: Default::default(),
                memory: Default::default(),
                works: vec![],
                receipts: vec![],
                checkpoints: Default::default(),
                completed: vec![],
                events: vec![],
            };
            let mut settings = Settings::default();
            settings.organizer.enabled = true;
            settings.organizer.batch_size = 1;
            settings.ai.model = "fixture".into();
            settings.ai.encrypted_key = b"synthetic".to_vec();
            flow.engine.send(Command::SaveSettings(settings)).unwrap();
            flow.engine
                .send(Command::HarnessMemoryEdit {
                    request_id: 1,
                    input: MemoryEdit {
                        id: None,
                        scope: MemoryScope::Global,
                        text: "初始规则".into(),
                        enabled: true,
                        confirmed: true,
                    },
                })
                .unwrap();
            flow.until(|s| !s.receipts.is_empty());
            flow.receipts.clear();
            let favorite = |id: &str| {
                Command::SaveFavorite(FavoriteInput {
                    id: Some(id.into()),
                    kind: ItemKind::Folder,
                    title: id.into(),
                    target: format!("C:/Synthetic/{id}"),
                    alias: String::new(),
                    tags: vec![],
                    open_with: None,
                })
            };
            flow.engine.send(favorite("first")).unwrap();
            flow.engine.send(Command::Flush).unwrap();
            flow.until(|s| s.events.iter().any(|e| matches!(e, Event::Saved(_))));
            flow.engine.send(Command::OrganizerTick).unwrap();
            flow.until(|_| enter.try_recv().is_ok());
            let old = flow.works[0].clone();
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("CREATE TRIGGER fail_retry BEFORE UPDATE ON harness_state BEGIN SELECT RAISE(FAIL,'synthetic denied'); END;").unwrap();
            let command = match operation {
                "feedback" => Command::HarnessFeedback {
                    request_id: 77,
                    sources: vec![FeedbackTarget {
                        source: old.candidates[0].source.clone(),
                        expected_version: old.candidates[0].expected_version.clone(),
                        rejected_alias: None,
                    }],
                    reason: "保留项目名".into(),
                    scope: FeedbackScope::Item,
                },
                "edit" => Command::HarnessMemoryEdit {
                    request_id: 77,
                    input: MemoryEdit {
                        id: Some(1),
                        scope: MemoryScope::Global,
                        text: "修改规则".into(),
                        enabled: true,
                        confirmed: true,
                    },
                },
                _ => Command::HarnessMemoryDelete {
                    request_id: 77,
                    ids: vec![1],
                },
            };
            let command = flow.memory.submit(command).unwrap();
            flow.send_memory(command);
            flow.until(|s| s.receipts.len() == 1);
            assert_eq!(flow.receipts[0], (false, true));
            assert!(flow.worker.busy(), "old HTTP still owns the slot");
            release.send(()).unwrap();
            flow.until(|s| !s.worker.busy());
            assert!(flow.session.finish(old.token).is_none());
            flow.engine.send(Command::OrganizerTick).unwrap();
            flow.until(|_| enter.try_recv().is_ok());
            let new = flow.works.last().unwrap().token;
            assert_ne!(new, old.token);
            // Initial settled checkpoint is received while the second model request is blocked.
            flow.until(|s| s.checkpoints.get(&new).is_some_and(|cp| cp.rounds == 1));
            let before = flow.checkpoints[&new].clone();
            if retry_success {
                db.execute_batch("DROP TRIGGER fail_retry").unwrap();
            }
            let retry = flow.memory.retry().unwrap();
            flow.send_memory(retry);
            flow.until(|s| s.receipts.len() == 2);
            assert_eq!(flow.receipts[1], (retry_success, !retry_success));
            assert_eq!(flow.checkpoints[&new], before);
            flow.engine
                .send(Command::HarnessContext {
                    token: new,
                    request_id: 997,
                })
                .unwrap();
            flow.until(|s| {
                s.events.iter().any(|e| {
                    matches!(
                        e,
                        Event::HarnessContext {
                            request_id: 997,
                            ..
                        }
                    )
                })
            });
            let context = flow
                .events
                .iter()
                .find_map(|e| match e {
                    Event::HarnessContext {
                        request_id: 997,
                        result,
                        ..
                    } => Some(result.as_ref().unwrap()),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                context.checkpoint.as_ref(),
                Some(&before),
                "save retry must preserve the actor's active run budget"
            );
            release.send(()).unwrap();
            flow.until(|s| !s.worker.busy());
            flow.until(|s| s.completed.contains(&new));
            flow.engine
                .send(Command::HarnessMemorySnapshot { request_id: 88 })
                .unwrap();
            flow.engine
                .send(Command::OrganizerSnapshot { request_id: 89 })
                .unwrap();
            flow.until(|s| {
                s.events
                    .iter()
                    .any(|e| matches!(e, Event::OrganizerState { request_id: 89, .. }))
            });
            let memory = flow
                .events
                .iter()
                .find_map(|e| match e {
                    Event::HarnessMemoryState {
                        request_id: 88,
                        snapshot,
                    } => Some(snapshot),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                memory.records.len(),
                if operation == "feedback" {
                    2
                } else if operation == "edit" {
                    1
                } else {
                    0
                }
            );
            let snapshot = flow
                .events
                .iter()
                .find_map(|e| match e {
                    Event::OrganizerState {
                        request_id: 89,
                        snapshot,
                    } => Some(snapshot),
                    _ => None,
                })
                .unwrap();
            assert!(!snapshot.in_flight);
            assert_eq!(snapshot.proposals.len(), 1);
            assert_eq!(flow.checkpoints[&new].rounds, 1);
            assert_eq!(flow.checkpoints[&new].calls, 1);
            if !retry_success {
                db.execute_batch("DROP TRIGGER fail_retry").unwrap();
            }
            // A later batch must still dispatch and complete. Explicit retry opens the timer gate.
            flow.engine.send(favorite("second")).unwrap();
            flow.engine.send(Command::OrganizerRetry).unwrap();
            flow.engine.send(Command::OrganizerTick).unwrap();
            flow.until(|_| enter.try_recv().is_ok());
            let next = flow.works.last().unwrap().token;
            release.send(()).unwrap();
            flow.until(|s| s.completed.contains(&next));
            assert_ne!(next, new);
            flow.engine.send(Command::Shutdown).unwrap();
        }
    }
}
