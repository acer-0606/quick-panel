use quick_panel::{
    ai::AiPlan,
    model::{Command, ItemKind, ItemSummary, Scope},
    query_feedback::{IndexReply, QueryReply, claude_reply, embedding_reply, index_reply},
    semantic::{EmbeddingCandidate, IndexBatch, IndexCursor},
    semantic_ui::{IndexSession, QuerySession},
    ui_state::UiState,
};

fn local_item() -> ItemSummary {
    ItemSummary {
        id: "local:report".into(),
        kind: ItemKind::Clipboard,
        title: "本地周报".into(),
        subtitle: "编辑器".into(),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms: 1,
        last_used_ms: 1,
        use_count: 0,
    }
}

#[test]
fn repeated_claude_failures_preserve_actionable_local_results() {
    let mut plan = None;
    let mut saved = vec![local_item()];
    let mut ready = true;
    let mut state = UiState::default();
    state.visible = true;
    let token = state.next_request();
    state.replace_ids(vec!["local:report".into()]);
    let target = state.menu_target();
    for error in ["认证失败", "网络超时", "429", "响应格式错误"] {
        assert_eq!(
            claude_reply(Err(error.into()), &mut plan, &mut saved, &mut ready),
            QueryReply::LocalFallback
        );
        assert_eq!(saved[0].id, "local:report");
        assert!(ready && state.is_current(token) && state.menu_target_current(&target));
        assert!(plan.is_none());
    }
}

#[test]
fn successful_claude_reply_still_adopts_plan_and_requests_local_retrieval() {
    let mut adopted = None;
    let mut saved = vec![local_item()];
    let mut ready = true;
    let plan = AiPlan {
        queries: vec!["报销".into()],
        ..Default::default()
    };
    assert_eq!(
        claude_reply(Ok(plan.clone()), &mut adopted, &mut saved, &mut ready),
        QueryReply::Applied
    );
    assert_eq!(adopted, Some(plan));
    assert!(saved.is_empty() && !ready);
}

#[test]
fn embedding_failure_and_invalid_reply_end_pending_work_without_revoking_local_view() {
    for result in [
        Err("API连接失败".into()),
        Ok(vec![]),
        Ok(vec![vec![0., 0.]]),
    ] {
        let mut session = QuerySession::default();
        session.begin(7, "周报", Scope::Clipboard, "profile");
        let mut state = UiState::default();
        state.visible = true;
        let token = state.next_request();
        state.replace_ids(vec!["local:report".into()]);
        let target = state.menu_target();
        assert_eq!(
            embedding_reply(
                result,
                &mut session,
                7,
                ("周报", Scope::Clipboard, "profile", true)
            ),
            QueryReply::LocalFallback
        );
        assert!(!session.busy() && !session.adopted());
        assert!(state.is_current(token) && state.menu_target_current(&target));
    }
}

#[test]
fn stale_embedding_failure_cannot_cancel_new_query_and_success_remains_usable() {
    let mut session = QuerySession::default();
    session.begin(8, "新查询", Scope::All, "profile");
    assert_eq!(
        embedding_reply(
            Err("旧错误".into()),
            &mut session,
            7,
            ("周报", Scope::All, "profile", true)
        ),
        QueryReply::Ignored
    );
    assert!(session.busy());
    assert_eq!(
        embedding_reply(
            Ok(vec![vec![1., 0.]]),
            &mut session,
            8,
            ("新查询", Scope::All, "profile", true)
        ),
        QueryReply::Applied
    );
    assert_eq!(session.local_query(19).unwrap().request_id, 19);
}

fn pending_index() -> IndexSession {
    let mut session = IndexSession::default();
    let request_id = match session.begin("profile") {
        Command::PrepareSemanticIndex { request_id, .. } => request_id,
        _ => unreachable!(),
    };
    session
        .accept_batch(IndexBatch {
            request_id,
            profile: "profile".into(),
            candidates: vec![EmbeddingCandidate {
                id: "local:report".into(),
                text: "周报".into(),
                text_hash: "hash".into(),
            }],
            cursor: IndexCursor::default(),
            done: true,
        })
        .unwrap();
    session.network_started(9);
    session
}

#[test]
fn index_api_failure_stops_without_store_command_or_retry() {
    let mut session = pending_index();
    assert!(matches!(
        index_reply(Err("接口失败".into()), &mut session, 9, "profile"),
        IndexReply::LocalFallback
    ));
    assert!(!session.active());
    assert!(matches!(
        index_reply(Err("重复旧错误".into()), &mut session, 9, "profile"),
        IndexReply::Ignored
    ));
}

#[test]
fn successful_index_reply_still_requires_storage_acknowledgement() {
    let mut session = pending_index();
    let command = match index_reply(Ok(vec![vec![1., 0.]]), &mut session, 9, "profile") {
        IndexReply::Store(command) => command,
        _ => panic!("successful index must reach storage"),
    };
    match *command {
        Command::StoreEmbeddings { items, .. } => assert_eq!(items[0].id, "local:report"),
        _ => panic!("wrong command"),
    }
    assert!(session.active());
}
