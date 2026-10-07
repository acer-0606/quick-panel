use quick_panel::{
    model::*,
    smart_search::{
        ClipboardMetadataForm, SmartAction, SmartPreflight, SmartView, StatusCache, choose,
    },
    ui_state::UiState,
};

fn view() -> SmartView {
    SmartView {
        query: "上周的部署说明".into(),
        scope: Scope::All,
        profile: "p".into(),
        preferred: SmartSearchMode::Auto,
        ai_enabled: true,
        embedding_enabled: true,
    }
}
fn status(indexed: usize) -> SemanticIndexStatus {
    SemanticIndexStatus {
        profile: "p".into(),
        indexed,
        needs_update: indexed == 0,
    }
}
fn request_id(command: Command) -> u64 {
    match command {
        Command::SemanticStatus { request_id } => request_id,
        _ => panic!("status request required"),
    }
}

#[test]
fn auto_prefers_valid_vectors_then_claude_and_never_embeds_an_empty_index() {
    let mut current = view();
    assert_eq!(choose(&current, Some(&status(3))), SmartAction::Semantic);
    assert_eq!(choose(&current, Some(&status(0))), SmartAction::Claude);
    current.ai_enabled = false;
    assert_eq!(choose(&current, Some(&status(0))), SmartAction::BuildIndex);
    current.embedding_enabled = false;
    assert_eq!(choose(&current, None), SmartAction::Configure);
}

#[test]
fn explicit_preferences_route_to_their_provider_configuration_or_index() {
    let mut current = view();
    current.preferred = SmartSearchMode::Claude;
    assert_eq!(choose(&current, Some(&status(3))), SmartAction::Claude);
    current.ai_enabled = false;
    assert_eq!(
        choose(&current, Some(&status(3))),
        SmartAction::ConfigureClaude
    );
    current.preferred = SmartSearchMode::Semantic;
    assert_eq!(choose(&current, Some(&status(0))), SmartAction::BuildIndex);
    current.embedding_enabled = false;
    assert_eq!(choose(&current, None), SmartAction::ConfigureSemantic);
}

#[test]
fn status_coalesces_bursts_and_discards_snapshots_before_content_or_profile_changes() {
    let mut cache = StatusCache::default();
    let first = request_id(cache.request("p").unwrap());
    for _ in 0..20 {
        cache.mark_dirty();
        assert!(cache.request("p").is_none());
    }
    assert!(!cache.complete(first, status(9), "p"));
    assert!(cache.ready("p").is_none());
    let second = request_id(cache.request("p").unwrap());
    assert!(cache.complete(second, status(2), "p"));
    assert_eq!(cache.ready("p").unwrap().indexed, 2);
    for _ in 0..20 {
        assert!(cache.request("p").is_none());
    }
    let third = request_id(cache.request("new").unwrap());
    assert!(cache.ready("new").is_none());
    assert!(!cache.complete(second, status(200), "new"));
    assert!(!cache.complete(third, status(100), "new"));
    assert!(cache.request("new").is_some());
}

#[test]
fn failed_status_releases_only_its_actual_slot_and_waits_for_an_explicit_retry() {
    let mut cache = StatusCache::default();
    let id = request_id(cache.request("p").unwrap());
    assert!(!cache.fail(id + 1));
    assert!(cache.request("p").is_none());
    assert!(cache.fail(id));
    assert!(cache.ready("p").is_none());
    let retry = request_id(cache.request("p").unwrap());
    assert!(!cache.complete(id, status(30), "p"));
    assert!(cache.complete(retry, status(1), "p"));
}

#[test]
fn smart_preflight_never_starts_a_paid_call_after_any_view_change_or_cancel() {
    let original = view();
    let mut changed = Vec::new();
    let mut v = original.clone();
    v.query = "别的查询".into();
    changed.push(v);
    let mut v = original.clone();
    v.scope = Scope::Clipboard;
    changed.push(v);
    let mut v = original.clone();
    v.profile = "new".into();
    changed.push(v);
    let mut v = original.clone();
    v.preferred = SmartSearchMode::Claude;
    changed.push(v);
    let mut v = original.clone();
    v.ai_enabled = false;
    changed.push(v);
    for current in changed {
        let mut pending = SmartPreflight::default();
        pending.begin(original.clone());
        assert_eq!(pending.finish(&current, true, Some(&status(3))), None);
    }
    for active in [false, true] {
        let mut pending = SmartPreflight::default();
        pending.begin(original.clone());
        if active {
            pending.cancel();
        }
        assert_eq!(pending.finish(&original, active, Some(&status(3))), None);
    }
    let mut pending = SmartPreflight::default();
    pending.begin(original.clone());
    assert_eq!(pending.finish(&original, true, None), None);
    assert!(pending.busy());
    assert_eq!(
        pending.finish(&original, true, Some(&status(0))),
        Some(SmartAction::Claude)
    );
    assert!(!pending.busy());
}

fn pinned() -> ItemSummary {
    ItemSummary {
        id: "kept-text".into(),
        kind: ItemKind::Clipboard,
        title: "部署说明".into(),
        subtitle: "terminal · 命中片段".into(),
        alias: String::new(),
        tags: vec![],
        pinned: true,
        created_at_ms: 100,
        last_used_ms: 100,
        use_count: 0,
    }
}

