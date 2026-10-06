use quick_panel::{
    model::{ItemKind, ItemSummary},
    palette::{self, Entry},
};

fn path(id: &str, target: &str) -> Entry {
    Entry::Saved(ItemSummary {
        id: id.into(),
        kind: ItemKind::Folder,
        title: target.rsplit(['\\', '/']).next().unwrap().into(),
        subtitle: target.into(),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms: 1,
        last_used_ms: 1,
        use_count: 0,
    })
}

#[test]
fn different_middle_directories_lead_the_label_without_changing_identity() {
    let rows = vec![
        path("a", r"D:\workspace\客户甲\server\logs"),
        path("b", r"D:\workspace\客户乙\server\logs"),
    ];
    let original = rows.clone();
    let details = palette::row_details(&rows, "");
    assert_eq!(details[0].title, "客户甲 · server / logs");
    assert_eq!(details[1].title, "客户乙 · server / logs");
    assert!(details[0].text.starts_with("客户甲"));
    assert_eq!(rows, original);
}

#[test]
fn aliases_and_custom_titles_win_while_context_still_disambiguates() {
    let mut a = path("a", r"D:\workspace\甲\server\logs");
    if let Entry::Saved(item) = &mut a {
        item.alias = "甲方生产日志".into();
    }
    let mut b = path("b", r"D:\workspace\乙\server\logs");
    if let Entry::Saved(item) = &mut b {
        item.title = "乙方诊断".into();
    }
    let details = palette::row_details(&[a.clone(), b], "");
    assert_eq!(a.title(), "甲方生产日志");
    assert_eq!(details[0].title, "甲方生产日志");
    assert!(details[0].text.starts_with('甲'));
    assert_eq!(details[1].title, "乙方诊断");
    assert!(details[1].text.starts_with('乙'));
}

#[test]
fn roots_and_unc_servers_remain_distinct_without_host_path_semantics() {
    let rows = vec![
        path("c", r"C:\work\logs"),
        path("d", "D:/work/logs"),
        path("s1", r"\\server甲\share\work\logs"),
        path("s2", r"\\server乙\share\work\logs"),
    ];
    let details = palette::row_details(&rows, "");
    assert!(details[0].title.starts_with("C:"));
    assert!(details[1].title.starts_with("D:"));
    assert!(details[2].title.starts_with("server甲"));
    assert!(details[3].title.starts_with("server乙"));
}

#[test]
fn long_differentiating_names_keep_the_different_part_near_the_front() {
    let common = "很长的共同目录前缀".repeat(12);
    let rows = vec![
        path("a", &format!("D:\\work\\{common}客户甲\\server\\logs")),
        path("b", &format!("D:\\work\\{common}客户乙\\server\\logs")),
    ];
    let details = palette::row_details(&rows, "");
    assert!(details[0].title.contains("客户甲"));
    assert!(details[1].title.contains("客户乙"));
    assert!(details[0].title.chars().count() <= 64);
    assert!(details[0].title.chars().position(|c| c == '甲').unwrap() < 24);
}

#[test]
fn matching_directory_is_visible_even_when_only_one_result_remains() {
    let target = format!("D:\\{}\\客户甲\\server\\logs", "共同目录\\".repeat(20));
    let detail = palette::row_details(&[path("a", &target)], "客户甲").remove(0);
    assert!(detail.text.starts_with("客户甲"));
    assert!(detail.text.chars().count() <= 120);
}

#[test]
fn observed_paths_and_saved_paths_share_the_same_display_logic() {
    let suggestion = quick_panel::path_activity::PathSuggestion {
        key: r"d:\work\乙\server\logs".into(),
        path: r"D:\work\乙\server\logs".into(),
        title: "logs".into(),
        kind: ItemKind::Folder,
        alias: String::new(),
        tags: vec![],
        visits: 4,
        last_seen_ms: 99,
    };
    let rows = vec![
        path("saved", r"D:\work\甲\server\logs"),
        Entry::Suggested(suggestion.into()),
    ];
    let details = palette::row_details(&rows, "");
    assert_eq!(details[0].title, "甲 · server / logs");
    assert_eq!(details[1].title, "乙 · server / logs");
    assert!(details[1].text.starts_with('乙'));
    assert!(details[1].text.contains("外部观察"));
    assert_eq!(
        rows[1].suggestion_action().unwrap().target.as_deref(),
        Some(r"D:\work\乙\server\logs")
    );
}

#[test]
fn observed_alias_wins_disambiguation_and_explicit_action_preserves_tags_and_target() {
    let suggestion = quick_panel::path_activity::PathSuggestion {
        key: "c:\\project\\logs".into(),
        path: "C:\\Project\\Logs".into(),
        title: "Logs".into(),
        alias: "项目运行记录".into(),
        tags: vec!["运行".into()],
        kind: ItemKind::Folder,
        visits: 1,
        last_seen_ms: 99,
    };
    let rows = vec![
        Entry::Suggested(suggestion.into()),
        path("other", "C:\\Other\\Logs"),
    ];
    assert_eq!(rows[0].title(), "项目运行记录");
    assert_eq!(palette::row_details(&rows, "")[0].title, "项目运行记录");
    let action = rows[0].suggestion_action().unwrap();
    assert_eq!(action.summary.alias, "项目运行记录");
    assert_eq!(action.summary.tags, vec!["运行"]);
    assert_eq!(action.target.as_deref(), Some("C:\\Project\\Logs"));
}
