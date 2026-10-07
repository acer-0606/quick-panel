use quick_panel::{
    model::{Command, Scope},
    semantic::{IndexBatch, IndexCursor},
    semantic_ui::{
        IndexSession, QuerySession, cancel_semantic_view, finish_semantic_effect,
        finish_semantic_effect_with_revoke, prepare_semantic_effect, resume_ordinary_query,
        sync_semantic_view,
    },
    ui_state::UiState,
};

#[test]
fn cancellation_revokes_network_and_adopted_local_results_and_selection() {
    for adopted in [false, true] {
        let mut session = QuerySession::default();
        session.begin(7, "连接", Scope::All, "profile");
        if adopted {
            assert!(session.finish(7, "连接", Scope::All, "profile", true, vec![1., 0.]));
        }
        let mut state = UiState::default();
        let old_request = state.next_request();
        state.replace_ids(vec!["old-result".into()]);
        let mut cache = vec!["old-result"];
        let mut ready = true;
        cancel_semantic_view(&mut session, &mut state, &mut cache, &mut ready);
        assert!(!state.is_current(old_request));
        assert_eq!(state.selected_id(), None);
        assert!(cache.is_empty());
        assert!(!ready);
        assert!(session.local_query(99).is_none());
        assert!(!session.finish(7, "连接", Scope::All, "profile", true, vec![1., 0.]));
    }
}

#[test]
fn changed_query_scope_profile_or_hidden_view_rejects_completion() {
    for (query, scope, profile, active) in [
        ("修改", Scope::All, "p", true),
        ("原始", Scope::Clipboard, "p", true),
        ("原始", Scope::All, "new", true),
        ("原始", Scope::All, "p", false),
    ] {
        let mut session = QuerySession::default();
        session.begin(1, "原始", Scope::All, "p");
        assert!(!session.finish(1, query, scope, profile, active, vec![1., 0.]));
        assert!(session.local_query(8).is_none());
    }
}

#[test]
fn source_refresh_reuses_adopted_vector_with_fresh_local_token() {
    let mut session = QuerySession::default();
    session.begin(1, "连接", Scope::Clipboard, "p");
    assert!(session.finish(1, "连接", Scope::Clipboard, "p", true, vec![0., 1.]));
    let local = session.local_query(42).unwrap();
    assert_eq!(local.request_id, 42);
    assert_eq!(local.vector, [0., 1.]);
    assert_eq!(local.text, "连接");
    assert_eq!(local.scope, Scope::Clipboard);
}

fn batch(id: u64, profile: &str, done: bool, empty: bool) -> IndexBatch {
    IndexBatch {
        request_id: id,
        profile: profile.into(),
        candidates: if empty {
            vec![]
        } else {
            vec![quick_panel::semantic::EmbeddingCandidate {
                id: "saved-id".into(),
                text: "text".into(),
                text_hash: "hash".into(),
            }]
        },
        cursor: IndexCursor {
            after_rowid: 128,
            ..Default::default()
        },
        done,
    }
}
fn token(command: Command) -> u64 {
    if let Command::PrepareSemanticIndex { request_id, .. } = command {
        request_id
    } else {
        panic!("expected page")
    }
}

#[test]
fn index_requires_matching_store_ack_before_next_page_and_keeps_current_hash() {
    let mut session = IndexSession::default();
    let id = token(session.begin("p"));
    assert!(
        session
            .accept_batch(batch(id, "other", false, false))
            .is_none()
    );
    assert_eq!(
        session.accept_batch(batch(id, "p", false, false)).unwrap(),
        ["text"]
    );
    session.network_started(9);
    assert!(session.network_result(8, "p", vec![vec![1., 0.]]).is_none());
    let store = session.network_result(9, "p", vec![vec![1., 0.]]).unwrap();
    match store {
        Command::StoreEmbeddings {
            request_id,
            profile,
            items,
        } => {
            assert_eq!(request_id, id);
            assert_eq!(profile, "p");
            assert_eq!(items[0].id, "saved-id");
            assert_eq!(items[0].text_hash, "hash");
        }
        _ => panic!("expected store"),
    }
    assert!(session.accept_batch(batch(id, "p", false, false)).is_none());
    assert!(session.acknowledge(id + 1, 1).is_none());
    let next = session.acknowledge(id, 1).unwrap();
    if let Command::PrepareSemanticIndex { request_id, cursor } = next {
        assert!(request_id > id);
        assert_eq!(cursor.after_rowid, 128);
    } else {
        panic!("expected next page");
    }
    assert_eq!(session.accepted(), 1);
}

