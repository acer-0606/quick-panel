use quick_panel::{
    model::*,
    palette::{self, Context, Entry, Function},
    ui_state::UiState,
};

fn saved(id: &str, title: &str, kind: ItemKind) -> ItemSummary {
    ItemSummary {
        id: id.into(),
        title: title.into(),
        kind,
        subtitle: "本地保存内容".into(),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms: 1,
        last_used_ms: 1,
        use_count: 0,
    }
}

#[test]
fn functional_actions_are_found_by_names_synonyms_pinyin_and_initials() {
    for (query, action) in [
        ("设置", Function::Settings),
        ("快捷键", Function::Settings),
        ("shezhi", Function::Settings),
        ("sz", Function::Settings),
        ("添加文件夹", Function::AddFolder),
        ("tjwjj", Function::AddFolder),
        ("添加文档", Function::AddDocument),
        ("暂停记录", Function::PauseCapture),
        ("预览", Function::ShowPreview),
        ("剪贴板", Function::Clipboard),
        ("文件夹", Function::Folders),
        ("文档", Function::Documents),
        ("全部", Function::All),
        ("退出", Function::Exit),
    ] {
        let entries = palette::results(query, vec![], Context::default());
        assert!(
            entries.iter().any(|entry| entry.function() == Some(action)),
            "missing {action:?} for {query}"
        );
    }
    assert!(palette::results("没有这个功能xyz", vec![], Context::default()).is_empty());
}

#[test]
fn stored_and_function_results_coexist_with_distinct_identity_and_kind() {
    let contents = vec![saved("saved-settings", "设置操作说明", ItemKind::Clipboard)];
    let entries = palette::results("设置", contents, Context::default());
    assert_eq!(entries[0].function(), Some(Function::Settings));
    assert_eq!(entries[0].kind_label(), "⚙ 功能");
    let content = entries
        .iter()
        .find(|entry| entry.id() == "saved-settings")
        .unwrap();
    assert!(matches!(content, Entry::Saved(_)));
    assert_eq!(content.kind_label(), "▤ 剪贴板");
    assert!(content.function().is_none());
}

#[test]
fn exact_saved_titles_outrank_incidental_function_keyword_hits() {
    let entries = palette::results(
        "文档",
        vec![saved("doc", "文档", ItemKind::Document)],
        Context::default(),
    );
    assert_eq!(entries[0].id(), "doc");
    assert!(
        entries
            .iter()
            .any(|entry| entry.function() == Some(Function::Documents))
    );
}

#[test]
fn stateful_commands_offer_only_the_valid_next_action() {
    let paused = Context {
        capture_paused: true,
        preview_open: true,
        ..Context::default()
    };
    let entries = palette::results("记录", vec![], paused);
    assert!(
        entries
            .iter()
            .any(|entry| entry.function() == Some(Function::ResumeCapture))
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.function() == Some(Function::PauseCapture))
    );
    let preview = palette::results("预览", vec![], paused);
    assert_eq!(preview[0].function(), Some(Function::HidePreview));
}

#[test]
fn empty_search_preserves_recent_contents_and_new_install_has_starter_actions() {
    let entries = palette::results(
        "",
        vec![
            saved("older", "上一次选择", ItemKind::Document),
            saved("newer", "另一个结果", ItemKind::Folder),
        ],
        Context::default(),
    );
    assert_eq!(
        entries.iter().map(Entry::id).collect::<Vec<_>>(),
        ["older", "newer"]
    );
    let starters = palette::results("", vec![], Context::default());
    assert_eq!(
        starters
            .iter()
            .filter_map(Entry::function)
            .collect::<Vec<_>>(),
        [
            Function::AddFolder,
            Function::AddDocument,
            Function::Settings
        ]
    );
}

#[test]
fn background_content_refresh_keeps_the_selected_function_identity() {
    let initial = palette::results("设置", vec![], Context::default());
    let mut state = UiState::default();
    state.replace_ids(initial.iter().map(|entry| entry.id().to_owned()).collect());
    state.select(
        initial
            .iter()
            .position(|entry| entry.function() == Some(Function::Settings))
            .unwrap(),
    );
    let selected = state.selected_id().unwrap().to_owned();
    let refreshed = palette::results(
        "设置",
        vec![saved("exact", "设置", ItemKind::Clipboard)],
        Context::default(),
    );
    state.replace_ids(
        refreshed
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect(),
    );
    assert_eq!(state.selected_id(), Some(selected.as_str()));
}

#[test]
fn combined_results_stay_bounded_without_hiding_an_exact_function() {
    let contents = (0..100)
        .map(|i| saved(&format!("item-{i}"), "设置说明", ItemKind::Clipboard))
        .collect();
    let entries = palette::results("设置", contents, Context::default());
    assert!(entries.len() <= 50);
    assert_eq!(entries[0].function(), Some(Function::Settings));
}

