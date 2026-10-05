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

#[test]
fn instance_show_request_is_blocked_during_modal_or_shutdown() {
    let mut ui = UiState::default();
    assert!(ui.can_show_panel());
    ui.modal_open = true;
    assert!(!ui.can_show_panel());
    ui.modal_open = false;
    ui.begin_exit();
    assert!(!ui.can_show_panel());
    ui.cancel_exit();
    assert!(ui.can_show_panel());
}

#[test]
fn repeated_launch_hidden_panel_starts_a_new_target_session() {
    let ui = UiState::default();
    assert!(ui.instance_show_updates_target());
    assert!(!ui.may_update_target(true));
}
#[test]
fn repeated_launch_visible_panel_preserves_the_existing_target() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert!(!ui.instance_show_updates_target());
}
#[test]
fn repeated_launch_modal_or_exiting_panel_never_updates_target() {
    let mut ui = UiState::default();
    ui.modal_open = true;
    assert!(!ui.instance_show_updates_target());
    ui.modal_open = false;
    ui.begin_exit();
    assert!(!ui.instance_show_updates_target());
}

#[test]
fn final_review_exit_errors_are_single_and_stale_errors_are_ignored() {
    let mut ui = UiState::default();
    assert!(!ui.exit_failed());
    ui.begin_exit();
    assert!(ui.exit_failed());
    assert!(!ui.exit_failed());
    ui.retry_exit();
    assert!(ui.exit_failed());
    ui.cancel_exit();
    assert!(!ui.exit_failed());
    assert_eq!(ui.lifecycle, Lifecycle::Running);
}

#[test]
fn final_review_paste_waits_for_release_and_runs_once() {
    let mut ui = UiState::default();
    ui.visible = true;
    assert!(ui.begin_action(7));
    assert!(!ui.finish_paste(7, true));
    assert!(ui.visible);
    assert!(ui.action_current(7));
    assert!(!ui.finish_paste(7, true));
    assert!(ui.finish_paste(7, false));
    assert!(!ui.finish_paste(7, false));
    assert!(ui.begin_action(8));
    assert!(ui.finish_paste(8, false)); // Loaded after release
}

#[test]
fn final_review_deferred_paste_cancels_on_every_interaction_boundary() {
    for boundary in 0..5 {
        let mut ui = UiState::default();
        ui.visible = true;
        ui.replace_ids(vec!["a".into(), "b".into()]);
        ui.begin_action(9);
        assert!(!ui.finish_paste(9, true));
        match boundary {
            0 => ui.hide(),
            1 => ui.target_changed(),
            2 => ui.select(1),
            3 => {
                ui.next_request();
            }
            _ => {
                ui.begin_exit();
            }
        }
        ui.visible = true;
        ui.cancel_exit();
        assert!(!ui.finish_paste(9, false));
    }
}

#[test]
fn result_menu_cannot_execute_after_function_is_replaced_by_saved_content() {
    let mut ui = UiState::default();
    ui.visible = true;
    ui.replace_ids(vec!["function:show-preview".into()]);
    let target = ui.menu_target();
    ui.modal_open = true;
    ui.replace_ids(vec!["clip:0".into()]);
    ui.modal_open = false;
    assert!(!ui.menu_target_current(&target));
}

#[test]
fn result_menu_requires_an_original_selection_and_the_same_interaction() {
    let mut ui = UiState::default();
    ui.visible = true;
    let empty = ui.menu_target();
    ui.replace_ids(vec!["a".into()]);
    assert!(!ui.menu_target_current(&empty));
    let selected = ui.menu_target();
    ui.replace_ids(vec!["a".into(), "b".into()]);
    assert!(ui.menu_target_current(&selected));
    ui.hide();
    ui.visible = true;
    assert!(!ui.menu_target_current(&selected));
}

#[test]
fn held_enter_cannot_activate_new_content_after_a_function_clears_the_query() {
    let mut ui = UiState::default();
    ui.visible = true;
    ui.replace_ids(vec!["function:pause".into()]);
    assert_eq!(ui.key_down(PanelKey::Enter, false), KeyAction::Activate);
    ui.replace_ids(vec!["clip:new".into()]);
    assert_eq!(ui.key_down(PanelKey::Enter, true), KeyAction::Ignore);
    assert_eq!(ui.key_down(PanelKey::Down, true), KeyAction::SelectNext);
    assert_eq!(ui.key_down(PanelKey::Enter, false), KeyAction::Activate);
    ui.composing = true;
    assert_eq!(ui.key_down(PanelKey::Enter, true), KeyAction::NativeInput);
}

#[test]
fn same_target_background_refresh_preserves_action_but_selection_change_revokes_it() {
    let mut state = UiState::default();
    state.visible = true;
    state.replace_ids(vec!["a".into(), "b".into()]);
    assert!(state.begin_action(77));
    let query = state.next_refresh_request();
    state.replace_ids(vec!["a".into(), "b".into()]);
    assert!(state.is_current(query));
    assert!(state.action_current(77));
    state.select(1);
    assert!(!state.action_current(77));
}

#[test]
fn removed_target_invalidates_action_before_selection_is_replaced() {
    let mut state = UiState::default();
    state.visible = true;
    state.replace_ids(vec!["a".into(), "b".into()]);
    assert!(state.begin_action(77));
    state.replace_ids(vec!["b".into()]);
    assert_eq!(state.selected_id(), Some("b"));
    assert!(!state.action_current(77));
}