#[test]
fn empty_nonfinal_pages_advance_and_cancelled_sessions_reject_old_replies() {
    let mut session = IndexSession::default();
    let old = token(session.begin("p"));
    assert_eq!(
        session.accept_batch(batch(old, "p", false, true)).unwrap(),
        Vec::<String>::new()
    );
    let next = token(session.empty_page().unwrap());
    assert!(next > old);
    session.cancel();
    let current = token(session.begin("new"));
    assert!(current > next);
    assert!(
        session
            .accept_batch(batch(next, "p", true, false))
            .is_none()
    );
    assert!(
        session
            .accept_batch(batch(current, "new", true, true))
            .is_some()
    );
    assert!(session.empty_page().is_none());
    assert!(!session.active());
}

#[test]
fn backend_error_without_ack_stops_index_but_query_remains_independent() {
    let mut session = IndexSession::default();
    let id = token(session.begin("p"));
    session.accept_batch(batch(id, "p", true, false));
    session.network_started(9);
    session.network_result(9, "p", vec![vec![1., 0.]]).unwrap();
    session.cancel();
    assert!(session.acknowledge(id, 1).is_none());
    assert!(!session.active());
}

#[test]
fn edits_scope_settings_return_and_hide_clear_adopted_selection_immediately() {
    for (query, scope, profile, active) in [
        ("edited", Scope::All, "p", true),
        ("original", Scope::Folder, "p", true),
        ("original", Scope::All, "changed", true),
        ("original", Scope::All, "p", false),
    ] {
        let mut session = QuerySession::default();
        session.begin(1, "original", Scope::All, "p");
        session.finish(1, "original", Scope::All, "p", true, vec![1., 0.]);
        let mut state = UiState::default();
        let old = state.next_request();
        state.replace_ids(vec!["stale".into()]);
        let mut saved = vec!["stale"];
        let mut ready = true;
        assert!(sync_semantic_view(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            (query, scope, profile, active)
        ));
        assert!(!state.is_current(old));
        assert_eq!(state.selected_id(), None);
        assert!(saved.is_empty());
        assert!(!ready);
        assert!(session.local_query(8).is_none());
    }
}

#[test]
fn hiding_query_preserves_user_started_index_and_original_input_hash() {
    let mut index = IndexSession::default();
    let id = token(index.begin("p"));
    index.accept_batch(batch(id, "p", true, false));
    index.network_started(9);
    let mut query = QuerySession::default();
    query.begin(1, "text", Scope::All, "p");
    let mut state = UiState::default();
    let mut saved: Vec<String> = vec![];
    let mut ready = false;
    cancel_semantic_view(&mut query, &mut state, &mut saved, &mut ready);
    assert!(index.active());
    match index.network_result(9, "p", vec![vec![1., 0.]]).unwrap() {
        Command::StoreEmbeddings { items, .. } => assert_eq!(items[0].text_hash, "hash"),
        _ => panic!("expected store"),
    }
    assert!(index.acknowledge(id, 0).is_none());
    assert!(!index.active());
    assert_eq!(index.accepted(), 0);
}

