use quick_panel::{engine::Engine, model::*, organizer::*};
use rusqlite::Connection;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn older_settings_gain_memory_and_round_defaults_without_enabling_parent() {
    let old = serde_json::json!({"enabled":false,"batch_size":4});
    let s: OrganizerSettings = serde_json::from_value(old).unwrap();
    let json = serde_json::to_value(s).unwrap();
    assert_eq!(json["memory_enabled"], true);
    assert_eq!(json["max_rounds"], 4);
    assert_eq!(json["enabled"], false);
    let invalid: OrganizerSettings =
        serde_json::from_value(serde_json::json!({"max_rounds":9})).unwrap();
    assert!(invalid.validate().is_err());
}
fn wait(e: &Engine, f: impl Fn(&Event) -> bool) -> Event {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match e.try_recv() {
            Ok(v) if f(&v) => return v,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            Ok(_) => (),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < end, "timeout");
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(x) => panic!("{x}"),
        }
    }
}
fn open(p: &std::path::Path) -> Engine {
    let e = Engine::start(EngineOptions::new(p.into()), Arc::new(|| {})).unwrap();
    wait(&e, |v| matches!(v, Event::Ready(_)));
    e
}
fn config() -> Settings {
    let mut s = Settings::default();
    s.organizer.enabled = true;
    s.organizer.scope = OrganizerScope::AllTextAndPaths;
    s.ai.model = "fixture".into();
    s.ai.encrypted_key = b"fixture".to_vec();
    s
}
fn settings(e: &Engine, s: Settings) {
    e.send(Command::SaveSettings(s)).unwrap();
    wait(e, |v| matches!(v, Event::Saved(_)));
}
fn favorite(e: &Engine, id: &str) {
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some(id.into()),
        kind: ItemKind::Folder,
        title: "项目日志".into(),
        target: format!("C:/Project/{id}"),
        alias: String::new(),
        tags: vec![],
        open_with: None,
    }))
    .unwrap();
    wait(e, |v| matches!(v, Event::Saved(_)));
}
fn work(e: &Engine) -> Work {
    e.send(Command::OrganizerTick).unwrap();
    match wait(e, |v| matches!(v, Event::OrganizerWork(_))) {
        Event::OrganizerWork(w) => w,
        _ => unreachable!(),
    }
}
fn snapshot(e: &Engine) -> Snapshot {
    e.send(Command::OrganizerSnapshot { request_id: 17 })
        .unwrap();
    match wait(e, |v| {
        matches!(v, Event::OrganizerState { request_id: 17, .. })
    }) {
        Event::OrganizerState { snapshot, .. } => snapshot,
        _ => unreachable!(),
    }
}
fn stop(e: &Engine) {
    e.send(Command::Shutdown).unwrap();
    wait(e, |v| matches!(v, Event::ExitReady));
}
fn data_version(db: &Connection) -> i64 {
    db.query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap()
}
fn context(e: &Engine, w: &Work) -> quick_panel::harness_tools::PiContext {
    e.send(Command::HarnessContext {
        token: w.token,
        request_id: 22,
    })
    .unwrap();
    match wait(e, |v| {
        matches!(v, Event::HarnessContext { request_id: 22, .. })
    }) {
        Event::HarnessContext { result, .. } => result.unwrap(),
        _ => unreachable!(),
    }
}
fn tool(
    e: &Engine,
    w: &Work,
    call: &str,
    name: quick_panel::pi_harness::ToolName,
    args: serde_json::Value,
) -> Result<serde_json::Value, quick_panel::pi_harness::ToolError> {
    e.send(Command::HarnessTool {
        token: w.token,
        call_id: call.into(),
        name,
        arguments: args,
    })
    .unwrap();
    match wait(
        e,
        |v| matches!(v,Event::HarnessToolResult{call_id,..} if call_id==call),
    ) {
        Event::HarnessToolResult { result, .. } => result.into_result(),
        _ => unreachable!(),
    }
}
fn submit(
    e: &Engine,
    w: &Work,
    call: &str,
    alias: &str,
) -> Result<serde_json::Value, quick_panel::pi_harness::ToolError> {
    tool(
        e,
        w,
        call,
        quick_panel::pi_harness::ToolName::OrganizerSubmit,
        serde_json::json!({"index":0,"expected_version":w.candidates[0].expected_version,"alias":alias,"tags":["项目"],"reason":"合成依据"}),
    )
}
fn finish(e: &Engine, w: &Work) -> Result<Vec<Proposal>, String> {
    e.send(Command::HarnessFinish {
        token: w.token,
        request_id: 23,
    })
    .unwrap();
    match wait(e, |v| {
        matches!(v, Event::HarnessFinished { request_id: 23, .. })
    }) {
        Event::HarnessFinished { result, .. } => result,
        _ => unreachable!(),
    }
}
#[test]
fn repairable_submit_is_staged_and_finish_complete_apply_only_once_without_read_writes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    let db = Connection::open(&p).unwrap();
    let before = data_version(&db);
    let ctx = context(&e, &w);
    assert_eq!(ctx.item_refs[0].index, 0);
    assert_eq!(ctx.item_refs[0].version, w.candidates[0].expected_version);
    let err = submit(&e, &w, "c1", &"中".repeat(49)).unwrap_err();
    assert_eq!(err.code, "INVALID_ALIAS");
    assert!(err.retryable);
    assert_eq!(snapshot(&e).failures, 0);
    assert!(finish(&e, &w).is_err());
    assert_eq!(
        submit(&e, &w, "c2", "项目目录").unwrap(),
        serde_json::json!({"accepted":true,"index":0})
    );
    assert_eq!(
        submit(&e, &w, "c2", "项目目录").unwrap(),
        serde_json::json!({"accepted":true,"index":0})
    );
    let proposals = finish(&e, &w).unwrap();
    assert_eq!(finish(&e, &w).unwrap(), proposals);
    let alias: String = db
        .query_row(
            "SELECT json_extract(summary,'$.alias') FROM items WHERE id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(alias, "");
    assert_eq!(snapshot(&e).proposals.len(), 0);
    assert_eq!(before, data_version(&db));
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(proposals.clone()),
    })
    .unwrap();
    assert_eq!(snapshot(&e).proposals.len(), 1);
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(proposals),
    })
    .unwrap();
    assert_eq!(snapshot(&e).proposals.len(), 1);
    assert_eq!(before, data_version(&db));
    stop(&e);
}
#[test]
fn all_read_tools_are_bounded_scope_projections_and_usage_does_not_stale_source() {
    use quick_panel::pi_harness::ToolName::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("read-db");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    favorite(&e, "b");
    let w = work(&e);
    let db = Connection::open(&p).unwrap();
    let v = data_version(&db);
    let value = tool(
        &e,
        &w,
        "read1",
        ItemsGetContext,
        serde_json::json!({"index":0}),
    )
    .unwrap();
    assert_eq!(value["version"], w.candidates[0].expected_version);
    assert!(value.get("source").is_none());
    assert!(value.get("id").is_none());
    assert_eq!(value["context"], "C:/Project/a");
    let found = tool(
        &e,
        &w,
        "read2",
        ItemsSearch,
        serde_json::json!({"query":"项目","limit":1}),
    )
    .unwrap();
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert!(
        tool(
            &e,
            &w,
            "read3",
            ItemsSearch,
            serde_json::json!({"query":"","limit":9})
        )
        .unwrap_err()
        .retryable
    );
    let usage = tool(
        &e,
        &w,
        "read4",
        UsageGetSummary,
        serde_json::json!({"index":0}),
    )
    .unwrap();
    assert_eq!(usage["use_count"], 0);
    let memory = tool(
        &e,
        &w,
        "read5",
        MemoryRecall,
        serde_json::json!({"index":0}),
    )
    .unwrap();
    assert_eq!(memory["records"], serde_json::json!([]));
    e.send(Command::Used("a".into())).unwrap();
    let _ = snapshot(&e);
    assert!(submit(&e, &w, "submit", "可用").is_ok());
    assert_eq!(v, data_version(&db));
    stop(&e);
}
#[test]
fn local_failure_skips_attention_and_only_typed_transport_spends_backoff() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("attention"));
    let mut s = config();
    s.organizer.batch_size = 1;
    settings(&e, s);
    favorite(&e, "a");
    favorite(&e, "b");
    let w = work(&e);
    e.send(Command::HarnessFailed {
        token: w.token,
        code: "deadline".into(),
        message: "活动时间耗尽".into(),
        retryable: false,
    })
    .unwrap();
    let snap = snapshot(&e);
    assert_eq!(snap.failures, 0);
    assert!(!snap.in_flight);
    assert_eq!(snap.attention.len(), 1);
    let next = work(&e);
    assert_ne!(next.candidates[0].source, w.candidates[0].source);
    e.send(Command::HarnessFailed {
        token: next.token,
        code: "TRANSIENT_NETWORK".into(),
        message: "合成网络错误".into(),
        retryable: true,
    })
    .unwrap();
    let snap = snapshot(&e);
    assert_eq!(snap.failures, 1);
    assert!(snap.retry_at_ms.is_some());
    e.send(Command::OrganizerRetry).unwrap();
    let snap = snapshot(&e);
    assert_eq!(snap.failures, 0);
    assert!(snap.attention.is_empty());
    stop(&e);
}
#[test]
fn disabled_and_source_edit_revoke_old_run_and_index_and_unknown_fields_are_rejected() {
    use quick_panel::pi_harness::ToolName::*;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("guards"));
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    let err = tool(
        &e,
        &w,
        "bad-index",
        ItemsGetContext,
        serde_json::json!({"index":7}),
    )
    .unwrap_err();
    assert!(err.retryable);
    assert!(
        tool(
            &e,
            &w,
            "bad-extra",
            ItemsGetContext,
            serde_json::json!({"index":0,"source":"b"})
        )
        .unwrap_err()
        .retryable
    );
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some("a".into()),
        kind: ItemKind::Folder,
        title: "人工改名".into(),
        target: "C:/Project/a".into(),
        alias: String::new(),
        tags: vec![],
        open_with: None,
    }))
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    assert!(!submit(&e, &w, "stale", "建议").unwrap_err().retryable);
    let mut s = config();
    s.organizer.enabled = false;
    settings(&e, s);
    assert_eq!(submit(&e, &w, "off", "建议").unwrap_err().code, "STALE_RUN");
    stop(&e);
}
fn memory(e: &Engine) -> quick_panel::harness_tools::MemorySnapshot {
    e.send(Command::HarnessMemorySnapshot { request_id: 70 })
        .unwrap();
    match wait(e, |v| {
        matches!(v, Event::HarnessMemoryState { request_id: 70, .. })
    }) {
        Event::HarnessMemoryState { snapshot, .. } => snapshot,
        _ => unreachable!(),
    }
}
fn receipt(e: &Engine, id: u64) -> (Result<(), String>, bool) {
    match wait(
        e,
        |v| matches!(v,Event::HarnessMemoryReceipt{request_id,..} if *request_id==id),
    ) {
        Event::HarnessMemoryReceipt {
            result,
            pending_save,
            ..
        } => (result, pending_save),
        _ => unreachable!(),
    }
}
#[test]
fn feedback_persists_reason_scope_negative_alias_and_duplicate_request_is_idempotent() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("feedback");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "已拒别名").unwrap();
    let ps = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(ps),
    })
    .unwrap();
    assert_eq!(snapshot(&e).proposals.len(), 1);
    let cmd = Command::HarnessFeedback {
        request_id: 80,
        sources: vec![seen_target(&snapshot(&e).proposals[0])],
        reason: "要保留项目标识".into(),
        scope: FeedbackScope::Item,
    };
    e.send(cmd.clone()).unwrap();
    assert!(receipt(&e, 80).0.is_ok());
    let m = memory(&e);
    assert_eq!(m.records.len(), 1);
    assert!(m.organizer_enabled && m.memory_enabled);
    assert_eq!(m.records[0].text, "要保留项目标识");
    assert_eq!(m.records[0].alias.as_deref(), Some("已拒别名"));
    assert_eq!(
        m.records[0].scope,
        MemoryScope::Item {
            source: w.candidates[0].source.clone()
        }
    );
    e.send(cmd).unwrap();
    assert!(receipt(&e, 80).0.is_ok());
    assert_eq!(memory(&e).records.len(), 1);
    let redo = work(&e);
    assert!(context(&e, &redo).checkpoint.is_none());
    let err = submit(&e, &redo, "negative", "已拒别名").unwrap_err();
    assert_eq!(err.code, "REJECTED_ALIAS");
    assert!(err.retryable);
    let recalled = tool(
        &e,
        &redo,
        "recall",
        quick_panel::pi_harness::ToolName::MemoryRecall,
        serde_json::json!({"index":0}),
    )
    .unwrap();
    assert_eq!(recalled["records"][0]["text"], "要保留项目标识");
    let mut disabled = config();
    disabled.organizer.memory_enabled = false;
    settings(&e, disabled.clone());
    let records = memory(&e).records;
    let db = Connection::open(&p).unwrap();
    let before = data_version(&db);
    for _ in 0..4 {
        let projected = memory(&e);
        assert!(projected.organizer_enabled && !projected.memory_enabled);
        assert_eq!(projected.records, records);
        assert!(!projected.record_effective(&projected.records[0]));
        assert!(projected.availability().contains("整理记忆已关闭"));
    }
    assert_eq!(
        data_version(&db),
        before,
        "read-only projection must not commit"
    );
    let redo = work(&e);
    assert!(submit(&e, &redo, "negative-disabled", "已拒别名").is_ok());
    assert_eq!(memory(&e).records, records);
    disabled.organizer.enabled = false;
    settings(&e, disabled);
    let projected = memory(&e);
    assert!(!projected.organizer_enabled && !projected.memory_enabled);
    assert_eq!(projected.records, records);
    assert!(projected.availability().contains("AI 管家已关闭"));
    stop(&e);
    let e = open(&p);
    assert_eq!(memory(&e).records.len(), 1);
    stop(&e);
}
#[test]
fn ai_rule_requires_explicit_human_confirmation_and_scope_edits_are_durable() {
    use quick_panel::harness_tools::*;
    use quick_panel::pi_harness::ToolName::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("rules");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    tool(
        &e,
        &w,
        "draft",
        MemoryProposeRule,
        serde_json::json!({"index":0,"text":"目录需要项目名称","evidence_ids":[]}),
    )
    .unwrap();
    let m = memory(&e);
    assert_eq!(m.records.len(), 1);
    assert!(!m.records[0].confirmed);
    assert_eq!(m.records[0].origin, MemoryOrigin::Ai);
    assert_eq!(
        tool(
            &e,
            &w,
            "before",
            MemoryRecall,
            serde_json::json!({"index":0})
        )
        .unwrap()["records"],
        serde_json::json!([])
    );
    e.send(Command::HarnessMemoryEdit {
        request_id: 81,
        input: MemoryEdit {
            id: Some(m.records[0].id),
            scope: MemoryScope::Directory {
                path: "c:\\project".into(),
            },
            text: "保留项目名称".into(),
            enabled: true,
            confirmed: true,
        },
    })
    .unwrap();
    assert!(receipt(&e, 81).0.is_ok());
    assert_eq!(memory(&e).version, 1);
    assert_eq!(
        serde_json::to_value(&memory(&e).records[0]).unwrap()["proposed_by_ai"],
        true
    );
    assert_eq!(
        submit(&e, &w, "stale", "草稿").unwrap_err().code,
        "STALE_RUN"
    );
    let next = work(&e);
    assert_eq!(
        tool(
            &e,
            &next,
            "after",
            MemoryRecall,
            serde_json::json!({"index":0})
        )
        .unwrap()["records"][0]["text"],
        "保留项目名称"
    );
    e.send(Command::HarnessMemoryDelete {
        request_id: 82,
        ids: vec![m.records[0].id],
    })
    .unwrap();
    assert!(receipt(&e, 82).0.is_ok());
    stop(&e);
    let e = open(&p);
    assert!(memory(&e).records.is_empty());
    stop(&e);
}
fn checkpoint(ctx: &quick_panel::harness_tools::PiContext) -> quick_panel::pi_harness::Checkpoint {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../agent-runtime/fixtures/protocol-v1.json")).unwrap();
    let mut cp: quick_panel::pi_harness::Checkpoint =
        serde_json::from_value(fixture["runtime"][1]["checkpoint"].clone()).unwrap();
    let mut messages: serde_json::Value = serde_json::from_str(&cp.messages_json).unwrap();
    let prefix = messages[0]["content"]
        .as_str()
        .unwrap()
        .split("\nInstructions:")
        .next()
        .unwrap();
    messages[0]["content"] = serde_json::json!(format!(
        "{prefix}\nInstructions: {}\nItem references: {}",
        ctx.instructions,
        serde_json::to_string(&ctx.item_refs).unwrap()
    ));
    cp.messages_json = serde_json::to_string(&messages).unwrap();
    cp.rounds = 2;
    cp.calls = 3;
    cp.elapsed_ms = 119900;
    cp.validate().unwrap();
    cp
}
#[test]
fn checkpoint_and_staged_results_survive_automatic_restart_with_all_budgets() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("resume");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    let cp = checkpoint(&context(&e, &w));
    submit(&e, &w, "s", "暂存名称").unwrap();
    e.send(Command::HarnessCheckpoint {
        token: w.token,
        checkpoint: cp.clone(),
    })
    .unwrap();
    assert_eq!(context(&e, &w).checkpoint, Some(cp.clone()));
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute(
        "UPDATE organizer_state SET data=json_set(data,'$.next_at_ms',0)",
        [],
    )
    .unwrap();
    drop(db);
    let e = open(&p);
    let resumed = work(&e);
    assert_eq!(context(&e, &resumed).checkpoint, Some(cp));
    assert_eq!(
        finish(&e, &resumed).unwrap()[0].alias.as_deref(),
        Some("暂存名称")
    );
    stop(&e);
}
#[test]
fn optional_tables_are_lazy_and_corrupt_or_oversized_harness_never_blocks_core() {
    for payload in ["{bad".to_owned(), "x".repeat(1024 * 1024 + 1)] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("optional");
        let e = open(&p);
        favorite(&e, "core");
        stop(&e);
        let db = Connection::open(&p).unwrap();
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='harness_state')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!exists);
        db.execute_batch("CREATE TABLE harness_state(id INTEGER PRIMARY KEY,data TEXT)")
            .unwrap();
        db.execute("INSERT INTO harness_state VALUES(1,?1)", [&payload])
            .unwrap();
        drop(db);
        let e = open(&p);
        assert!(memory(&e).storage_error.is_some());
        settings(&e, config());
        e.send(Command::OrganizerTick).unwrap();
        assert!(!snapshot(&e).in_flight);
        stop(&e);
        let db = Connection::open(&p).unwrap();
        let saved: String = db
            .query_row("SELECT data FROM harness_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(saved, payload);
    }
}
#[test]
fn human_acceptance_uses_final_edited_name_and_auto_and_defer_are_not_human_rules() {
    use quick_panel::harness_tools::*;
    use quick_panel::organizer_ui::ReviewDecision;
    for mode in [OrganizerMode::Suggest, OrganizerMode::Automatic] {
        let d = tempfile::tempdir().unwrap();
        let e = open(&d.path().join("origin"));
        let mut s = config();
        s.organizer.mode = mode;
        settings(&e, s);
        favorite(&e, "a");
        let w = work(&e);
        submit(&e, &w, "s", "模型名称").unwrap();
        let ps = finish(&e, &w).unwrap();
        e.send(Command::OrganizerComplete {
            token: w.token,
            result: Ok(ps),
        })
        .unwrap();
        let snap = snapshot(&e);
        if mode == OrganizerMode::Suggest {
            let mut p = snap.proposals[0].clone();
            p.alias = Some("人工最终名称".into());
            e.send(Command::OrganizerReview {
                request_id: 91,
                decision: ReviewDecision::Apply(vec![p]),
            })
            .unwrap();
            wait(&e, |v| {
                matches!(v, Event::OrganizerReceipt { request_id: 91, .. })
            });
            let m = memory(&e);
            assert_eq!(m.records[0].alias.as_deref(), Some("人工最终名称"));
            assert_eq!(m.records[0].kind, MemoryKind::Edited);
            assert_eq!(m.records[0].origin, MemoryOrigin::Human);
            assert!(m.records[0].confirmed);
        } else {
            let m = memory(&e);
            assert_eq!(m.records[0].kind, MemoryKind::AiApplied);
            assert_eq!(m.records[0].origin, MemoryOrigin::Ai);
            assert!(!m.records[0].confirmed);
        }
        stop(&e);
    }
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("defer"));
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "建议").unwrap();
    let ps = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(ps),
    })
    .unwrap();
    let _ = snapshot(&e);
    e.send(Command::OrganizerDismiss {
        sources: vec![w.candidates[0].source.clone()],
    })
    .unwrap();
    let _ = snapshot(&e);
    assert!(memory(&e).records.is_empty());
    stop(&e);
}
#[test]
fn confirmed_capacity_never_evicts_human_rules_and_save_failure_retains_pending_edit() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("capacity");
    let e = open(&p);
    for i in 0..64 {
        e.send(Command::HarnessMemoryEdit {
            request_id: 100 + i,
            input: MemoryEdit {
                id: None,
                scope: MemoryScope::Global,
                text: format!("规则{i}"),
                enabled: true,
                confirmed: true,
            },
        })
        .unwrap();
        assert!(receipt(&e, 100 + i).0.is_ok());
    }
    e.send(Command::HarnessMemoryEdit {
        request_id: 200,
        input: MemoryEdit {
            id: None,
            scope: MemoryScope::Global,
            text: "第65条规则".into(),
            enabled: true,
            confirmed: true,
        },
    })
    .unwrap();
    assert!(receipt(&e, 200).0.is_err());
    assert_eq!(memory(&e).records.len(), 64);
    let db = Connection::open(&p).unwrap();
    db.execute_batch("CREATE TRIGGER fail_harness BEFORE UPDATE ON harness_state BEGIN SELECT RAISE(FAIL,'synthetic denied'); END;").unwrap();
    e.send(Command::HarnessMemoryEdit {
        request_id: 201,
        input: MemoryEdit {
            id: Some(1),
            scope: MemoryScope::Global,
            text: "失败但保留".into(),
            enabled: true,
            confirmed: true,
        },
    })
    .unwrap();
    let (result, pending) = receipt_after_failed_write(&e, 201);
    assert!(result.is_err() && pending);
    assert_eq!(memory(&e).records[0].text, "失败但保留");
    db.execute_batch("DROP TRIGGER fail_harness").unwrap();
    e.send(Command::HarnessMemoryEdit {
        request_id: 201,
        input: MemoryEdit {
            id: Some(1),
            scope: MemoryScope::Global,
            text: "失败但保留".into(),
            enabled: true,
            confirmed: true,
        },
    })
    .unwrap();
    assert!(receipt(&e, 201).0.is_ok());
    assert_eq!(memory(&e).records.len(), 64);
    stop(&e);
}
#[test]
fn automatic_restart_keeps_original_reduced_batch_before_newer_queue_and_profile_changes_clear_checkpoint()
 {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("batch-binding");
    let e = open(&p);
    let mut s = config();
    s.organizer.batch_size = 2;
    settings(&e, s.clone());
    favorite(&e, "a");
    let w = work(&e);
    assert_eq!(w.candidates.len(), 1);
    let cp = checkpoint(&context(&e, &w));
    e.send(Command::HarnessCheckpoint {
        token: w.token,
        checkpoint: cp.clone(),
    })
    .unwrap();
    let _ = context(&e, &w);
    favorite(&e, "b");
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute(
        "UPDATE organizer_state SET data=json_set(data,'$.next_at_ms',0)",
        [],
    )
    .unwrap();
    drop(db);
    let e = open(&p);
    let resumed = work(&e);
    assert_eq!(resumed.candidates, w.candidates);
    assert_eq!(context(&e, &resumed).checkpoint, Some(cp));
    s.ai.effort = quick_panel::ai::Effort::High;
    settings(&e, s);
    let changed = work(&e);
    assert!(context(&e, &changed).checkpoint.is_none());
    stop(&e);
}
#[test]
fn checkpoint_failure_refresh_is_latest_monotonic_and_budget_corruption_preserves_history() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("latest");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    let mut cp = checkpoint(&context(&e, &w));
    cp.elapsed_ms = 119950;
    e.send(Command::HarnessCheckpoint {
        token: w.token,
        checkpoint: cp.clone(),
    })
    .unwrap();
    e.send(Command::HarnessFailed {
        token: w.token,
        code: "TRANSIENT_NETWORK".into(),
        message: "等待失败".into(),
        retryable: true,
    })
    .unwrap();
    let snap = snapshot(&e);
    assert_eq!(snap.failures, 1);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute(
        "UPDATE organizer_state SET data=json_set(data,'$.next_at_ms',0,'$.retry_at_ms',0)",
        [],
    )
    .unwrap();
    drop(db);
    let e = open(&p);
    let w = work(&e);
    assert_eq!(context(&e, &w).checkpoint, Some(cp.clone()));
    let mut backwards = cp;
    backwards.elapsed_ms = 10;
    e.send(Command::HarnessCheckpoint {
        token: w.token,
        checkpoint: backwards,
    })
    .unwrap();
    let snap = snapshot(&e);
    assert_eq!(snap.attention[0].code, "protocol");
    assert_eq!(snap.failures, 1);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute(
        "UPDATE harness_state SET data=json_set(data,'$.run.checkpoint.elapsed_ms',120001)",
        [],
    )
    .unwrap();
    let preserved: String = db
        .query_row("SELECT data FROM harness_state", [], |r| r.get(0))
        .unwrap();
    drop(db);
    let e = open(&p);
    assert!(memory(&e).storage_error.is_some());
    e.send(Command::Search(SearchQuery {
        text: "项目".into(),
        scope: Scope::All,
        request_id: 313,
    }))
    .unwrap();
    match wait(&e, |v| {
        matches!(
            v,
            Event::Results {
                request_id: 313,
                ..
            }
        )
    }) {
        Event::Results { items, .. } => assert_eq!(items.len(), 1),
        _ => unreachable!(),
    };
    stop(&e);
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row("SELECT data FROM harness_state", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        preserved
    );
}
#[test]
fn automatic_save_deadline_commits_ai_drafts_and_checkpoints_but_read_refreshes_do_not_postpone_it()
{
    use quick_panel::pi_harness::ToolName::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("deadline");
    let mut options = EngineOptions::new(p.clone());
    options.save_interval_ms = 180;
    let e = Engine::start(options, Arc::new(|| {})).unwrap();
    wait(&e, |v| matches!(v, Event::Ready(_)));
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    let cp = checkpoint(&context(&e, &w));
    e.send(Command::HarnessCheckpoint {
        token: w.token,
        checkpoint: cp,
    })
    .unwrap();
    tool(
        &e,
        &w,
        "draft",
        MemoryProposeRule,
        serde_json::json!({"index":0,"text":"合成待确认规则","evidence_ids":[]}),
    )
    .unwrap();
    let db = Connection::open(&p).unwrap();
    let before = data_version(&db);
    for i in 0..4 {
        tool(
            &e,
            &w,
            &format!("read-{i}"),
            ItemsGetContext,
            serde_json::json!({"index":0}),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(15));
    }
    assert_eq!(before, data_version(&db));
    wait(&e, |v| matches!(v, Event::Saved(_)));
    assert_ne!(before, data_version(&db));
    let saved: String = db
        .query_row("SELECT data FROM harness_state", [], |r| r.get(0))
        .unwrap();
    let json: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(json["run"]["checkpoint"]["elapsed_ms"], 119900);
    assert_eq!(json["memories"].as_array().unwrap().len(), 1);
    stop(&e);
}
#[test]
fn scoped_memory_and_recall_limits_do_not_promote_item_feedback_or_ai_drafts() {
    use quick_panel::{harness_tools::*, pi_harness::ToolName::*};
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("scopes"));
    settings(&e, config());
    favorite(&e, "a");
    favorite(&e, "b");
    for i in 0..12 {
        e.send(Command::HarnessMemoryEdit {
            request_id: 400 + i,
            input: MemoryEdit {
                id: None,
                scope: if i == 0 {
                    MemoryScope::Item {
                        source: Source::SavedItem { id: "a".into() },
                    }
                } else if i == 1 {
                    MemoryScope::Directory {
                        path: "c:\\project".into(),
                    }
                } else {
                    MemoryScope::Global
                },
                text: format!("{i}{}", "规".repeat(500)),
                enabled: true,
                confirmed: true,
            },
        })
        .unwrap();
        assert!(receipt(&e, 400 + i).0.is_ok());
    }
    let w = work(&e);
    let recalled = tool(&e, &w, "r", MemoryRecall, serde_json::json!({"index":0})).unwrap();
    assert_eq!(recalled["records"].as_array().unwrap().len(), 8);
    assert!(
        recalled["records"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with('0')
    );
    assert_eq!(recalled["records"][1]["scope"], "directory");
    assert!(serde_json::to_string(&recalled).unwrap().chars().count() <= 8192);
    let other = tool(&e, &w, "r2", MemoryRecall, serde_json::json!({"index":1})).unwrap();
    assert_eq!(other["records"][0]["scope"], "directory");
    let mut s = config();
    s.organizer.memory_enabled = false;
    settings(&e, s);
    let w = work(&e);
    assert_eq!(
        tool(&e, &w, "off", MemoryRecall, serde_json::json!({"index":0})).unwrap()["records"],
        serde_json::json!([])
    );
    stop(&e);
}
#[test]
fn auth_and_unavailable_pause_globally_once_without_network_backoff_until_explicit_retry() {
    for code in ["AUTH", "RUNTIME_UNAVAILABLE"] {
        let d = tempfile::tempdir().unwrap();
        let e = open(&d.path().join("global-stop"));
        let mut s = config();
        s.organizer.batch_size = 1;
        settings(&e, s);
        favorite(&e, "a");
        favorite(&e, "b");
        let w = work(&e);
        e.send(Command::HarnessFailed {
            token: w.token,
            code: code.into(),
            message: "合成终态".into(),
            retryable: false,
        })
        .unwrap();
        let snap = snapshot(&e);
        assert!(snap.paused);
        assert_eq!(snap.failures, 0);
        assert!(snap.retry_at_ms.is_none());
        assert_eq!(snap.queued, 1);
        e.send(Command::OrganizerTick).unwrap();
        assert!(!snapshot(&e).in_flight);
        e.send(Command::OrganizerRetry).unwrap();
        assert!(!snapshot(&e).paused);
        let _ = work(&e);
        stop(&e);
    }
}
#[test]
fn duplicate_call_id_with_changed_arguments_and_bypassed_finish_cannot_admit_results() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("idempotence"));
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "same", "首个名称").unwrap();
    assert_eq!(
        submit(&e, &w, "same", "另一名称").unwrap_err().code,
        "protocol"
    );
    let p = Proposal {
        candidate: w.candidates[0].clone(),
        alias: Some("绕过暂存".into()),
        tags: vec![],
        reason: "不经完成".into(),
    };
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(vec![p]),
    })
    .unwrap();
    let snap = snapshot(&e);
    assert!(snap.proposals.is_empty());
    assert_eq!(snap.failures, 0);
    assert_eq!(snap.attention[0].code, "incomplete");
    stop(&e);
}
#[test]
fn editing_rejected_feedback_retains_negative_alias_and_feedback_capacity_preserves_authority() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("negative-edit"));
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "禁止名称").unwrap();
    let ps = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(ps),
    })
    .unwrap();
    let _ = snapshot(&e);
    e.send(Command::HarnessFeedback {
        request_id: 600,
        sources: vec![seen_target(&snapshot(&e).proposals[0])],
        reason: "保留目录标识".into(),
        scope: FeedbackScope::Directory,
    })
    .unwrap();
    assert!(receipt(&e, 600).0.is_ok());
    let m = memory(&e).records[0].clone();
    e.send(Command::HarnessMemoryEdit {
        request_id: 601,
        input: MemoryEdit {
            id: Some(m.id),
            scope: m.scope.clone(),
            text: "修订反馈原因".into(),
            enabled: true,
            confirmed: true,
        },
    })
    .unwrap();
    assert!(receipt(&e, 601).0.is_ok());
    let edited = memory(&e).records[0].clone();
    assert_eq!(edited.kind, MemoryKind::Rejected);
    assert_eq!(edited.alias.as_deref(), Some("禁止名称"));
    let redo = work(&e);
    assert_eq!(
        submit(&e, &redo, "rejected", "禁止名称").unwrap_err().code,
        "REJECTED_ALIAS"
    );
    stop(&e);
}
#[test]
fn large_unicode_read_results_return_bounded_prefix_and_checkpoint_cannot_exceed_configured_rounds()
{
    use quick_panel::{harness_tools::*, pi_harness::ToolName::*};
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("read-budget"));
    let mut s = config();
    s.organizer.batch_size = 8;
    s.organizer.max_rounds = 2;
    settings(&e, s);
    for i in 0..8 {
        e.send(Command::SaveFavorite(FavoriteInput {
            id: Some(format!("unicode-{i}")),
            kind: ItemKind::Folder,
            title: "😀".repeat(256),
            target: format!("C:/{}", "😀".repeat(300)),
            alias: "😀".repeat(256),
            tags: (0..32).map(|i| format!("{i}{}", "😀".repeat(60))).collect(),
            open_with: None,
        }))
        .unwrap();
        wait(&e, |v| matches!(v, Event::Saved(_)));
    }
    for i in 0..8 {
        e.send(Command::HarnessMemoryEdit {
            request_id: 700 + i,
            input: MemoryEdit {
                id: None,
                scope: MemoryScope::Global,
                text: "😀".repeat(512),
                enabled: true,
                confirmed: true,
            },
        })
        .unwrap();
        assert!(receipt(&e, 700 + i).0.is_ok());
    }
    let w = work(&e);
    let found = tool(
        &e,
        &w,
        "large-search",
        ItemsSearch,
        serde_json::json!({"query":"","limit":8}),
    )
    .unwrap();
    assert!(!found["items"].as_array().unwrap().is_empty());
    assert!(serde_json::to_vec(&found).unwrap().len() <= 16 * 1024);
    let recalled = tool(
        &e,
        &w,
        "large-memory",
        MemoryRecall,
        serde_json::json!({"index":0}),
    )
    .unwrap();
    assert!(!recalled["records"].as_array().unwrap().is_empty());
    assert!(serde_json::to_vec(&recalled).unwrap().len() <= 16 * 1024);
    let mut cp = checkpoint(&context(&e, &w));
    cp.rounds = 3;
    e.send(Command::HarnessCheckpoint {
        token: w.token,
        checkpoint: cp,
    })
    .unwrap();
    let snap = snapshot(&e);
    assert_eq!(snap.attention[0].code, "round_limit");
    assert_eq!(snap.failures, 0);
    stop(&e);
}
#[test]
fn oversized_actor_bridge_commands_are_rejected_before_queueing_and_corrupt_receipts_pause_only_pi()
{
    use quick_panel::pi_harness::ToolName::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("bridge-bound");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    let huge = Command::HarnessTool {
        token: w.token,
        call_id: "huge".into(),
        name: ItemsSearch,
        arguments: serde_json::json!({"query":"x".repeat(256*1024+1),"limit":1}),
    };
    assert!(e.send(huge).is_err());
    submit(&e, &w, "s", "暂存").unwrap();
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute("UPDATE harness_state SET data=json_set(data,'$.run.receipts[0][2].value',json('{\"alien\":true}'))",[]).unwrap();
    let saved: String = db
        .query_row("SELECT data FROM harness_state", [], |r| r.get(0))
        .unwrap();
    drop(db);
    let e = open(&p);
    assert!(memory(&e).storage_error.is_some());
    assert!(snapshot(&e).storage_error.is_some());
    stop(&e);
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row("SELECT data FROM harness_state", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        saved
    );
}
#[test]
fn ai_draft_cache_evicts_oldest_examples_and_never_evicts_confirmed_rule() {
    use quick_panel::{harness_tools::*, pi_harness::ToolName::*};
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("evict"));
    settings(&e, config());
    favorite(&e, "a");
    e.send(Command::HarnessMemoryEdit {
        request_id: 900,
        input: MemoryEdit {
            id: None,
            scope: MemoryScope::Global,
            text: "保护规则".into(),
            enabled: true,
            confirmed: true,
        },
    })
    .unwrap();
    assert!(receipt(&e, 900).0.is_ok());
    let mut w = work(&e);
    for i in 0..260 {
        if i != 0 && i % 30 == 0 {
            e.send(Command::OrganizerRetry).unwrap();
            let _ = snapshot(&e);
            w = work(&e);
        }
        tool(
            &e,
            &w,
            &format!("draft-{i}"),
            MemoryProposeRule,
            serde_json::json!({"index":0,"text":format!("草稿{i}"),"evidence_ids":[]}),
        )
        .unwrap();
    }
    let m = memory(&e);
    assert_eq!(m.records.len(), 256);
    assert_eq!(m.records[0].text, "保护规则");
    assert!(!m.records.iter().any(|r| r.text == "草稿0"));
    assert!(m.records.iter().any(|r| r.text == "草稿259"));
    stop(&e);
}
#[test]
fn automatic_memory_captures_preserved_long_alias_and_actual_merged_tags_in_same_commit() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("actual-final");
    let e = open(&p);
    let mut s = config();
    s.organizer.mode = OrganizerMode::Automatic;
    settings(&e, s);
    let old = "人".repeat(60);
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some("a".into()),
        kind: ItemKind::Folder,
        title: "项目".into(),
        target: "C:/Project/a".into(),
        alias: old.clone(),
        tags: vec!["原标签".into()],
        open_with: None,
    }))
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    let w = work(&e);
    submit(&e, &w, "s", "模型空位名称").unwrap();
    let ps = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(ps),
    })
    .unwrap();
    let _ = snapshot(&e);
    let m = memory(&e);
    assert_eq!(m.records[0].alias.as_deref(), Some(old.as_str()));
    assert_eq!(m.records[0].tags, ["原标签", "项目"]);
    e.send(Command::Flush).unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    stop(&e);
    let e = open(&p);
    assert_eq!(memory(&e).records[0].alias.as_deref(), Some(old.as_str()));
    stop(&e);
}
#[test]
fn directory_feedback_normalizes_drive_and_unc_share_roots_before_durable_receipt() {
    use quick_panel::harness_tools::*;
    for (target, expected) in [
        ("C:/a", "c:\\"),
        ("C:/", "c:\\"),
        ("\\\\server\\share\\a", "\\\\server\\share\\"),
        ("\\\\server\\share\\", "\\\\server\\share\\"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("root-directory");
        let e = open(&p);
        settings(&e, config());
        e.send(Command::SaveFavorite(FavoriteInput {
            id: Some("a".into()),
            kind: ItemKind::Folder,
            title: "根目录项目".into(),
            target: target.into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
        wait(&e, |v| matches!(v, Event::Saved(_)));
        let w = work(&e);
        submit(&e, &w, "s", "负例").unwrap();
        let ps = finish(&e, &w).unwrap();
        e.send(Command::OrganizerComplete {
            token: w.token,
            result: Ok(ps),
        })
        .unwrap();
        let _ = snapshot(&e);
        e.send(Command::HarnessFeedback {
            request_id: 1000,
            sources: vec![seen_target(&snapshot(&e).proposals[0])],
            reason: "保留根目录标识".into(),
            scope: FeedbackScope::Directory,
        })
        .unwrap();
        assert!(receipt(&e, 1000).0.is_ok());
        assert_eq!(
            memory(&e).records[0].scope,
            MemoryScope::Directory {
                path: expected.into()
            }
        );
        stop(&e);
        let e = open(&p);
        assert_eq!(
            memory(&e).records[0].scope,
            MemoryScope::Directory {
                path: expected.into()
            }
        );
        stop(&e);
    }
}

fn seed_near_capacity_memories(
    path: &std::path::Path,
    confirmed: bool,
) -> Vec<quick_panel::harness_tools::MemoryRecord> {
    use quick_panel::harness_tools::*;
    let make = |id: u64, source: String, protected: bool| MemoryRecord {
        id,
        scope: if id == 1 {
            MemoryScope::Global
        } else {
            MemoryScope::Item {
                source: Source::SavedItem { id: source },
            }
        },
        kind: MemoryKind::Rule,
        origin: MemoryOrigin::Human,
        proposed_by_ai: false,
        text: if id == 1 {
            "保护规则".into()
        } else {
            "😀".repeat(512)
        },
        alias: None,
        tags: vec![],
        evidence_ids: vec![],
        confirmed: protected,
        enabled: true,
        created_at_ms: 1,
    };
    let encode = |records: &Vec<MemoryRecord>| {
        serde_json::to_vec(&serde_json::json!({"version":0,"next_id":records.last().unwrap().id,"memories":records,"run":null,"human_requests":[]})).unwrap()
    };
    let mut records = vec![make(1, String::new(), true)];
    loop {
        let id = records.len() as u64 + 1;
        records.push(make(id, format!("{}-{id}", "界".repeat(4090)), confirmed));
        if encode(&records).len() > 512_000 {
            records.pop();
            break;
        }
    }
    let id = records.len() as u64 + 1;
    let (mut low, mut high) = (0usize, 4090usize);
    while low < high {
        let mid = (low + high).div_ceil(2);
        records.push(make(id, format!("{}-{id}", "😀".repeat(mid)), confirmed));
        let fits = encode(&records).len() <= 512_000;
        records.pop();
        if fits {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    records.push(make(id, format!("{}-{id}", "😀".repeat(low)), confirmed));
    let data = encode(&records);
    assert!((511_984..=512_000).contains(&data.len()));
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE harness_state(id INTEGER PRIMARY KEY CHECK(id=1),data TEXT NOT NULL)",
    )
    .unwrap();
    db.execute(
        "INSERT INTO harness_state VALUES(1,?1)",
        [String::from_utf8(data).unwrap()],
    )
    .unwrap();
    records
}
fn large_favorite(e: &Engine) {
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some("large".into()),
        kind: ItemKind::Folder,
        title: "😀".repeat(256),
        target: format!("C:/{}", "😀".repeat(300)),
        alias: "😀".repeat(256),
        tags: (0..32).map(|i| format!("{i}{}", "😀".repeat(60))).collect(),
        open_with: None,
    }))
    .unwrap();
    wait(e, |v| matches!(v, Event::Saved(_)));
}
#[test]
fn beginning_larger_batch_reclaims_oldest_ordinary_memory_without_search_error_or_losing_rule() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("begin-evict");
    let e = open(&p);
    settings(&e, config());
    large_favorite(&e);
    favorite(&e, "small");
    stop(&e);
    let before = seed_near_capacity_memories(&p, false);
    let e = open(&p);
    assert!(memory(&e).storage_error.is_none());
    let w = work(&e);
    assert_eq!(w.candidates.len(), 2);
    let after = memory(&e);
    assert!(after.records.len() < before.len());
    assert_eq!(after.records[0].text, "保护规则");
    assert!(after.records[0].confirmed);
    assert!(!after.records.iter().any(|m| m.id == before[1].id));
    assert_eq!(snapshot(&e).failures, 0);
    stop(&e);
}
#[test]
fn unstartable_capacity_item_becomes_local_attention_and_later_item_and_explicit_retry_recover() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("begin-attention");
    let e = open(&p);
    settings(&e, config());
    large_favorite(&e);
    favorite(&e, "small");
    stop(&e);
    let protected = seed_near_capacity_memories(&p, true);
    let e = open(&p);
    assert!(memory(&e).storage_error.is_none());
    e.send(Command::OrganizerTick).unwrap();
    let s = snapshot(&e);
    assert_eq!(s.attention.len(), 1);
    assert_eq!(s.attention[0].code, "CAPACITY");
    assert_eq!(
        s.attention[0].source,
        Source::SavedItem { id: "large".into() }
    );
    assert_eq!(s.queued, 1);
    assert_eq!(s.failures, 0);
    assert!(!s.paused && s.retry_at_ms.is_none());
    assert_eq!(memory(&e).records, protected);
    let next = work(&e);
    assert_eq!(
        next.candidates[0].source,
        Source::SavedItem { id: "small".into() }
    );
    assert_eq!(snapshot(&e).attention.len(), 1);
    submit(&e, &next, "small-submit", "可容纳小项").unwrap();
    let proposals = finish(&e, &next).unwrap();
    e.send(Command::OrganizerComplete {
        token: next.token,
        result: Ok(proposals),
    })
    .unwrap();
    assert_eq!(snapshot(&e).proposals.len(), 1);
    e.send(Command::HarnessMemoryDelete {
        request_id: 2001,
        ids: protected.iter().map(|m| m.id).collect(),
    })
    .unwrap();
    assert!(receipt(&e, 2001).0.is_ok());
    e.send(Command::OrganizerRetry).unwrap();
    assert!(snapshot(&e).attention.is_empty());
    let resumed = work(&e);
    assert_eq!(resumed.candidates.len(), 1);
    assert_eq!(
        resumed.candidates[0].source,
        Source::SavedItem { id: "large".into() }
    );
    stop(&e);
}
#[test]
fn disabled_feedback_returns_correlated_error_without_learning_or_requeue() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("feedback-off"));
    let mut settings_value = config();
    settings_value.organizer.memory_enabled = false;
    settings(&e, settings_value);
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "所见别名").unwrap();
    let p = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(p),
    })
    .unwrap();
    let before = snapshot(&e);
    e.send(Command::HarnessFeedback {
        request_id: 2002,
        sources: vec![seen_target(&snapshot(&e).proposals[0])],
        reason: "保留输入".into(),
        scope: FeedbackScope::Item,
    })
    .unwrap();
    let (result, pending) = receipt(&e, 2002);
    assert!(result.unwrap_err().contains("启用"));
    assert!(!pending);
    let after = snapshot(&e);
    assert_eq!(after.proposals, before.proposals);
    assert_eq!(after.queued, before.queued);
    assert!(memory(&e).records.is_empty());
    stop(&e);
}

