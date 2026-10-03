use quick_panel::{
    model::{Command, Scope},
    semantic::{IndexBatch, IndexCursor},
    semantic_ui::{IndexSession, QuerySession, cancel_semantic_view, sync_semantic_view},
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