#[test]
fn result_menu_keeps_target_cache_and_adopted_mode_while_stopping_pending_network() {
    for adopted in [false, true] {
        let mut session = QuerySession::default();
        session.begin(7, "query", Scope::All, "p");
        if adopted {
            session.finish(7, "query", Scope::All, "p", true, vec![1., 0.]);
        }
        let mut state = UiState::default();
        state.visible = true;
        let old_request = state.next_request();
        state.replace_ids(vec!["real-id".into()]);
        let target = state.menu_target();
        let mut saved = vec!["real-id"];
        let mut ready = true;
        prepare_semantic_effect(&mut session, &mut state, &mut saved, &mut ready, true);
        state.modal_open = true;
        assert!(!state.menu_target_current(&target));
        state.modal_open = false;
        assert!(state.menu_target_current(&target));
        assert!(state.is_current(old_request));
        assert_eq!(state.selected_id(), Some("real-id"));
        assert_eq!(saved, ["real-id"]);
        assert!(ready);
        assert_eq!(session.adopted(), adopted);
        assert!(!session.busy());
        assert!(!session.finish(7, "query", Scope::All, "p", true, vec![1., 0.]));
        // The existing guard also gates the subsequent real load/action path.
        assert!(state.begin_action(91));
        assert!(state.action_current(91));
    }
}

#[test]
fn result_menu_still_rejects_target_after_selection_or_source_refresh() {
    for source_refresh in [false, true] {
        let mut session = QuerySession::default();
        session.begin(7, "query", Scope::All, "p");
        session.finish(7, "query", Scope::All, "p", true, vec![1., 0.]);
        let mut state = UiState::default();
        state.visible = true;
        state.replace_ids(vec!["real-id".into(), "other-id".into()]);
        let target = state.menu_target();
        let mut saved = vec!["real-id", "other-id"];
        let mut ready = true;
        prepare_semantic_effect(&mut session, &mut state, &mut saved, &mut ready, true);
        if source_refresh {
            state.next_request();
        } else {
            state.select(1);
        }
        assert!(!state.menu_target_current(&target));
    }
}

#[test]
fn general_settings_cancel_or_failure_resumes_fresh_ordinary_query() {
    for cancelled in [false, true] {
        let mut session = QuerySession::default();
        session.begin(7, "设置", Scope::Clipboard, "p");
        session.finish(7, "设置", Scope::Clipboard, "p", true, vec![1., 0.]);
        let mut state = UiState::default();
        state.visible = true;
        let old = state.next_request();
        state.replace_ids(vec!["old-result".into()]);
        let mut saved = vec!["old-result"];
        let mut ready = true;
        prepare_semantic_effect(&mut session, &mut state, &mut saved, &mut ready, false);
        state.modal_open = true;
        // Cancel yields no settings; a registration failure leaves existing scope/query.
        let return_path = if cancelled {
            "cancelled settings"
        } else {
            "failed settings apply"
        };
        let restored = resume_ordinary_query(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            "设置",
            Scope::Clipboard,
        )
        .unwrap_or_else(|| panic!("{return_path} must issue local query"));
        assert_eq!(restored.text, "设置");
        assert_eq!(restored.scope, Scope::Clipboard);
        assert!(state.is_current(restored.request_id));
        assert!(!state.is_current(old));
        assert!(!state.modal_open);
        assert!(saved.is_empty());
        assert!(!ready);
        assert!(session.local_query(99).is_none());
        // Delayed pre-dialog results cannot satisfy the restored request.
        assert!(restored.request_id > old);
    }
}

// Restoring a modal must not reload a panel hidden during the native pump.
#[test]
fn modal_return_while_root_hidden_does_not_issue_ordinary_query() {
    let mut session = QuerySession::default();
    session.begin(7, "退款", Scope::Document, "p");
    session.finish(7, "退款", Scope::Document, "p", true, vec![1., 0.]);
    let mut state = UiState::default();
    state.visible = true;
    state.replace_ids(vec!["document".into()]);
    let mut saved = vec!["document"];
    let mut ready = true;
    assert!(prepare_semantic_effect(
        &mut session,
        &mut state,
        &mut saved,
        &mut ready,
        false
    ));
    state.modal_open = true;
    state.hide();
    assert!(
        finish_semantic_effect(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            "退款",
            Scope::Document,
            true
        )
        .is_none()
    );
    assert!(!state.modal_open);
    assert!(!state.visible);
    assert!(!session.adopted());
}

