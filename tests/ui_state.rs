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