#[test]
fn metadata_submit_normalizes_only_metadata_and_cancel_or_switched_target_emits_nothing() {
    let summary = pinned();
    let form = ClipboardMetadataForm::new(&summary).unwrap();
    let mut state = UiState::default();
    state.visible = true;
    state.replace_ids(vec![summary.id.clone(), "another".into()]);
    let target = state.menu_target();
    assert!(
        form.finish(None, state.menu_target_current(&target))
            .unwrap()
            .is_none()
    );
    let fields = vec![
        "  新名称  ".into(),
        "  quick  ".into(),
        "部署, 部署, PROD，prod".into(),
    ];
    let input = form
        .finish(Some(fields.clone()), state.menu_target_current(&target))
        .unwrap()
        .unwrap();
    assert_eq!(input.id, "kept-text");
    assert_eq!(input.title, "新名称");
    assert_eq!(input.alias, "quick");
    assert_eq!(input.tags, ["部署", "PROD"]);
    state.select(1);
    assert!(
        form.finish(Some(fields), state.menu_target_current(&target))
            .unwrap()
            .is_none()
    );
    let mut unpinned = summary.clone();
    unpinned.pinned = false;
    assert!(ClipboardMetadataForm::new(&unpinned).is_none());
}

#[test]
fn metadata_form_uses_engine_limits_and_rejects_controls() {
    let form = ClipboardMetadataForm::new(&pinned()).unwrap();
    for fields in [
        vec![" ".into(), "".into(), "".into()],
        vec!["名".repeat(161), "".into(), "".into()],
        vec!["名称".into(), "别".repeat(129), "".into()],
        vec!["名称".into(), "".into(), "标".repeat(65)],
        vec![
            "名称".into(),
            "".into(),
            (0..33)
                .map(|i| format!("t{i}"))
                .collect::<Vec<_>>()
                .join(","),
        ],
        vec!["名称".into(), "bad\nname".into(), "".into()],
        vec!["名称".into(), "".into(), "one,\n bad".into()],
    ] {
        assert!(form.finish(Some(fields), true).is_err());
    }
}

#[test]
fn explicit_claude_does_not_wait_for_embedding_status() {
    let mut current = view();
    current.preferred = SmartSearchMode::Claude;
    assert!(!current.needs_index_status());
    let mut preflight = SmartPreflight::default();
    preflight.begin(current.clone());
    assert_eq!(
        preflight.finish(&current, true, None),
        Some(SmartAction::Claude)
    );
    current.ai_enabled = false;
    preflight.begin(current.clone());
    assert_eq!(
        preflight.finish(&current, true, None),
        Some(SmartAction::ConfigureClaude)
    );
}

#[test]
fn hub_save_rejects_old_status_without_admitting_a_new_scan() {
    use quick_panel::settings_ui::{Group, Navigation, Page};
    let mut cache = StatusCache::default();
    let mut nav = Navigation::default();
    let old = request_id(
        cache
            .request_for_view("p", nav.allows_panel_query())
            .unwrap(),
    );
    nav.open(Page::Hub(Group::Paths));
    cache.mark_dirty();
    assert!(!cache.complete(old, status(7), "p"));
    assert!(
        cache
            .request_for_view("p", nav.allows_panel_query())
            .is_none()
    );
    assert!(cache.ready("p").is_none());
    nav.close();
    // Hub close emits no status command; this is a later existing explicit request.
    let next = request_id(
        cache
            .request_for_view("p", nav.allows_panel_query())
            .unwrap(),
    );
    assert!(cache.complete(next, status(4), "p"));
    assert!(cache.request_for_view("p", true).is_none());
}
#[test]
fn index_terminal_allows_one_profile_bound_followup_then_revokes_permission() {
    for ending in [
        "complete",
        "fail",
        "stale",
        "profile",
        "settings",
        "old_failure",
    ] {
        let mut cache = StatusCache::default();
        let old = request_id(cache.request_for_view("p", true).unwrap());
        cache.index_terminal("p");
        assert!(!cache.fail(old + 999)); // unrelated failure cannot consume its permit
        assert!(
            cache.request_for_view("p", false).is_none(),
            "old scan owns slot"
        );
        if ending == "old_failure" {
            assert!(cache.fail(old));
            assert!(cache.request_for_view("p", false).is_none());
            continue;
        }
        if ending == "profile" {
            assert!(!cache.complete(old, status(0), "new"));
            assert!(cache.request_for_view("new", false).is_none());
            assert!(cache.request_for_view("p", false).is_none());
            continue;
        }
        if ending == "settings" {
            cache.invalidate_settings();
        }
        assert!(!cache.complete(old, status(0), "p"));
        if ending == "settings" {
            assert!(cache.request_for_view("p", false).is_none());
            continue;
        }
        let terminal = request_id(
            cache
                .request_for_view("p", false)
                .expect("real terminal gets one followup"),
        );
        assert!(cache.request_for_view("p", false).is_none());
        match ending {
            "complete" => assert!(cache.complete(terminal, status(3), "p")),
            "fail" => assert!(cache.fail(terminal)),
            "stale" => {
                cache.mark_dirty();
                assert!(!cache.complete(terminal, status(2), "p"));
            }
            _ => unreachable!(),
        }
        cache.mark_dirty();
        assert!(
            cache.request_for_view("p", false).is_none(),
            "{ending} cannot leave an unlimited permit"
        );
    }
}