// Picker cancel/error and Favorite cancel/blank validation all complete the same gate.
#[test]
fn revoked_picker_and_favorite_returns_resume_fresh_ordinary_query() {
    for path in [
        "picker cancelled",
        "picker failed",
        "favorite cancelled",
        "blank name",
        "blank path",
    ] {
        let mut session = QuerySession::default();
        session.begin(7, "退款", Scope::Document, "p");
        session.finish(7, "退款", Scope::Document, "p", true, vec![1., 0.]);
        let mut state = UiState::default();
        state.visible = true;
        let old = state.next_request();
        state.replace_ids(vec!["document".into()]);
        let target = state.menu_target();
        assert!(state.begin_action(90));
        let mut saved = vec!["document"];
        let mut ready = true;
        assert!(prepare_semantic_effect(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            false
        ));
        state.modal_open = true;
        let restored = finish_semantic_effect(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            "退款",
            Scope::Document,
            true,
        )
        .unwrap_or_else(|| panic!("{path}"));
        assert_eq!(restored.text, "退款");
        assert_eq!(restored.scope, Scope::Document);
        assert!(state.is_current(restored.request_id));
        assert!(!state.is_current(old));
        assert!(!state.action_current(90));
        assert!(!state.menu_target_current(&target));
        assert!(!state.modal_open);
        assert!(saved.is_empty());
        assert!(!ready);
        assert!(session.local_query(99).is_none());
    }
}

#[test]
fn modal_completion_preserves_menu_target_and_new_semantic_request() {
    for menu in [true, false] {
        let mut session = QuerySession::default();
        session.begin(7, "query", Scope::All, "p");
        session.finish(7, "query", Scope::All, "p", true, vec![1., 0.]);
        let mut state = UiState::default();
        state.visible = true;
        let old = state.next_request();
        state.replace_ids(vec!["real-id".into()]);
        let target = state.menu_target();
        let mut saved = vec!["real-id"];
        let mut ready = true;
        let revoked =
            prepare_semantic_effect(&mut session, &mut state, &mut saved, &mut ready, menu);
        state.modal_open = true;
        if !menu {
            session.begin(8, "new query", Scope::All, "p");
        }
        assert!(
            finish_semantic_effect(
                &mut session,
                &mut state,
                &mut saved,
                &mut ready,
                "query",
                Scope::All,
                revoked
            )
            .is_none()
        );
        assert!(!state.modal_open);
        if menu {
            assert!(state.menu_target_current(&target));
            assert!(state.is_current(old));
            assert_eq!(saved, ["real-id"]);
            assert!(ready);
            assert!(session.adopted());
        } else {
            assert!(session.current(8, "new query", Scope::All, "p", true));
            assert!(!state.menu_target_current(&target));
        }
    }
}

#[test]
fn revoked_modal_return_during_shutdown_never_issues_query() {
    use quick_panel::ui_state::Lifecycle;
    for lifecycle in [
        Lifecycle::Draining,
        Lifecycle::Saving,
        Lifecycle::SaveFailed,
    ] {
        let mut session = QuerySession::default();
        session.begin(7, "query", Scope::All, "p");
        session.finish(7, "query", Scope::All, "p", true, vec![1., 0.]);
        let mut state = UiState::default();
        state.visible = true;
        let mut saved = vec!["real-id"];
        let mut ready = true;
        let revoked =
            prepare_semantic_effect(&mut session, &mut state, &mut saved, &mut ready, false);
        state.modal_open = true;
        state.lifecycle = lifecycle;
        assert!(
            finish_semantic_effect(
                &mut session,
                &mut state,
                &mut saved,
                &mut ready,
                "query",
                Scope::All,
                revoked
            )
            .is_none()
        );
        assert!(!state.modal_open);
        assert_eq!(state.lifecycle, lifecycle);
        assert!(saved.is_empty());
        assert!(session.local_query(90).is_none());
    }
}

