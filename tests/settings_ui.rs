use quick_panel::form_validation::{Validation, validate};

#[test]
fn general_form_owns_all_three_hotkeys_and_no_data_or_search_fields() {
    let fields = ["0", "Ctrl+Space", "Ctrl+Alt+V", "Ctrl+Alt+S"].map(str::to_owned);
    assert!(validate(&Validation::General, &fields, &[]).is_ok());
    let mut conflict = fields;
    conflict[3] = conflict[2].clone();
    assert_eq!(
        validate(&Validation::General, &conflict, &[])
            .unwrap_err()
            .field,
        3
    );
}

use quick_panel::{
    model::Settings,
    settings_ui::{self, Action, Entry, Group},
};
#[test]
fn all_settings_entries_share_hub_and_operations_are_not_groups() {
    for entry in [Entry::Main, Entry::Tray, Entry::Search] {
        assert_eq!(settings_ui::entry_group(entry), Group::General);
    }
    assert_eq!(Group::ALL.len(), 6);
    assert_eq!(
        settings_ui::function_group(quick_panel::palette::Function::Settings),
        Some(Group::General)
    );
    assert_eq!(
        settings_ui::function_group(quick_panel::palette::Function::Screenshot),
        None
    );
    assert_eq!(
        settings_ui::function_group(quick_panel::palette::Function::OrganizerReview),
        None
    );
}
#[test]
fn retention_rejects_invalid_preserves_unedited_legacy_and_merges_current() {
    let saved = Settings::default();
    for raw in ["", "0", "-1", "abc", "36501", "9999999999999999999999999"] {
        let v = ["0", "", raw, "10000"].map(str::to_owned);
        assert_eq!(
            settings_ui::edit(&saved, Action::Data, &v)
                .unwrap_err()
                .field,
            2
        );
    }
    for raw in ["0", "1000001", "9999999999999999999999999"] {
        assert_eq!(
            settings_ui::edit(
                &saved,
                Action::Data,
                &["0", "", "30", raw].map(str::to_owned)
            )
            .unwrap_err()
            .field,
            3
        );
    }
    for (days, limit) in [("1", "1"), ("36500", "1000000")] {
        let next = settings_ui::edit(
            &saved,
            Action::Data,
            &["0", "", days, limit].map(str::to_owned),
        )
        .unwrap();
        assert_eq!(next.retention_days.to_string(), days);
        assert_eq!(next.max_history.to_string(), limit);
    }
    let legacy = Settings {
        retention_days: 0,
        max_history: usize::MAX,
        ..saved
    };
    let next = settings_ui::edit(
        &legacy,
        Action::Data,
        &[
            "1".into(),
            "secret.exe".into(),
            "0".into(),
            usize::MAX.to_string(),
        ],
    )
    .unwrap();
    assert_eq!(next.retention_days, 0);
    assert_eq!(next.max_history, usize::MAX);
    assert!(next.capture_paused);
    let general = settings_ui::edit(
        &next,
        Action::General,
        &["2", "Ctrl+Space", "", ""].map(str::to_owned),
    )
    .unwrap();
    assert!(general.capture_paused);
    assert_eq!(general.max_history, usize::MAX);
}
#[test]
fn disabled_states_keep_configuration_and_unknown_index_honest() {
    let s = Settings::default();
    assert!(!s.ai.enabled && !s.embedding.enabled);
    assert!(!Group::Services.actions().is_empty());
    let text = settings_ui::description(Group::Smart, &s, "尚未加载当前配置的索引状态", None);
    assert!(text.contains("请启用"));
    assert!(text.contains("尚未加载"));
    assert!(!text.contains("0 条"));
    assert!(settings_ui::description(Group::Organizer, &s, "", None).contains("仍可管理已有"));
    assert!(settings_ui::description(Group::Services, &s, "", None).contains("不代表连接成功"));
}
#[test]
fn native_geometry_keeps_navigation_and_actions_reachable() {
    for dpi in [96, 144, 192] {
        for (w, h) in [(360, 480), (800, 600)] {
            let layout = settings_ui::layout(w, h, dpi);
            for r in [
                layout.navigation,
                layout.detail,
                layout.action,
                layout.open,
                layout.close,
            ] {
                assert!(
                    r.x >= 0 && r.y >= 0 && r.width > 0 && r.height > 0,
                    "{layout:?}"
                );
                assert!(r.x + r.width <= w && r.y + r.height <= h, "{layout:?}");
            }
            assert!(layout.detail.y + layout.detail.height <= layout.action.y);
            assert!(layout.action.y + layout.action.height <= layout.open.y);
            if w == 360 {
                assert!(layout.top_navigation);
            }
        }
    }
}