#[test]
fn exact_function_cannot_be_displaced_by_pinned_content_with_the_same_name() {
    let contents = (0..60)
        .map(|index| {
            let mut item = saved(&format!("pinned-{index}"), "设置", ItemKind::Document);
            item.alias = "设置".into();
            item.pinned = true;
            item.use_count = 10_000;
            item
        })
        .collect();
    let entries = palette::results("设置", contents, Context::default());
    assert_eq!(entries[0].function(), Some(Function::Settings));
    assert!(entries.iter().any(|entry| entry.id() == "pinned-0"));
}

#[test]
fn empty_loading_query_waits_for_contents_before_showing_starter_functions() {
    let loading = Context {
        content_ready: false,
        ..Context::default()
    };
    assert!(palette::results("", vec![], loading).is_empty());
    let functions = palette::results("快捷键", vec![], loading);
    assert_eq!(functions[0].function(), Some(Function::Settings));
    let ready = palette::results("", vec![], Context::default());
    assert_eq!(ready[0].function(), Some(Function::AddFolder));
}

#[test]
fn ai_suggestions_resolve_only_existing_functions_and_keep_real_saved_rows() {
    use quick_panel::ai::AiPlan;
    let plan = AiPlan {
        functions: vec!["function:pause".into(), "function:invented".into()],
        ..AiPlan::default()
    };
    let rows = palette::ai_results(
        &plan,
        vec![saved("real", "实际内容", ItemKind::Clipboard)],
        Context::default(),
    );
    assert_eq!(rows[0].function(), Some(Function::PauseCapture));
    assert_eq!(rows[1].id(), "real");
    assert_eq!(rows.len(), 2);
    let catalog = palette::ai_catalog(Context::default());
    assert!(
        catalog
            .iter()
            .any(|function| function.id == "function:settings")
    );
    assert!(
        !catalog
            .iter()
            .any(|function| function.id == "function:ai-search")
    );
}

#[test]
fn disabled_ai_hides_search_action_but_configuration_remains_searchable() {
    assert!(
        !palette::results("AI检索", vec![], Context::default())
            .iter()
            .any(|row| row.function() == Some(Function::AiSearch))
    );
    assert!(
        palette::results("AI设置", vec![], Context::default())
            .iter()
            .any(|row| row.function() == Some(Function::AiSettings))
    );
    let enabled = Context {
        ai_enabled: true,
        ..Context::default()
    };
    assert!(
        palette::results("按意思查找", vec![], enabled)
            .iter()
            .any(|row| row.function() == Some(Function::AiSearch))
    );
}

#[test]
fn embedding_actions_are_searchable_only_when_available() {
    let disabled = Context::default();
    assert!(
        palette::results("语义检索设置", vec![], disabled)
            .iter()
            .any(|e| e.function() == Some(Function::EmbeddingSettings))
    );
    for (query, action) in [
        ("语义查找", Function::SemanticSearch),
        ("更新语义索引", Function::UpdateSemanticIndex),
        ("取消索引", Function::CancelSemanticIndex),
    ] {
        assert!(
            !palette::results(query, vec![], disabled)
                .iter()
                .any(|e| e.function() == Some(action))
        );
        let enabled = Context {
            embedding_enabled: true,
            indexing: true,
            ..disabled
        };
        assert!(
            palette::results(query, vec![], enabled)
                .iter()
                .any(|e| e.function() == Some(action))
        );
    }
    assert!(
        !palette::results(
            "取消索引",
            vec![],
            Context {
                embedding_enabled: true,
                ..disabled
            }
        )
        .iter()
        .any(|e| e.function() == Some(Function::CancelSemanticIndex))
    );
}

#[test]
fn semantic_saved_order_survives_exact_function_coexistence() {
    let entries = palette::semantic_results(
        "设置",
        vec![
            saved("cosine-first", "网络资料", ItemKind::Clipboard),
            saved("lexical-later", "设置", ItemKind::Clipboard),
        ],
        Context::default(),
    );
    assert_eq!(entries[0].function(), Some(Function::Settings));
    assert_eq!(
        entries
            .iter()
            .filter_map(|e| e.saved().map(|s| s.id.as_str()))
            .collect::<Vec<_>>(),
        ["cosine-first", "lexical-later"]
    );
}

#[test]
fn adopted_semantic_result_menu_preserves_real_row_identity_and_engine_order() {
    use quick_panel::{
        model::Scope,
        semantic_ui::{QuerySession, prepare_semantic_effect},
    };
    let mut session = QuerySession::default();
    session.begin(1, "设置", Scope::All, "p");
    session.finish(1, "设置", Scope::All, "p", true, vec![1., 0.]);
    let mut saved_rows = vec![
        saved("semantic-first", "网络说明", ItemKind::Clipboard),
        saved("lexical-next", "设置", ItemKind::Clipboard),
    ];
    let rows = palette::semantic_results("设置", saved_rows.clone(), Context::default());
    let mut state = UiState::default();
    state.visible = true;
    state.replace_ids(rows.iter().map(|row| row.id().to_owned()).collect());
    state.select(
        rows.iter()
            .position(|row| row.id() == "semantic-first")
            .unwrap(),
    );
    assert_eq!(state.selected_id(), Some("semantic-first"));
    let target = state.menu_target();
    let mut ready = true;
    prepare_semantic_effect(&mut session, &mut state, &mut saved_rows, &mut ready, true);
    let rows = palette::semantic_results("设置", saved_rows, Context::default());
    assert_eq!(
        rows.iter()
            .filter_map(|row| row.saved().map(|saved| saved.id.as_str()))
            .collect::<Vec<_>>(),
        ["semantic-first", "lexical-next"]
    );
    assert!(state.menu_target_current(&target));
    assert!(session.adopted());
    assert!(ready);
}

