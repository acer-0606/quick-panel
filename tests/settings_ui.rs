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
