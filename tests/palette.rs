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