// The native completion contract must revoke Claude before returning its final token.
#[test]
fn adopted_claude_plan_modal_cancel_returns_current_ordinary_search_token() {
    use quick_panel::{ai::AiPlan, ai_jobs::revoke_adopted_plan, model::ItemSummary};
    let mut session = QuerySession::default();
    let mut state = UiState::default();
    state.visible = true;
    let old = state.next_request();
    state.replace_ids(vec!["old-ai-result".into()]);
    let mut plan = Some(AiPlan {
        queries: vec!["退款".into()],
        ..AiPlan::default()
    });
    let mut saved: Vec<ItemSummary> = vec![];
    let mut ready = true;
    state.modal_open = true;
    // General-settings cancellation forces ordinary restoration even without a semantic mode.
    let search = finish_semantic_effect_with_revoke(
        &mut session,
        &mut state,
        &mut saved,
        &mut ready,
        ("退款", Scope::Document, true),
        |state, saved, ready| {
            revoke_adopted_plan(&mut plan, state, saved, ready);
        },
    )
    .expect("settings cancellation must send an ordinary search");
    assert!(plan.is_none());
    assert!(!state.is_current(old));
    let Command::Search(sent) = Command::Search(search) else {
        unreachable!()
    };
    assert!(
        state.is_current(sent.request_id),
        "final Search token was invalidated by Claude plan revocation"
    );
    assert_eq!(sent.text, "退款");
    assert_eq!(sent.scope, Scope::Document);
    // This is the same token predicate used by native Event::Results acceptance.
    if state.is_current(sent.request_id) {
        state.replace_ids(vec!["ordinary-result".into()]);
    }
    assert_eq!(state.selected_id(), Some("ordinary-result"));
    assert!(!state.modal_open);
}

#[test]
fn modal_completion_guards_do_not_revoke_claude_plan() {
    use quick_panel::{
        ai::AiPlan, ai_jobs::revoke_adopted_plan, model::ItemSummary, ui_state::Lifecycle,
    };
    for guarded in [
        "menu",
        "new semantic request",
        "new semantic results",
        "hidden",
        "shutdown",
    ] {
        let mut session = QuerySession::default();
        let mut state = UiState::default();
        state.visible = true;
        let old = state.next_request();
        state.replace_ids(vec!["ai-result".into()]);
        let mut plan = Some(AiPlan::default());
        let mut saved: Vec<ItemSummary> = vec![];
        let mut ready = true;
        let revoked = guarded != "menu";
        match guarded {
            "new semantic request" => session.begin(9, "new", Scope::All, "p"),
            "new semantic results" => {
                session.begin(9, "new", Scope::All, "p");
                assert!(session.finish(9, "new", Scope::All, "p", true, vec![1., 0.]));
            }
            "hidden" => state.hide(),
            "shutdown" => state.lifecycle = Lifecycle::Saving,
            _ => (),
        }
        state.modal_open = true;
        assert!(
            finish_semantic_effect_with_revoke(
                &mut session,
                &mut state,
                &mut saved,
                &mut ready,
                ("退款", Scope::Document, revoked),
                |state, saved, ready| {
                    revoke_adopted_plan(&mut plan, state, saved, ready);
                },
            )
            .is_none(),
            "{guarded}"
        );
        assert!(
            plan.is_some(),
            "{guarded} must not revoke the adopted Claude plan"
        );
        assert!(state.is_current(old));
        assert!(ready);
        assert!(!state.modal_open);
    }
}

#[test]
fn cancelled_index_retains_receipts_and_requests_exactly_one_refresh() {
    use quick_panel::semantic_ui::IndexOutcome;
    let mut session = IndexSession::default();
    let id = token(session.begin("p"));
    session.accept_batch(batch(id, "p", false, false));
    session.network_started(9);
    session.network_result(9, "p", vec![vec![1., 0.]]).unwrap();
    session.acknowledge(id, 1).unwrap();
    session.cancel();
    assert_eq!(session.outcome(), IndexOutcome::Cancelled);
    assert_eq!(session.accepted(), 1);
    assert!(session.take_status_refresh());
    session.cancel();
    assert!(!session.take_status_refresh());
    let current = quick_panel::model::SemanticIndexStatus {
        profile: "p".into(),
        indexed: 4,
        needs_update: true,
    };
    assert!(
        session
            .label(Some(&current))
            .contains("已取消 · 保留可用 4 条")
    );
    assert!(session.acknowledge(id, 99).is_none());
    assert_eq!(session.accepted(), 1);
}