#[test]
fn smart_switches_do_not_disable_independent_organizer_and_reject_missing_connections() {
    let mut saved = Settings::default();
    saved.organizer.enabled = true;
    let next = settings_ui::edit(
        &saved,
        Action::Smart,
        &["0", "0", "2", "1"].map(str::to_owned),
    )
    .unwrap();
    assert!(next.organizer.enabled);
    assert_eq!(
        next.smart_search,
        quick_panel::model::SmartSearchMode::Semantic
    );
    assert_eq!(
        next.embedding.index_scope,
        quick_panel::semantic::IndexScope::AllText
    );
    for (claude, semantic, field) in [("1", "0", 0), ("0", "1", 1)] {
        assert_eq!(
            settings_ui::edit(
                &saved,
                Action::Smart,
                &[claude, semantic, "0", "0"].map(str::to_owned)
            )
            .unwrap_err()
            .field,
            field
        );
    }
    assert!(!saved.ai.enabled && !saved.embedding.enabled);
}

#[test]
fn path_leaf_submission_emits_only_explicit_save_or_clear_never_search() {
    use quick_panel::model::Command;
    for (enabled, clear) in [("0", "0"), ("1", "0"), ("0", "1")] {
        let current = Settings::default();
        let submission =
            settings_ui::submit_paths(&current, &[enabled, "1", "", clear].map(str::to_owned))
                .unwrap();
        assert!(
            !submission
                .commands
                .iter()
                .any(|c| matches!(c, Command::Search(_))),
            "path leaf must not restore ordinary search"
        );
        assert_eq!(
            submission
                .commands
                .iter()
                .filter(|c| matches!(c, Command::SaveSettings(_)))
                .count(),
            usize::from(enabled == "1")
        );
        assert_eq!(
            submission
                .commands
                .iter()
                .filter(|c| matches!(c, Command::ClearPathActivity))
                .count(),
            usize::from(clear == "1")
        );
    }
}
#[test]
fn navigation_keeps_both_management_sources_across_cross_links_and_nested_settings() {
    use settings_ui::{Navigation, Page};
    for (source, other, label) in [
        (Page::Memory, Page::Review, "返回记忆"),
        (Page::Review, Page::Memory, "返回建议"),
    ] {
        let mut nav = Navigation::default();
        nav.open(source);
        nav.open(Page::Hub(Group::Organizer));
        nav.select_group(Group::Services);
        assert_eq!(nav.back_label(), label);
        nav.open(other);
        // The management page may open settings again; return goes to that page
        // first and then to the original hub and source, with its selected group.
        nav.open(Page::Hub(Group::Organizer));
        nav.select_group(Group::General);
        assert_eq!(nav.close(), Some(other));
        assert_eq!(nav.close(), Some(Page::Hub(Group::Services)));
        assert_eq!(nav.back_label(), label);
        assert_eq!(nav.close(), Some(source));
        assert_eq!(nav.close(), None);
    }
}