fn seen_target(p: &Proposal) -> quick_panel::harness_tools::FeedbackTarget {
    quick_panel::harness_tools::FeedbackTarget {
        source: p.candidate.source.clone(),
        expected_version: p.candidate.expected_version.clone(),
        rejected_alias: p.alias.clone(),
    }
}
#[test]
fn feedback_rejects_same_version_ready_alias_advanced_after_the_user_snapshot() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("seen-alias");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "用户所见").unwrap();
    let ps = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(ps),
    })
    .unwrap();
    let shown = seen_target(&snapshot(&e).proposals[0]);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute("UPDATE organizer_state SET data=json_set(data,'$.records[0].status.Ready.alias','已推进新建议')",[]).unwrap();
    drop(db);
    let e = open(&p);
    let before = snapshot(&e);
    assert_eq!(before.proposals[0].alias.as_deref(), Some("已推进新建议"));
    assert_eq!(
        before.proposals[0].candidate.expected_version,
        shown.expected_version
    );
    e.send(Command::HarnessFeedback {
        request_id: 2100,
        sources: vec![shown],
        reason: "旧界面反馈".into(),
        scope: FeedbackScope::Item,
    })
    .unwrap();
    let (result, pending) = receipt(&e, 2100);
    assert!(result.is_err());
    assert!(!pending);
    assert!(memory(&e).records.is_empty());
    assert_eq!(snapshot(&e).proposals, before.proposals);
    stop(&e);
}
#[test]
fn feedback_on_processing_and_attention_without_displayed_alias_never_learns_hidden_staged_alias() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("hidden-stage"));
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "未展示的暂存名称").unwrap();
    let target = FeedbackTarget {
        source: w.candidates[0].source.clone(),
        expected_version: w.candidates[0].expected_version.clone(),
        rejected_alias: None,
    };
    e.send(Command::HarnessFeedback {
        request_id: 2101,
        sources: vec![target],
        reason: "调整当前内容".into(),
        scope: FeedbackScope::Item,
    })
    .unwrap();
    assert!(receipt(&e, 2101).0.is_ok());
    assert!(memory(&e).records[0].alias.is_none());
    let w = work(&e);
    submit(&e, &w, "s2", "另一隐藏名称").unwrap();
    e.send(Command::HarnessFailed {
        token: w.token,
        code: "incomplete".into(),
        message: "合成需处理".into(),
        retryable: false,
    })
    .unwrap();
    e.send(Command::OrganizerProgress { request_id: 909 })
        .unwrap();
    let Event::OrganizerProgress { progress, .. } = wait(&e, |v| {
        matches!(
            v,
            Event::OrganizerProgress {
                request_id: 909,
                ..
            }
        )
    }) else {
        panic!()
    };
    assert!(
        progress.label().contains("需处理 1 条"),
        "{}",
        progress.label()
    );
    assert!(!progress.label().contains("暂无待办"));
    let attention = snapshot(&e).attention[0].clone();
    let target = FeedbackTarget {
        source: attention.source,
        expected_version: attention.expected_version,
        rejected_alias: None,
    };
    e.send(Command::HarnessFeedback {
        request_id: 2102,
        sources: vec![target],
        reason: "调整需处理内容".into(),
        scope: FeedbackScope::Item,
    })
    .unwrap();
    assert!(receipt(&e, 2102).0.is_ok());
    assert!(memory(&e).records.iter().all(|m| m.alias.is_none()));
    stop(&e);
}
#[test]
fn feedback_rejects_old_seen_source_version_after_actual_title_edit_and_same_alias_ready() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("seen-version"));
    settings(&e, config());
    favorite(&e, "a");
    let first = work(&e);
    submit(&e, &first, "s", "相同显示名称").unwrap();
    let ps = finish(&e, &first).unwrap();
    e.send(Command::OrganizerComplete {
        token: first.token,
        result: Ok(ps),
    })
    .unwrap();
    let shown = seen_target(&snapshot(&e).proposals[0]);
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some("a".into()),
        kind: ItemKind::Folder,
        title: "人工修改后的标题".into(),
        target: "C:/Project/a".into(),
        alias: String::new(),
        tags: vec![],
        open_with: None,
    }))
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    e.send(Command::OrganizerRetry).unwrap();
    let _ = snapshot(&e);
    let changed = work(&e);
    assert_ne!(
        changed.candidates[0].expected_version,
        shown.expected_version
    );
    submit(&e, &changed, "s2", "相同显示名称").unwrap();
    let ps = finish(&e, &changed).unwrap();
    e.send(Command::OrganizerComplete {
        token: changed.token,
        result: Ok(ps),
    })
    .unwrap();
    let before = snapshot(&e);
    assert_eq!(before.proposals[0].alias, shown.rejected_alias);
    e.send(Command::HarnessFeedback {
        request_id: 2200,
        sources: vec![shown],
        reason: "旧版本所见反馈".into(),
        scope: FeedbackScope::Item,
    })
    .unwrap();
    let (result, pending) = receipt(&e, 2200);
    assert!(result.is_err());
    assert!(!pending);
    assert!(memory(&e).records.is_empty());
    assert_eq!(snapshot(&e).proposals, before.proposals);
    stop(&e);
}
fn receipt_after_failed_write(e: &Engine, id: u64) -> (Result<(), String>, bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match e.try_recv() {
            Ok(Event::HarnessMemoryReceipt {
                request_id,
                result,
                pending_save,
            }) if request_id == id => return (result, pending_save),
            Ok(_) => {}
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(err) => panic!("{err}"),
        }
    }
}
#[test]
fn disabled_memory_allows_same_submitted_feedback_save_retry_and_explicit_memory_management() {
    use quick_panel::harness_tools::*;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("feedback-save-retry");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "a");
    let w = work(&e);
    submit(&e, &w, "s", "所见名称").unwrap();
    let ps = finish(&e, &w).unwrap();
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(ps),
    })
    .unwrap();
    let shown = seen_target(&snapshot(&e).proposals[0]);
    e.send(Command::Flush).unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    let db = Connection::open(&p).unwrap();
    db.execute_batch("CREATE TRIGGER fail_feedback BEFORE UPDATE ON harness_state BEGIN SELECT RAISE(FAIL,'synthetic denied'); END;").unwrap();
    let cmd = Command::HarnessFeedback {
        request_id: 2201,
        sources: vec![shown.clone()],
        reason: "待保存的已提交反馈".into(),
        scope: FeedbackScope::Item,
    };
    e.send(cmd.clone()).unwrap();
    let (result, pending) = receipt_after_failed_write(&e, 2201);
    assert!(result.is_err() && pending);
    let mut off = config();
    off.organizer.memory_enabled = false;
    e.send(Command::SaveSettings(off)).unwrap();
    e.send(Command::HarnessMemorySnapshot { request_id: 2202 })
        .unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < end);
        if let Ok(Event::HarnessMemoryState {
            request_id: 2202,
            snapshot,
        }) = e.try_recv()
        {
            assert_eq!(snapshot.records.len(), 1);
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    db.execute_batch("DROP TRIGGER fail_feedback").unwrap();
    e.send(cmd).unwrap();
    let (result, pending) = receipt(&e, 2201);
    assert!(result.is_ok() && !pending);
    assert_eq!(memory(&e).records.len(), 1);
    e.send(Command::HarnessFeedback {
        request_id: 2203,
        sources: vec![shown],
        reason: "关闭后新反馈".into(),
        scope: FeedbackScope::Item,
    })
    .unwrap();
    assert!(receipt(&e, 2203).0.unwrap_err().contains("启用"));
    let saved = memory(&e).records[0].clone();
    e.send(Command::HarnessMemoryEdit {
        request_id: 2204,
        input: MemoryEdit {
            id: Some(saved.id),
            scope: saved.scope,
            text: "关闭时明确编辑".into(),
            enabled: false,
            confirmed: true,
        },
    })
    .unwrap();
    assert!(receipt(&e, 2204).0.is_ok());
    e.send(Command::HarnessMemoryDelete {
        request_id: 2205,
        ids: vec![saved.id],
    })
    .unwrap();
    assert!(receipt(&e, 2205).0.is_ok());
    assert!(memory(&e).records.is_empty());
    stop(&e);
}
#[test]
fn start_capacity_shrinks_new_batch_prefix_without_evicting_confirmed_memory_or_losing_tail() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("begin-prefix");
    let e = open(&p);
    settings(&e, config());
    favorite(&e, "small");
    large_favorite(&e);
    stop(&e);
    let protected = seed_near_capacity_memories(&p, true);
    let e = open(&p);
    assert!(memory(&e).storage_error.is_none());
    let w = work(&e);
    assert_eq!(w.candidates.len(), 1);
    assert_eq!(
        w.candidates[0].source,
        Source::SavedItem { id: "small".into() }
    );
    let state = snapshot(&e);
    assert_eq!(state.queued, 1);
    assert!(state.attention.is_empty());
    assert_eq!(state.failures, 0);
    assert_eq!(memory(&e).records, protected);
    stop(&e);
}

