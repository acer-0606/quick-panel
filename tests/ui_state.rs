use quick_panel::ui_state::{KeyAction, Lifecycle, PanelKey, UiState};
#[test]
fn refreshed_results_preserve_selected_identity() {
    let mut ui = UiState::default();
    ui.replace_ids(vec!["a".into(), "b".into()]);
    ui.select(1);
    ui.replace_ids(vec!["new".into(), "a".into(), "b".into()]);
    assert_eq!(ui.selected_id(), Some("b"));
    assert_eq!(ui.selected_index(), Some(2));
}
#[test]
fn ime_composition_owns_navigation_enter_and_escape() {
    let mut ui = UiState::default();
    ui.composing = true;
    for key in [
        PanelKey::Enter,
        PanelKey::Escape,
        PanelKey::Up,
        PanelKey::Down,
    ] {
        assert_eq!(ui.key(key), KeyAction::NativeInput);
    }
}
#[test]
fn escape_hides_without_entering_shutdown() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert_eq!(ui.key(PanelKey::Escape), KeyAction::Hide);
    ui.hide();
    assert!(!ui.visible);
    assert_eq!(ui.lifecycle, Lifecycle::Running);
    assert!(ui.begin_exit());
    assert_eq!(ui.lifecycle, Lifecycle::Saving);
}
#[test]
fn exit_drains_all_accepted_jobs_before_requesting_final_save() {
    let mut ui = UiState::default();
    assert!(ui.accept_capture());
    assert!(ui.accept_capture());
    assert!(!ui.begin_exit());
    assert!(!ui.accept_capture());
    assert!(!ui.capture_completed());
    assert!(ui.capture_completed());
    assert_eq!(ui.lifecycle, Lifecycle::Saving);
    ui.exit_failed();
    assert_eq!(ui.lifecycle, Lifecycle::SaveFailed);
    ui.cancel_exit();
    assert_eq!(ui.lifecycle, Lifecycle::Running);
    assert!(ui.accept_capture());
}
#[test]
fn stale_search_results_are_discarded() {
    let mut ui = UiState::default();
    let old = ui.next_request();
    let latest = ui.next_request();
    assert!(!ui.is_current(old));
    assert!(ui.is_current(latest));
}

#[test]
fn hiding_invalidates_a_pending_paste_even_after_reopen() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert!(ui.begin_action(7));
    ui.hide();
    ui.visible = true;
    assert!(!ui.finish_action(7));
}
#[test]
fn exit_and_external_focus_change_cancel_pending_actions() {
    let mut exiting = UiState::default();
    exiting.visible = true;
    assert!(exiting.begin_action(10));
    exiting.begin_exit();
    assert!(!exiting.finish_action(10));
    exiting.cancel_exit();
    assert!(!exiting.finish_action(10));
    let mut switched = UiState::default();
    switched.visible = true;
    assert!(switched.begin_action(20));
    switched.target_changed();
    assert!(!switched.finish_action(20));
}
#[test]
fn modal_and_own_windows_never_replace_the_paste_target() {
    let mut ui = UiState::default();
    assert!(ui.may_update_target(false));
    assert!(!ui.may_update_target(true));
    ui.modal_open = true;
    assert!(!ui.may_update_target(false));
    assert!(!ui.begin_action(1));
}
#[test]
fn latest_action_runs_once_only_while_visible_and_running() {
    let mut ui = UiState::default();
    assert!(!ui.begin_action(1));
    ui.visible = true;
    assert!(ui.begin_action(1));
    assert!(ui.begin_action(2));
    assert!(!ui.finish_action(1));
    assert!(ui.finish_action(2));
    assert!(!ui.finish_action(2));
}

#[test]
fn cancelled_menu_losing_focus_hides_and_invalidates_pending_action() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert!(ui.begin_action(44));
    assert!(ui.menu_cancel_should_hide(false, false, false));
    if ui.menu_cancel_should_hide(false, false, false) {
        ui.hide();
    }
    assert!(!ui.visible);
    ui.visible = true;
    assert!(!ui.finish_action(44));
}
#[test]
fn cancelled_menu_keeps_panel_when_foreground_is_panel_or_owned_modal() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert!(!ui.menu_cancel_should_hide(false, true, false));
    assert!(!ui.menu_cancel_should_hide(false, false, true));
}
#[test]
fn menu_command_and_hidden_tray_do_not_trigger_cancellation_hide() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert!(ui.begin_action(55));
    assert!(!ui.menu_cancel_should_hide(true, false, false));
    assert!(ui.finish_action(55));
    ui.hide();
    assert!(!ui.menu_cancel_should_hide(false, false, false));
    assert!(!ui.visible);
}