#[test]
fn native_editor_reflows_long_fields_notes_and_status_from_wide_96_to_narrow_192() {
    use settings_ui::{EditorField, EditorFieldKind, EditorSpec};
    let spec = EditorSpec {
        fields: vec![
            EditorField {
                label: "服务隐藏能力开关".into(),
                kind: EditorFieldKind::Hidden,
            },
            EditorField {
                label: "未固定历史：多少天未使用后清理（1–36500）".into(),
                kind: EditorFieldKind::Line,
            },
            EditorField {
                label: "整理记忆的使用规则与记录范围".into(),
                kind: EditorFieldKind::Switch("启用整理记忆（使用已有规则并记录新的反馈）".into()),
            },
            EditorField {
                label: "整理说明（保留原始多行输入与中文输入法草稿）".into(),
                kind: EditorFieldKind::Multiline,
            },
        ],
        note: Some(settings_ui::RETENTION_NOTE.into()),
        test_label: Some("测试连接".into()),
        models: true,
    };
    let wide = spec.layout(800, 96);
    let narrow = spec.layout(360, 192);
    assert!(
        narrow.fields[1].label.height > wide.fields[1].label.height * 2,
        "changed line count must be recomputed"
    );
    assert!(
        narrow.fields[2].control.height > wide.fields[2].control.height * 2,
        "checkbox text must rewrap"
    );
    let mut bottom = 0;
    for field in narrow.fields.iter().filter(|f| !f.hidden) {
        for r in [field.label, field.control] {
            assert!(r.x >= 0 && r.x + r.width <= 360);
            assert!(r.y >= bottom);
            bottom = r.y + r.height;
        }
    }
    for r in [narrow.note, narrow.test_button, narrow.test_result]
        .into_iter()
        .flatten()
    {
        assert!(r.x + r.width <= 360);
        assert!(r.y >= bottom);
        bottom = r.y + r.height;
    }
    assert!(narrow.content_height >= bottom);
    let note = narrow.note.unwrap();
    assert!(
        note.height >= settings_ui::wrapped_height(settings_ui::RETENTION_NOTE, note.width, 192)
    );
    let model = narrow.model_button.unwrap();
    let test = narrow.test_button.unwrap();
    assert!(model.x >= test.x + test.width);
    assert!(model.x + model.width <= 360);
}

#[test]
fn path_save_and_clear_notifications_cannot_refresh_panel_inside_navigation_chain() {
    use settings_ui::{Navigation, Page};
    let mut nav = Navigation::default();
    nav.open(Page::Hub(Group::Paths));
    let submission = settings_ui::submit_paths(
        &Settings::default(),
        &["1", "1", "", "1"].map(str::to_owned),
    )
    .unwrap();
    assert_eq!(submission.commands.len(), 2);
    // The native Search entry and Changed/PathActivityChanged/Saved handlers all
    // consume this gate, including events emitted asynchronously by these commands.
    assert!(!nav.allows_panel_query());
    nav.open(Page::Review);
    nav.open(Page::Hub(Group::Services));
    assert!(!nav.allows_panel_query());
    nav.close();
    assert!(!nav.allows_panel_query());
    nav.close();
    assert!(!nav.allows_panel_query());
    nav.close();
    assert!(nav.allows_panel_query());
    nav.open(Page::Memory);
    nav.open(Page::Hub(Group::Organizer));
    nav.clear();
    assert_eq!(nav.current(), None);
    assert!(nav.allows_panel_query());
}
#[test]
fn editor_dpi_scroll_keeps_focused_control_reachable_and_can_reveal_last_note() {
    use settings_ui::{EditorField, EditorFieldKind, EditorSpec};
    let spec = EditorSpec {
        fields: (0..8)
            .map(|_| EditorField {
                label: "跨屏重新排版的完整中文字段标签不能裁掉任何文字".into(),
                kind: EditorFieldKind::Switch("使用已有规则并记录新的反馈".into()),
            })
            .collect(),
        note: Some(settings_ui::RETENTION_NOTE.into()),
        test_label: None,
        models: false,
    };
    for (width, dpi, height) in [(360, 192, 260), (800, 96, 450), (360, 144, 280)] {
        let layout = spec.layout(width, dpi);
        for i in [0, 3, 7] {
            let scroll = layout.scroll_after_reflow(300, 96, dpi, height, Some(i));
            let r = layout.fields[i].control;
            assert!(r.y >= scroll && r.y + r.height <= scroll + height);
        }
        let bottom = layout.scroll_after_reflow(i32::MAX / 4, 96, dpi, height, None);
        let note = layout.note.unwrap();
        assert!(note.y + note.height <= bottom + height);
        assert!(bottom > 0);
    }
}
