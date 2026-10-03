use quick_panel::{
    ai::*,
    ai_jobs::*,
    model::{ItemKind, ItemSummary, Scope, Settings},
    ui_state::{Lifecycle, UiState},
};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

fn request(text: &str) -> AiRequest {
    AiRequest {
        settings: AiSettings {
            enabled: true,
            model: "test-model".into(),
            encrypted_key: vec![1, 2, 3],
            ..AiSettings::default()
        },
        query: text.into(),
        scope: Scope::All,
        functions: vec![],
    }
}
fn wait_result(worker: &AiWorker) -> AiCompletion {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = worker.try_result() {
            return result;
        }
        assert!(Instant::now() < until, "completion timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn legacy_settings_disable_ai_and_debug_never_contains_credential_bytes() {
    let old: Settings =
        serde_json::from_str(r#"{"theme":"Dark","hotkey_modifiers":6,"hotkey_key":80}"#).unwrap();
    assert!(!old.ai.enabled);
    assert!(old.ai.encrypted_key.is_empty());
    let config = AiSettings {
        encrypted_key: b"dummy-private-value".to_vec(),
        ..AiSettings::default()
    };
    let debug = format!("{config:?}");
    assert!(!debug.contains("dummy-private-value"));
    assert!(!debug.contains("100, 117, 109, 109, 121"));
}

#[test]
fn latest_request_replaces_queued_work_and_old_results_never_escape() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = AiWorker::start(
        move |request| {
            started.send(request.query.clone()).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(AiPlan {
                queries: vec![request.query],
                ..AiPlan::default()
            })
        },
        Arc::new(|| {}),
    );
    worker.submit(request("old")).unwrap();
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), "old");
    worker.submit(request("queued-one")).unwrap();
    let latest = worker.submit(request("latest")).unwrap();
    release.send(()).unwrap();
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        "latest"
    );
    assert!(worker.try_result().is_none());
    release.send(()).unwrap();
    let result = wait_result(&worker);
    assert_eq!(result.id, latest);
    assert_eq!(result.result.unwrap().queries, ["latest"]);
}

#[test]
fn drop_does_not_wait_for_a_blocked_network_call() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = AiWorker::start(
        move |_| {
            started.send(()).unwrap();
            releases.lock().unwrap().recv().unwrap();
            finished.send(()).unwrap();
            Ok(AiPlan::default())
        },
        Arc::new(|| {}),
    );
    worker.submit(request("old")).unwrap();
    starts.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.cancel();
    assert!(worker.try_result().is_none());
    let start = Instant::now();
    drop(worker);
    assert!(start.elapsed() < Duration::from_millis(200));
    release.send(()).unwrap();
    finishes.recv_timeout(Duration::from_secs(2)).unwrap();
}

#[test]
fn cancelled_response_is_not_published_even_if_transport_completes_later() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (notified, notices) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = AiWorker::start(
        move |_| {
            started.send(()).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(AiPlan::default())
        },
        Arc::new(move || {
            notified.send(()).unwrap();
        }),
    );
    worker.submit(request("old")).unwrap();
    starts.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.cancel();
    release.send(()).unwrap();
    assert!(notices.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(worker.try_result().is_none());
}

#[test]
fn worker_admission_rejects_disabled_empty_and_unbounded_queries() {
    let worker = AiWorker::start(
        |_| panic!("rejected job must not reach transport"),
        Arc::new(|| {}),
    );
    assert!(worker.submit(request("")).is_err());
    assert!(worker.submit(request(&"中".repeat(2049))).is_err());
    let mut disabled = request("query");
    disabled.settings.enabled = false;
    assert!(worker.submit(disabled).is_err());
}

#[test]
fn completion_gate_rejects_old_query_scope_hidden_and_repeated_results() {
    let mut gate = AiResultGate::default();
    gate.begin(1, "旧查询", Scope::All);
    gate.begin(2, "新查询", Scope::Folder);
    assert!(!gate.finish(1, "旧查询", Scope::All, true));
    assert!(gate.busy());
    assert!(gate.finish(2, "新查询", Scope::Folder, true));
    assert!(!gate.finish(2, "新查询", Scope::Folder, true));
    for (query, scope, active) in [
        ("不同查询", Scope::All, true),
        ("查询", Scope::Document, true),
        ("查询", Scope::All, false),
    ] {
        gate.begin(3, "查询", Scope::All);
        assert!(!gate.finish(3, query, scope, active));
        assert!(!gate.busy());
    }
    gate.begin(4, "查询", Scope::All);
    gate.cancel();
    assert!(!gate.finish(4, "查询", Scope::All, true));
}