#[test]
fn zero_completion_explains_eligible_content_and_failure_is_distinct() {
    use quick_panel::semantic_ui::IndexOutcome;
    let mut session = IndexSession::default();
    let id = token(session.begin("p"));
    session.accept_batch(batch(id, "p", true, true));
    assert!(session.empty_page().is_none());
    assert_eq!(session.outcome(), IndexOutcome::Completed);
    let empty = quick_panel::model::SemanticIndexStatus {
        profile: "p".into(),
        indexed: 0,
        needs_update: false,
    };
    let label = session.label(Some(&empty));
    assert!(label.contains("没有可索引内容"));
    assert!(label.contains("固定文本") && label.contains("索引范围"));
    assert!(session.needs_content_guidance(Some(&empty)));
    let changed = quick_panel::model::SemanticIndexStatus {
        needs_update: true,
        ..empty.clone()
    };
    let changed_label = session.label(Some(&changed));
    assert!(changed_label.contains("内容待更新"));
    assert!(!changed_label.contains("没有可索引内容"));
    session.begin("p");
    session.fail();
    assert_eq!(session.outcome(), IndexOutcome::Failed);
    assert!(session.label(Some(&empty)).contains("已停止"));
}

#[test]
fn zero_new_embeddings_with_existing_vectors_reports_up_to_date_not_empty() {
    let mut session = IndexSession::default();
    let id = token(session.begin("p"));
    session.accept_batch(batch(id, "p", true, true));
    assert!(session.empty_page().is_none());
    assert_eq!(session.accepted(), 0);
    let status = quick_panel::model::SemanticIndexStatus {
        profile: "p".into(),
        indexed: 12,
        needs_update: false,
    };
    let label = session.label(Some(&status));
    assert!(label.contains("已是最新"));
    assert!(label.contains("可用 12 条"));
    assert!(!label.contains("没有可索引内容"));
    assert!(!label.contains("固定文本"));
}

#[test]
fn settings_hub_browsing_preserves_rows_until_whole_hub_close_restores_local_query() {
    for (visible, running) in [(true, true), (false, true), (true, false)] {
        let mut session = QuerySession::default();
        session.begin(91, "kept", Scope::Clipboard, "profile");
        assert!(session.finish(
            91,
            "kept",
            Scope::Clipboard,
            "profile",
            true,
            vec![1.0, 0.0]
        ));
        let mut state = UiState::default();
        state.visible = visible;
        let mut saved = vec!["row"];
        let mut ready = true;
        // Actual native hub entry uses the preserving modal gate. Group and leaf
        // navigation consume no gate and issue no retrieval command.
        assert!(!prepare_semantic_effect(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            true
        ));
        assert_eq!(saved, vec!["row"]);
        assert!(session.adopted());
        state.modal_open = true;
        if !running {
            state.begin_exit();
        }
        // Actual whole-hub exit cancels derived rows then restores through the
        // existing gate, including its hidden/exiting and final-token safeguards.
        prepare_semantic_effect(&mut session, &mut state, &mut saved, &mut ready, false);
        let query = finish_semantic_effect(
            &mut session,
            &mut state,
            &mut saved,
            &mut ready,
            "kept",
            Scope::Clipboard,
            true,
        );
        assert_eq!(query.is_some(), visible && running);
        if let Some(query) = query {
            assert_eq!(query.text, "kept");
            assert_eq!(query.scope, Scope::Clipboard);
            assert!(state.is_current(query.request_id));
        }
        assert!(!session.adopted());
        assert!(!state.modal_open);
    }
}
