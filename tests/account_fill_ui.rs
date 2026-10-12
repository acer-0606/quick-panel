use quick_panel::{
    account_fill::WindowIdentity,
    account_fill_ui::{Access, AccountFlow},
    account_vault::{Secret, VaultDocument},
    account_vault_jobs::VaultNotice,
    ai::AiPlan,
    model::Event,
    palette::{self, Context},
};
#[test]
fn access_and_read_receipts_cannot_install_a_cancelled_session() {
    let mut ui = AccountFlow::new();
    assert_eq!(ui.access(false), Access::Disabled);
    assert_eq!(ui.access(true), Access::Reading);
    let old = ui.read_request();
    ui.dismiss();
    assert!(!ui.loaded(old, Ok(None)));
    let id = ui.read_request();
    assert!(ui.loaded(id, Ok(None)));
    assert_eq!(ui.access(true), Access::Uninitialized);
    ui.error = Some("合成故障".into());
    assert_eq!(ui.access(true), Access::Failed);
}
#[test]
fn own_panel_preserves_target_but_dismiss_revokes_it() {
    let mut ui = AccountFlow::new();
    let target = WindowIdentity {
        handle: 9,
        pid: 7,
        process_start: 88,
    };
    ui.capture(Some(target), false);
    ui.capture(None, true);
    assert_eq!(ui.target, Some(target));
    ui.dismiss();
    assert!(ui.target.is_none());
}
#[test]
fn accepted_preparation_blocks_every_shutdown_until_correlated_receipt_and_cannot_unlock_on_exit() {
    let mut ui = AccountFlow::new();
    let id = ui.next_id();
    let password = Secret::new("synthetic-master-123".into());
    ui.vault
        .accept_create(id, password.clone(), password, VaultDocument::default())
        .unwrap();
    ui.request_exit();
    assert!(!ui.shutdown_ready());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if matches!(ui.vault.poll(), Some(VaultNotice::Prepared)) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    ui.vault.dispatch_save(|_| Ok(())).unwrap();
    assert!(!ui.shutdown_ready());
    assert!(
        ui.vault
            .handle_event(&Event::AccountVaultWritten {
                request_id: id + 1,
                result: Ok(1)
            })
            .is_none()
    );
    assert!(!ui.shutdown_ready());
    ui.vault.handle_event(&Event::AccountVaultWritten {
        request_id: id,
        result: Ok(1),
    });
    assert!(ui.shutdown_ready());
    assert!(!ui.vault.session().is_unlocked());
}
#[test]
fn account_commands_are_searchable_but_never_ai_executable() {
    let rows = palette::results("账号", vec![], Context::default());
    assert!(rows.iter().any(|r| r.title() == "账号填充"));
    assert!(
        !palette::ai_catalog(Context::default())
            .iter()
            .any(|r| r.id.contains("account"))
    );
    let plan = AiPlan {
        functions: vec![
            "function:account-fill".into(),
            "function:account-manage".into(),
            "function:account-lock".into(),
        ],
        ..Default::default()
    };
    assert!(palette::ai_results(&plan, vec![], Context::default()).is_empty());
}
#[test]
fn async_form_only_finishes_its_accepted_operation_and_keeps_failed_draft_open() {
    use quick_panel::account_fill_ui::{FormProgress, form_progress};
    assert_eq!(
        form_progress(4, 4, 8, 8, true, Some("合成保存失败")),
        FormProgress::Failed("合成保存失败".into())
    );
    assert_eq!(
        form_progress(4, 4, 8, 9, false, None),
        FormProgress::Pending
    );
    assert_eq!(
        form_progress(4, 4, 8, 9, true, None),
        FormProgress::Complete
    );
    assert_eq!(
        form_progress(4, 5, 8, 9, true, None),
        FormProgress::Cancelled
    );
}

