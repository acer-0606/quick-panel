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