#[test]
fn unified_smart_entry_remains_reachable_without_configured_capabilities() {
    let entries = palette::results("智能查找", vec![], Context::default());
    assert!(
        entries
            .iter()
            .any(|e| e.function() == Some(Function::SmartSearch))
    );
}
#[test]
fn row_detail_prioritizes_source_excerpt_and_keeps_long_alias_out_of_title() {
    let mut summary = saved("metadata", "核心标题", ItemKind::Clipboard);
    summary.subtitle = "terminal · 命中部署片段".into();
    summary.alias = "长别名".repeat(50);
    summary.tags = vec!["长标签".repeat(100)];
    summary.created_at_ms = 1728117900000;
    let row = Entry::Saved(summary);
    assert!(row.subtitle().starts_with("terminal · 命中部署片段"));
    assert!(row.subtitle().chars().count() < 80);
    assert_eq!(row.title(), "核心标题");
    assert_eq!(row.saved().unwrap().created_at_ms, 1728117900000);
}

#[test]
fn row_detail_keeps_body_hit_near_the_start_and_exposes_saved_time_without_loading_body() {
    let mut summary = saved("hit", "保存标题", ItemKind::Clipboard);
    summary.subtitle = format!(
        "very-long-source-application.exe · {}部署目标是在周末",
        "上下文".repeat(12)
    );
    summary.created_at_ms = 1728117900000;
    let row = Entry::Saved(summary.clone());
    let detail = row.detail("部署");
    assert!(detail.text.starts_with("very-long-so… · …"));
    assert!(detail.text.chars().position(|c| c == '部').unwrap() < 24);
    assert!(detail.text.contains("部署目标是在周末"));
    assert_eq!(detail.saved_at_ms, Some(1728117900000));
    assert_eq!(row.saved().unwrap(), &summary);
    let functions = palette::results("设置", vec![], Context::default());
    assert_eq!(functions[0].detail("设置").saved_at_ms, None);
}

fn suggested(number: usize) -> quick_panel::path_activity::PathSuggestion {
    quick_panel::path_activity::PathSuggestion {
        key: format!("c:\\path{number}"),
        path: format!("C:\\Path{number}"),
        title: format!("Path{number}"),
        kind: ItemKind::Folder,
        visits: 7,
        last_seen_ms: 999,
    }
}
#[test]
fn automatic_rows_have_independent_identity_actions_and_visit_time() {
    let entry = Entry::Suggested(suggested(1).into());
    assert!(entry.id().starts_with("path-suggestion:"));
    assert_eq!(entry.id(), Entry::Suggested(suggested(1).into()).id());
    assert!(entry.saved().is_none());
    assert!(!entry.pinned());
    assert_eq!(entry.suggested().unwrap().path, "C:\\Path1");
    assert_eq!(entry.detail("").saved_at_ms, None);
    assert_eq!(entry.detail("").visited_at_ms, Some(999));
    assert!(entry.subtitle().contains("外部观察"));
    let action = entry.suggestion_action().unwrap();
    assert_eq!(action.target.as_deref(), Some("C:\\Path1"));
    assert!(action.representations.is_empty());
}
#[test]
fn suggestions_merge_after_manual_matches_deduplicate_and_obey_bounds() {
    let mut favorite = saved("manual", "Name", ItemKind::Folder);
    favorite.subtitle = "c:/PATH1".into();
    favorite.pinned = true;
    let rows = palette::with_suggestions(
        "",
        vec![Entry::Saved(favorite)],
        (0..20).map(suggested).collect(),
    );
    assert_eq!(rows[0].id(), "manual");
    assert_eq!(
        rows.iter().filter(|row| row.suggested().is_some()).count(),
        5
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.suggested().is_some_and(|path| path.key == "c:\\path1"))
    );
    let rows = palette::with_suggestions("Path", vec![], (0..20).map(suggested).collect());
    assert_eq!(rows.len(), 10);
    let rows = palette::with_suggestions(
        "Path",
        (0..49)
            .map(|i| Entry::Saved(saved(&format!("s{i}"), "Path", ItemKind::Folder)))
            .collect(),
        (0..20).map(suggested).collect(),
    );
    assert_eq!(rows.len(), 50);
    assert_eq!(rows[49].suggested().unwrap().key, "c:\\path0");
}

#[test]
fn external_path_settings_are_reachable_before_any_saved_favorite() {
    let rows = palette::results("常用推荐设置", vec![], Context::default());
    assert!(
        rows.iter()
            .any(|row| row.id() == "function:path-activity-settings")
    );
}