fn saved_timeout() -> ItemSummary {
    ItemSummary {
        id: "clip:actual-timeout".into(),
        kind: ItemKind::Clipboard,
        title: "connection timeout".into(),
        subtitle: String::new(),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms: 1,
        last_used_ms: 1,
        use_count: 0,
    }
}

fn adopted_plan() -> (Option<AiPlan>, UiState, u64) {
    let mut gate = AiResultGate::default();
    gate.begin(7, "之前那个连不上的报错", Scope::All);
    assert!(gate.finish(7, "之前那个连不上的报错", Scope::All, true));
    assert!(!gate.busy());
    let mut state = UiState::default();
    state.visible = true;
    let local_id = state.next_request();
    let plan = Some(AiPlan {
        queries: vec!["connection timeout".into()],
        ..AiPlan::default()
    });
    (plan, state, local_id)
}

#[test]
fn revoking_adopted_plan_before_hide_rejects_its_pending_local_results_after_reopen() {
    let (mut plan, mut state, local_id) = adopted_plan();
    let mut saved = vec![];
    let mut ready = false;
    state.replace_ids(vec!["function:ai-recommendation".into()]);
    assert!(state.begin_action(81));

    assert!(revoke_adopted_plan(
        &mut plan, &mut state, &mut saved, &mut ready
    ));
    state.hide();
    state.visible = true;

    assert!(
        !state.is_current(local_id),
        "revoked local AI receipt still accepted"
    );
    assert!(plan.is_none());
    assert!(saved.is_empty());
    assert!(!ready);
    assert!(state.selected_id().is_none());
    assert!(!state.finish_action(81));
    let normal_id = state.next_request();
    assert!(state.is_current(normal_id));
    assert!(!state.is_current(local_id));
}

#[test]
fn ai_settings_cancellation_cannot_reuse_derived_rows_or_their_selected_menu_target() {
    let (mut plan, mut state, local_id) = adopted_plan();
    let mut saved = vec![saved_timeout()];
    let mut ready = true;
    state.replace_ids(vec![saved[0].id.clone()]);
    let old_menu = state.menu_target();

    assert!(revoke_adopted_plan(
        &mut plan, &mut state, &mut saved, &mut ready
    ));
    state.modal_open = true;
    assert!(!state.is_current(local_id));
    state.modal_open = false;

    assert!(
        saved.is_empty(),
        "derived AI cache leaked into normal palette"
    );
    assert!(!ready);
    assert!(state.selected_id().is_none());
    assert!(!state.menu_target_current(&old_menu));
    let normal_id = state.next_request();
    assert!(state.is_current(normal_id));
    assert!(!state.is_current(local_id));
}

#[test]
fn cancelling_failed_exit_does_not_reactivate_a_revoked_local_ai_receipt() {
    let (mut plan, mut state, local_id) = adopted_plan();
    let mut saved = vec![saved_timeout()];
    let mut ready = true;
    state.replace_ids(vec![saved[0].id.clone()]);

    assert!(revoke_adopted_plan(
        &mut plan, &mut state, &mut saved, &mut ready
    ));
    assert!(state.begin_exit());
    assert!(!state.is_current(local_id));
    assert!(state.exit_failed());
    state.cancel_exit();

    assert_eq!(state.lifecycle, Lifecycle::Running);
    assert!(!state.is_current(local_id));
    assert!(plan.is_none());
    assert!(saved.is_empty());
    assert!(!ready);
    assert!(state.selected_id().is_none());
    let normal_id = state.next_request();
    assert!(state.is_current(normal_id));
}

#[test]
fn revocation_without_an_adopted_plan_keeps_normal_cache_and_selection() {
    let mut plan = None;
    let mut state = UiState::default();
    state.visible = true;
    let normal_id = state.next_request();
    let mut saved = vec![saved_timeout()];
    let mut ready = true;
    state.replace_ids(vec![saved[0].id.clone()]);
    let target = state.menu_target();

    assert!(!revoke_adopted_plan(
        &mut plan, &mut state, &mut saved, &mut ready
    ));
    assert!(state.is_current(normal_id));
    assert_eq!(saved.len(), 1);
    assert!(ready);
    assert_eq!(state.selected_id(), Some("clip:actual-timeout"));
    assert!(state.menu_target_current(&target));
}