fn settle_create(ui: &mut AccountFlow) {
    let password = Secret::new("synthetic-master-123".into());
    let id = ui.next_id();
    ui.vault
        .accept_create(id, password.clone(), password, VaultDocument::default())
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if matches!(ui.vault.poll(), Some(VaultNotice::Prepared)) {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    ui.vault.dispatch_save(|_| Ok(())).unwrap();
    ui.vault.handle_event(&Event::AccountVaultWritten {
        request_id: id,
        result: Ok(1),
    });
}
#[test]
fn closing_unlocked_panel_preserves_session_but_lock_clears_rows_and_read() {
    let mut ui = AccountFlow::new();
    settle_create(&mut ui);
    let epoch = ui.vault.session().epoch();
    ui.dismiss();
    assert!(ui.vault.session().is_unlocked());
    assert!(quick_panel::account_fill_ui::fill_gate(true, &ui.vault, false).is_ok());
    assert!(quick_panel::account_fill_ui::fill_gate(true, &ui.vault, true).is_err());
    assert_eq!(ui.vault.session().epoch(), epoch);
    assert_eq!(ui.access(true), Access::Ready);
    ui.lock();
    assert!(!ui.vault.session().is_unlocked());
    assert!(ui.vault.session().summaries().is_empty());
    assert_ne!(ui.vault.session().epoch(), epoch);
}
#[test]
fn failed_save_preserves_candidate_and_exit_retry_sends_identical_ciphertext() {
    use quick_panel::{account_vault::AccountDraft, model::Command};
    let mut ui = AccountFlow::new();
    settle_create(&mut ui);
    let mut document = ui.vault.session().document().unwrap().clone();
    document
        .upsert_account(AccountDraft {
            id: "synthetic".into(),
            name: "合成账号".into(),
            username: "合成用户名".into(),
            password: Secret::new("synthetic-password".into()),
        })
        .unwrap();
    let id = ui.next_id();
    ui.vault.accept_save(id, document).unwrap();
    assert!(
        quick_panel::account_fill_ui::fill_gate(true, &ui.vault, false).is_err(),
        "hint/manual boundary must deny accepted save"
    );
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !matches!(ui.vault.poll(), Some(VaultNotice::Prepared)) {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut first = vec![];
    ui.vault
        .dispatch_save(|c| {
            if let Command::WriteAccountVault { envelope, .. } = c {
                first = envelope;
            }
            Ok(())
        })
        .unwrap();
    ui.vault.handle_event(&Event::AccountVaultWritten {
        request_id: id,
        result: Err("合成磁盘故障".into()),
    });
    let guidance = quick_panel::account_fill_ui::fill_gate(true, &ui.vault, false).unwrap_err();
    assert!(guidance.contains("重试原保存") && guidance.contains("重新解锁"));
    assert_eq!(
        ui.vault.pending_document().unwrap().accounts[0].name,
        "合成账号"
    );
    assert!(ui.vault.session().document().unwrap().accounts.is_empty());
    ui.request_exit();
    assert!(!ui.shutdown_ready());
    assert!(ui.vault.pending_document().is_none());
    ui.vault.retry().unwrap();
    ui.vault
        .dispatch_save(|c| {
            if let Command::WriteAccountVault {
                request_id,
                expected_revision,
                envelope,
            } = c
            {
                assert_eq!(request_id, id);
                assert_eq!(expected_revision, 1);
                assert_eq!(envelope, first);
            } else {
                panic!("expected sealed save");
            }
            Ok(())
        })
        .unwrap();
    assert!(!ui.shutdown_ready());
    ui.vault.handle_event(&Event::AccountVaultWritten {
        request_id: id,
        result: Ok(2),
    });
    assert!(ui.shutdown_ready());
    assert!(!ui.vault.session().is_unlocked());
}
fn hint_scene() -> quick_panel::account_fill::SceneSnapshot {
    use quick_panel::account_fill::*;
    SceneSnapshot {
        complete: true,
        scope: SceneScope::Window,
        app: AppIdentity {
            canonical_path: "C:\\synthetic.exe".into(),
            file_fingerprint: [1; 32],
        },
        window: WindowIdentity {
            handle: 1,
            pid: 2,
            process_start: 3,
        },
        scene_key: "login".into(),
        origin: SceneOrigin::NativeDesktop,
        fields: vec![],
        targets: vec![],
        node_count: 1,
        max_depth: 1,
    }
}
#[test]
fn hint_query_budget_filters_app_scene_and_rejects_unbounded_selectors_browser_or_partial_scope() {
    use quick_panel::{account_fill::*, account_fill_ui::hint_candidates};
    let mut scene = hint_scene();
    let selector = FieldSelector {
        automation_id: Some("server".into()),
        control_type: 50020,
        class_name: None,
        anchors: vec![],
    };
    let binding = Binding {
        binding_id: "binding".into(),
        account_id: "account".into(),
        app: scene.app.clone(),
        scene_key: "login".into(),
        password: selector.clone(),
        username: None,
        target: Some(TargetCondition {
            selector: selector.clone(),
            expected: "synthetic-server".into(),
        }),
    };
    assert_eq!(
        hint_candidates(std::slice::from_ref(&binding), &scene).len(),
        1
    );
    let mut foreign = binding.clone();
    foreign.app.canonical_path = "C:\\other.exe".into();
    assert_eq!(
        hint_candidates(&[binding.clone(), foreign], &scene).len(),
        1
    );
    assert!(hint_candidates(&vec![binding.clone(); 9], &scene).is_empty());
    let mut alternate = binding.clone();
    alternate.target.as_mut().unwrap().selector.automation_id = Some("another".into());
    assert!(hint_candidates(&[binding.clone(), alternate], &scene).is_empty());
    scene.scope = SceneScope::SelectedContext;
    assert!(hint_candidates(std::slice::from_ref(&binding), &scene).is_empty());
    scene.scope = SceneScope::Window;
    scene.origin = SceneOrigin::UnverifiedBrowser;
    assert!(hint_candidates(std::slice::from_ref(&binding), &scene).is_empty());
}
#[test]
fn new_settings_merge_only_accounts_and_enforce_idle_bounds() {
    use quick_panel::{
        model::Settings,
        settings_ui::{self, Action},
    };
    let settings = Settings {
        capture_paused: true,
        ..Settings::default()
    };
    let next = settings_ui::edit(
        &settings,
        Action::Accounts,
        &["1".into(), "0".into(), "60".into()],
    )
    .unwrap();
    assert!(next.capture_paused);
    assert!(next.account_fill.enabled);
    assert!(!next.account_fill.show_hint);
    assert_eq!(next.account_fill.lock_minutes, 60);
    for invalid in ["0", "61", "-1", "one"] {
        assert!(
            settings_ui::edit(
                &settings,
                Action::Accounts,
                &["1".into(), "1".into(), invalid.into()]
            )
            .is_err()
        );
    }
}
#[test]
fn partial_native_result_never_claims_password_completed() {
    use quick_panel::{
        account_fill_execution::{FillError, FillOutcome},
        account_fill_ui::fill_message,
    };
    let message = fill_message(&FillOutcome {
        username_written: true,
        password_written: false,
        error: Some(FillError::WriteUncertain),
    });
    assert!(message.contains("密码未确认完成"));
    assert!(!message.contains("用户名和密码已填入"));
    let password = fill_message(&FillOutcome {
        username_written: false,
        password_written: true,
        error: None,
    });
    assert!(password.contains("仅密码已填入"));
}
#[test]
fn account_management_returns_to_settings_source_without_releasing_main_query_gate() {
    use quick_panel::settings_ui::{Group, Navigation, Page};
    let mut nav = Navigation::default();
    nav.open(Page::Memory);
    nav.open(Page::Hub(Group::Accounts));
    nav.open(Page::Accounts);
    assert!(!nav.allows_panel_query());
    assert_eq!(nav.close(), Some(Page::Hub(Group::Accounts)));
    assert!(!nav.allows_panel_query());
    assert_eq!(nav.close(), Some(Page::Memory));
    nav.close();
    assert!(nav.allows_panel_query());
}

#[test]
fn hint_focus_or_layout_event_invalidates_click_before_timer_consumes_event() {
    use quick_panel::account_fill_ui::{HintChecks, HintClock};
    for _event_kind in ["focus", "layout"] {
        let clock = HintClock::default();
        let mut checks = HintChecks::new(clock.clone());
        let version = checks.take_check().unwrap();
        assert!(checks.offer(version));
        clock.event(); // Same producer call as the native callback; no timer/check yet.
        assert!(checks.consume(version).is_none());
        assert!(!checks.offer(version)); // A late MTA inspection cannot reinstall it.
    }
}
#[test]
fn same_native_hwnd_logical_focus_returns_to_password_each_round() {
    use quick_panel::account_fill_ui::{HintChecks, HintClock};
    let clock = HintClock::default();
    let mut checks = HintChecks::new(clock.clone());
    for password_focused in [false, true, false, true] {
        // Virtual fields share HWND/object/child; each actual focus notification counts.
        clock.event();
        let version = checks
            .take_check()
            .expect("focus change must schedule a fresh bounded check");
        assert!(checks.take_check().is_none());
        if password_focused {
            assert!(checks.offer(version));
            assert!(checks.consume(version).is_some());
            assert!(checks.consume(version).is_none());
        }
    }
}
#[test]
fn repeated_hint_events_coalesce_and_consumed_grant_survives_internal_hide_only() {
    use quick_panel::account_fill_ui::{HintChecks, HintClock};
    let clock = HintClock::default();
    let mut checks = HintChecks::new(clock.clone());
    for _ in 0..10_000 {
        clock.event();
    }
    let version = checks.take_check().unwrap();
    assert_eq!(version, clock.version());
    for _ in 0..100 {
        assert!(checks.take_check().is_none());
    }
    assert!(checks.offer(version));
    let permit = checks.consume(version).unwrap();
    checks.hide();
    assert!(permit.current());
    assert!(permit.matches_focused_password(7, Some(7)));
    assert!(!permit.matches_focused_password(7, Some(8)));
    assert!(!permit.matches_focused_password(7, None));
    clock.event();
    assert!(!permit.current());
}
#[test]
fn queued_old_hint_click_cannot_consume_a_newly_built_offer() {
    use quick_panel::account_fill_ui::{HintChecks, HintClock};
    let clock = HintClock::default();
    let mut checks = HintChecks::new(clock.clone());
    let old = checks.take_check().unwrap();
    assert!(checks.offer(old));
    clock.event();
    let new = checks.take_check().unwrap();
    assert!(checks.offer(new));
    assert!(checks.consume(old).is_none());
    assert!(checks.consume(new).is_some());
}

#[test]
fn hidden_selection_revokes_actions_without_selecting_a_replacement() {
    use quick_panel::account_fill_ui::reconcile_selection;
    let mut selected = Some("a".into());
    assert!(!reconcile_selection(
        &mut selected,
        &["a".into(), "b".into()]
    ));
    assert_eq!(selected.as_deref(), Some("a"));
    assert!(reconcile_selection(&mut selected, &["b".into()]));
    assert!(selected.is_none());
    assert!(!reconcile_selection(&mut selected, &["b".into()]));
    assert!(selected.is_none());
    selected = Some("b".into());
    assert!(reconcile_selection(&mut selected, &[]));
    assert!(selected.is_none());
}
#[test]
fn unresolved_username_requires_selection_or_explicit_skip() {
    use quick_panel::account_fill_ui::UsernameChoice;
    assert!(UsernameChoice::suggested(None).resolve(true).is_err());
    assert_eq!(UsernameChoice::Selected(2).resolve(true), Ok(Some(2)));
    assert_eq!(UsernameChoice::Skipped.resolve(true), Ok(None));
    assert_eq!(UsernameChoice::Unresolved.resolve(false), Ok(None));
    assert_eq!(UsernameChoice::Selected(2).resolve(false), Ok(None));
}
#[test]
fn actual_target_is_readable_and_verification_is_truthful() {
    use quick_panel::{account_fill::*, account_fill_ui::target_presentation};
    let selector = FieldSelector {
        automation_id: Some("server".into()),
        control_type: 50004,
        class_name: None,
        anchors: vec![],
    };
    let scene = SceneSnapshot {
        complete: true,
        scope: SceneScope::Window,
        app: AppIdentity {
            canonical_path: "C:\\Db.exe".into(),
            file_fingerprint: [1; 32],
        },
        window: WindowIdentity {
            handle: 1,
            pid: 1,
            process_start: 1,
        },
        scene_key: "login".into(),
        origin: SceneOrigin::NativeDesktop,
        fields: vec![],
        targets: vec![LiveTarget {
            selector: selector.clone(),
            value: "synthetic-db:5432".into(),
            depth: 1,
        }],
        node_count: 1,
        max_depth: 1,
    };
    let presentation = target_presentation(&scene, None, Some(&selector));
    assert!(presentation.contains("synthetic-db:5432"));
    assert!(presentation.contains("未保存"));
    let binding = Binding {
        binding_id: "b".into(),
        account_id: "a".into(),
        app: scene.app.clone(),
        scene_key: scene.scene_key.clone(),
        password: selector.clone(),
        username: None,
        target: Some(TargetCondition {
            selector: selector.clone(),
            expected: "other-db".into(),
        }),
    };
    let presentation = target_presentation(&scene, Some(&binding), Some(&selector));
    assert!(!presentation.contains("已核验"));
    let mut scene = scene;
    let mut binding = binding;
    binding.target.as_mut().unwrap().expected = "synthetic-db:5432".into();
    assert!(
        target_presentation(&scene, Some(&binding), Some(&selector)).contains("已核验保存目标")
    );
    scene.complete = false;
    assert!(!target_presentation(&scene, Some(&binding), Some(&selector)).contains("已核验"));
    scene.complete = true;
    scene.scope = SceneScope::SelectedContext;
    assert!(!target_presentation(&scene, Some(&binding), Some(&selector)).contains("已核验"));
    scene.targets[0].value = format!("{}-endmarker", "x".repeat(MAX_TARGET_BYTES - 10));
    assert_eq!(scene.targets[0].value.len(), MAX_TARGET_BYTES);
    assert!(target_presentation(&scene, None, Some(&selector)).contains(&scene.targets[0].value));
}

#[test]
fn selection_change_or_filter_cancels_old_save_and_inspection_continuations() {
    use quick_panel::account_fill_ui::AccountSelection;
    let mut selection = AccountSelection::default();
    selection.set(Some("a".into()));
    let accepted_save = selection.token().unwrap();
    assert!(selection.permits(&accepted_save));
    selection.reconcile(&["b".into()]);
    assert!(!selection.permits(&accepted_save));
    // Removing the filter does not silently select a replacement.
    selection.reconcile(&["a".into(), "b".into()]);
    assert!(selection.current.is_none());
    selection.set(Some("a".into()));
    assert!(!selection.permits(&accepted_save));
    let inspection = selection.token().unwrap();
    selection.set(Some("b".into()));
    selection.set(Some("a".into()));
    assert!(!selection.permits(&inspection));
}

#[test]
fn completion_wake_drains_hidden_locked_save_retry_and_exit_without_poll_timer() {
    use quick_panel::{account_fill_ui::account_timer_delay, account_vault_jobs::SaveState};
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };
    let (tx, rx) = mpsc::channel();
    let mut flow = AccountFlow::with_notify(Arc::new(move || {
        let _ = tx.send(());
    }));
    assert_eq!(account_timer_delay(false, Duration::ZERO, 5, false), None);
    let id = flow.next_id();
    let master = Secret::new("synthetic-wake-master".into());
    flow.vault
        .accept_create(id, master.clone(), master, VaultDocument::default())
        .unwrap();
    flow.dismiss();
    flow.request_exit();
    assert!(!flow.shutdown_ready());
    rx.recv_timeout(Duration::from_secs(30))
        .expect("background completion must wake the closed UI");
    assert_eq!(flow.vault.poll(), Some(VaultNotice::Prepared));
    let mut commands = vec![];
    flow.vault
        .dispatch_save(|command| {
            commands.push(command);
            Ok(())
        })
        .unwrap();
    assert_eq!(commands.len(), 1);
    assert!(
        flow.vault
            .handle_event(&Event::AccountVaultWritten {
                request_id: id + 1,
                result: Ok(1)
            })
            .is_none()
    );
    flow.vault.handle_event(&Event::AccountVaultWritten {
        request_id: id,
        result: Err("合成磁盘错误".into()),
    });
    assert_eq!(flow.vault.state(), SaveState::FailedWrite);
    assert!(!flow.shutdown_ready());
    flow.vault.retry().unwrap();
    flow.vault
        .dispatch_save(|command| {
            commands.push(command);
            Ok(())
        })
        .unwrap();
    flow.vault.handle_event(&Event::AccountVaultWritten {
        request_id: id,
        result: Ok(1),
    });
    assert!(flow.shutdown_ready());
    assert!(!flow.vault.session().is_unlocked());
    assert_eq!(commands.len(), 2);
    assert_eq!(account_timer_delay(false, Duration::ZERO, 5, false), None);
}

#[test]
fn account_idle_deadline_and_native_guard_select_only_needed_wakes() {
    use quick_panel::account_fill_ui::account_timer_delay;
    use std::time::Duration as D;
    assert_eq!(account_timer_delay(false, D::ZERO, 5, false), None);
    assert_eq!(
        account_timer_delay(true, D::from_secs(299), 5, false),
        Some(D::from_secs(1))
    );
    assert_eq!(
        account_timer_delay(true, D::from_secs(300), 5, false),
        Some(D::ZERO)
    );
    assert_eq!(
        account_timer_delay(true, D::ZERO, 5, false),
        Some(D::from_secs(300))
    );
    assert_eq!(
        account_timer_delay(false, D::ZERO, 5, true),
        Some(D::from_millis(100))
    );
    assert_eq!(
        account_timer_delay(true, D::from_millis(299950), 5, true),
        Some(D::from_millis(50))
    );
}