#[test]
fn memory_source_labels_are_bounded_read_only_metadata_and_persisted_receipts_use_fresh_ids() {
    use quick_panel::{harness_tools::*, organizer_ui::memory_request_id};
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("labels");
    let e = open(&path);
    settings(&e, config());
    favorite(&e, "labels");
    let id = memory_request_id();
    let input = MemoryEdit {
        id: None,
        scope: MemoryScope::Item {
            source: Source::SavedItem {
                id: "labels".into(),
            },
        },
        text: "保留项目名".into(),
        enabled: true,
        confirmed: true,
    };
    e.send(Command::HarnessMemoryEdit {
        request_id: id,
        input: input.clone(),
    })
    .unwrap();
    assert!(receipt(&e, id).0.is_ok());
    let db = Connection::open(&path).unwrap();
    let before = data_version(&db);
    for _ in 0..4 {
        let snapshot = memory(&e);
        assert_eq!(
            snapshot.source_labels,
            vec![(
                Source::SavedItem {
                    id: "labels".into()
                },
                "项目日志".into()
            )]
        );
    }
    assert_eq!(before, data_version(&db));
    stop(&e);
    let e = open(&path);
    let next = memory_request_id();
    assert_ne!(id, next);
    let mut input = input;
    input.text = "重新启动后的明确修改".into();
    input.id = Some(memory(&e).records[0].id);
    e.send(Command::HarnessMemoryEdit {
        request_id: next,
        input,
    })
    .unwrap();
    assert!(receipt(&e, next).0.is_ok());
    assert_eq!(memory(&e).records[0].text, "重新启动后的明确修改");
    stop(&e);
}
