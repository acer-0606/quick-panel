use quick_panel::account_fill::*;

fn selector(id: &str) -> FieldSelector {
    FieldSelector {
        automation_id: Some(id.into()),
        control_type: 50004,
        class_name: Some("Edit".into()),
        anchors: vec![SelectorAnchor {
            automation_id: Some("login".into()),
            control_type: 50033,
            class_name: None,
            label: Some("登录".into()),
        }],
    }
}
fn scene() -> SceneSnapshot {
    SceneSnapshot {
        complete: true,
        scope: quick_panel::account_fill::SceneScope::Window,
        app: AppIdentity {
            canonical_path: "C:\\Apps\\DbClient.exe".into(),
            file_fingerprint: [7; 32],
        },
        window: WindowIdentity {
            handle: 20,
            pid: 100,
            process_start: 500,
        },
        scene_key: "login-dialog".into(),
        origin: SceneOrigin::NativeDesktop,
        fields: vec![
            LiveField {
                field_id: 1,
                selector: selector("password"),
                depth: 2,
                is_password: true,
                visible: true,
                enabled: true,
                writable: true,
            },
            LiveField {
                field_id: 2,
                selector: selector("username"),
                depth: 2,
                is_password: false,
                visible: true,
                enabled: true,
                writable: true,
            },
        ],
        targets: vec![LiveTarget {
            selector: selector("server"),
            value: "db-prod.example:5432".into(),
            depth: 2,
        }],
        node_count: 4,
        max_depth: 2,
    }
}
fn binding() -> Binding {
    Binding {
        binding_id: "binding-1".into(),
        account_id: "account-1".into(),
        app: scene().app,
        scene_key: "login-dialog".into(),
        password: selector("password"),
        username: Some(selector("username")),
        target: Some(TargetCondition {
            selector: selector("server"),
            expected: "db-prod.example:5432".into(),
        }),
    }
}
fn enabled() -> AccountFillSettings {
    AccountFillSettings {
        enabled: true,
        ..Default::default()
    }
}
fn resolved() -> ResolvedFields {
    ResolvedFields {
        password_id: 1,
        username_id: Some(2),
    }
}
fn reject(b: &Binding, s: &SceneSnapshot) {
    assert!(matches!(match_binding(b, s), MatchDecision::Rejected(_)));
}

#[test]
fn different_or_missing_target_never_matches() {
    assert!(!target_matches(
        "db-prod.example:5432",
        Some("db-test.example:5432")
    ));
    assert!(!target_matches("db-prod.example:5432", None));
    assert!(target_matches(
        "db-prod.example:5432",
        Some(" db-prod.example:5432 ")
    ));
    assert!(!target_matches("", Some(" ")));
    assert!(!target_matches("db-prod", Some("DB-PROD")));
    assert!(!target_matches("db-prod", Some("db-prod.example")));
}
#[test]
fn only_same_app_scene_unique_fields_and_existing_exact_target_match() {
    assert_eq!(
        match_binding(&binding(), &scene()),
        MatchDecision::Exact(resolved())
    );
    let mut s = scene();
    s.app.canonical_path = "C:\\Other.exe".into();
    reject(&binding(), &s);
    s = scene();
    s.app.file_fingerprint = [8; 32];
    reject(&binding(), &s);
    s = scene();
    s.scene_key = "different-page".into();
    reject(&binding(), &s);
    s = scene();
    s.targets[0].value = "db-test.example:5432".into();
    reject(&binding(), &s);
    s.targets.clear();
    reject(&binding(), &s);
    s = scene();
    s.targets.push(s.targets[0].clone());
    s.node_count += 1;
    reject(&binding(), &s);
}
#[test]
fn application_only_never_becomes_exact_or_a_confirmed_hint() {
    let mut b = binding();
    b.target = None;
    assert_eq!(
        match_binding(&b, &scene()),
        MatchDecision::ApplicationOnly(resolved())
    );
    let mut gate = FillGate::default();
    assert!(
        gate.begin(
            UserFillAction::ConfirmedHint,
            &enabled(),
            1,
            true,
            &b,
            &scene()
        )
        .is_err()
    );
    assert!(
        gate.begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            1,
            true,
            &b,
            &scene()
        )
        .is_ok()
    );
}
#[test]
fn password_must_be_unique_supported_and_distinct_from_username() {
    for changed in 0..4 {
        let mut s = scene();
        match changed {
            0 => s.fields[0].is_password = false,
            1 => s.fields[0].visible = false,
            2 => s.fields[0].enabled = false,
            _ => s.fields[0].writable = false,
        }
        reject(&binding(), &s);
    }
    let mut s = scene();
    let mut second = s.fields[0].clone();
    second.field_id = 3;
    s.fields.push(second);
    s.node_count += 1;
    reject(&binding(), &s);
    s = scene();
    s.fields.push(s.fields[0].clone());
    s.node_count += 1;
    reject(&binding(), &s);
    let mut b = binding();
    b.username = Some(b.password.clone());
    reject(&b, &scene());
    s = scene();
    s.fields[1].is_password = true;
    reject(&binding(), &s);
    s = scene();
    s.fields[1].writable = false;
    reject(&binding(), &s);
}
#[test]
fn tickets_bind_window_instance_scene_account_and_epoch_and_are_single_use() {
    let mut gate = FillGate::default();
    let s = scene();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            10,
            true,
            &binding(),
            &s,
        )
        .unwrap();
    assert_eq!(
        gate.validate(&ticket, &enabled(), 10, true, &s),
        Ok(resolved())
    );
    for changed in 0..7 {
        let mut fresh = s.clone();
        match changed {
            0 => fresh.window.handle += 1,
            1 => fresh.window.pid += 1,
            2 => fresh.window.process_start += 1,
            3 => fresh.app.file_fingerprint = [9; 32],
            4 => fresh.targets[0].value = "other".into(),
            5 => fresh.fields[0].field_id = 7,
            _ => fresh.scene_key = "other".into(),
        }
        assert!(
            gate.validate(&ticket, &enabled(), 10, true, &fresh)
                .is_err()
        );
    }
    assert!(gate.validate(&ticket, &enabled(), 11, true, &s).is_err());
    assert!(gate.validate(&ticket, &enabled(), 10, false, &s).is_err());
    assert!(
        gate.validate(&ticket, &AccountFillSettings::default(), 10, true, &s)
            .is_err()
    );
    let mut other = binding();
    other.account_id = "account-2".into();
    other.binding_id = "binding-2".into();
    let newer = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            10,
            true,
            &other,
            &s,
        )
        .unwrap();
    assert!(gate.validate(&ticket, &enabled(), 10, true, &s).is_err());
    assert_eq!(
        gate.consume(&newer, &enabled(), 10, true, &s),
        Ok(resolved())
    );
    assert!(gate.consume(&newer, &enabled(), 10, true, &s).is_err());
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            10,
            true,
            &binding(),
            &s,
        )
        .unwrap();
    gate.revoke();
    assert!(gate.validate(&ticket, &enabled(), 10, true, &s).is_err());
}
#[test]
fn disabled_or_locked_start_revokes_previous_permission() {
    let mut gate = FillGate::default();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            1,
            true,
            &binding(),
            &scene(),
        )
        .unwrap();
    assert!(
        gate.begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            1,
            false,
            &binding(),
            &scene()
        )
        .is_err()
    );
    assert!(
        gate.validate(&ticket, &enabled(), 1, true, &scene())
            .is_err()
    );
    assert!(
        gate.begin(
            UserFillAction::SelectedAccount,
            &AccountFillSettings::default(),
            1,
            true,
            &binding(),
            &scene()
        )
        .is_err()
    );
}
#[test]
fn settings_are_backward_compatible_bounded_and_disable_observation() {
    let s: quick_panel::model::Settings = serde_json::from_str("{}").unwrap();
    assert!(!s.account_fill.observe_enabled(true));
    assert_eq!(s.account_fill.lock_minutes, 5);
    let partial: AccountFillSettings = serde_json::from_str("{\"enabled\":true}").unwrap();
    assert!(partial.observe_enabled(true));
    assert!(!partial.observe_enabled(false));
    for minutes in [0, 61, u32::MAX] {
        assert!(
            AccountFillSettings {
                lock_minutes: minutes,
                ..enabled()
            }
            .validate()
            .is_err()
        );
    }
    for minutes in [1, 60] {
        assert!(
            AccountFillSettings {
                lock_minutes: minutes,
                ..enabled()
            }
            .validate()
            .is_ok()
        );
    }
}
#[test]
fn bounded_scene_and_selectors_fail_closed_even_after_deserialization() {
    let mut s = scene();
    s.node_count = 257;
    assert!(s.validate().is_err());
    reject(&binding(), &s);
    s = scene();
    s.max_depth = 9;
    assert!(s.validate().is_err());
    s = scene();
    s.fields[0].depth = 9;
    assert!(s.validate().is_err());
    s = scene();
    s.node_count = 1;
    assert!(s.validate().is_err());
    s = scene();
    s.scene_key = "x".repeat(1025);
    assert!(s.validate().is_err());
    s = scene();
    s.targets[0].value = "x".repeat(1025);
    assert!(s.validate().is_err());
    let mut b = binding();
    b.password.automation_id = Some("x".repeat(257));
    reject(&b, &scene());
    b = binding();
    b.password.anchors = vec![b.password.anchors[0].clone(); 8];
    assert!(b.validate().is_err());
    let decoded: Binding = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    reject(&decoded, &scene());
    let mut s = scene();
    s.node_count = 256;
    s.max_depth = 8;
    assert!(s.validate().is_ok());
}
#[test]
fn teaching_is_ordered_cancellable_unique_and_never_persists_browser_guess() {
    let mut teaching = TeachingSession::begin(&scene()).unwrap();
    assert_eq!(teaching.step(), TeachingStep::Password);
    assert!(teaching.select_username(Some(2)).is_err());
    teaching.select_password(1).unwrap();
    teaching.select_username(Some(2)).unwrap();
    teaching.select_target(Some(&selector("server"))).unwrap();
    assert_eq!(
        teaching
            .finish("binding-1".into(), "account-1".into())
            .unwrap(),
        binding()
    );
    for cancel_at in 0..4 {
        let mut t = TeachingSession::begin(&scene()).unwrap();
        if cancel_at > 0 {
            t.select_password(1).unwrap();
        }
        if cancel_at > 1 {
            t.select_username(None).unwrap();
        }
        if cancel_at > 2 {
            t.select_target(None).unwrap();
        }
        t.cancel();
        assert!(t.finish("b".into(), "a".into()).is_err());
    }
    assert!(
        TeachingSession::begin(&scene())
            .unwrap()
            .finish("b".into(), "a".into())
            .is_err()
    );
    let mut s = scene();
    s.node_count = 257;
    assert!(TeachingSession::begin(&s).is_err());
    s = scene();
    let mut second = s.fields[0].clone();
    second.field_id = 3;
    s.fields.push(second);
    s.node_count += 1;
    assert!(
        TeachingSession::begin(&s)
            .unwrap()
            .select_password(1)
            .is_err()
    );
    s = scene();
    s.origin = SceneOrigin::UnverifiedBrowser;
    let mut t = TeachingSession::begin(&s).unwrap();
    t.select_password(1).unwrap();
    t.select_username(None).unwrap();
    t.select_target(None).unwrap();
    assert!(t.finish("b".into(), "a".into()).is_err());
    assert!(matches!(
        match_binding(&binding(), &s),
        MatchDecision::ApplicationOnly(_)
    ));
    assert!(
        FillGate::default()
            .begin(
                UserFillAction::SelectedAccount,
                &enabled(),
                1,
                true,
                &binding(),
                &s
            )
            .is_ok()
    );
}

#[test]
fn cross_application_binding_is_rejected() {
    let mut s = scene();
    s.app.canonical_path = "C:\\Other.exe".into();
    reject(&binding(), &s);
}
#[test]
fn different_server_binding_is_rejected() {
    let mut s = scene();
    s.targets[0].value = "db-test.example:5432".into();
    reject(&binding(), &s);
}
#[test]
fn missing_server_binding_is_rejected() {
    let mut s = scene();
    s.targets.clear();
    reject(&binding(), &s);
}
#[test]
fn multiple_password_candidates_are_rejected() {
    let mut s = scene();
    let mut extra = s.fields[0].clone();
    extra.field_id = 3;
    s.fields.push(extra);
    s.node_count += 1;
    reject(&binding(), &s);
}
#[test]
fn reused_window_handle_with_another_process_is_rejected() {
    let mut gate = FillGate::default();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            10,
            true,
            &binding(),
            &scene(),
        )
        .unwrap();
    let mut fresh = scene();
    fresh.window.process_start += 1;
    assert!(
        gate.validate(&ticket, &enabled(), 10, true, &fresh)
            .is_err()
    );
}
#[test]
fn old_vault_epoch_is_rejected() {
    let mut gate = FillGate::default();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            10,
            true,
            &binding(),
            &scene(),
        )
        .unwrap();
    assert!(
        gate.validate(&ticket, &enabled(), 11, true, &scene())
            .is_err()
    );
}
#[test]
fn cancelled_teaching_does_not_produce_a_binding() {
    let mut t = TeachingSession::begin(&scene()).unwrap();
    t.select_password(1).unwrap();
    t.select_username(None).unwrap();
    t.select_target(None).unwrap();
    t.cancel();
    assert!(t.finish("b".into(), "a".into()).is_err());
}
#[test]
fn selector_conditions_use_exact_optional_constraints_and_parent_order() {
    let mut broad = selector("password");
    broad.automation_id = None;
    assert!(broad.matches(&selector("password")));
    let mut wrong_parent = selector("password");
    wrong_parent.anchors[0].label = Some("其他页面".into());
    assert!(!broad.matches(&wrong_parent));
    let mut observed = selector("password");
    observed.automation_id = Some("password-extra".into());
    assert!(!selector("password").matches(&observed));
    assert!(broad.matches(&observed));
}

#[test]
fn stale_consume_and_failed_teaching_never_leave_hidden_permission() {
    let mut gate = FillGate::default();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            10,
            true,
            &binding(),
            &scene(),
        )
        .unwrap();
    assert!(
        gate.consume(&ticket, &enabled(), 11, true, &scene())
            .is_err()
    );
    assert!(
        gate.validate(&ticket, &enabled(), 10, true, &scene())
            .is_err()
    );
    let mut teaching = TeachingSession::begin(&scene()).unwrap();
    assert!(teaching.select_password(2).is_err());
    assert_eq!(teaching.step(), TeachingStep::Password);
    teaching.select_password(1).unwrap();
    assert!(teaching.select_username(Some(1)).is_err());
    assert_eq!(teaching.step(), TeachingStep::Username);
    teaching.select_username(None).unwrap();
    assert!(teaching.select_target(Some(&selector("missing"))).is_err());
    assert_eq!(teaching.step(), TeachingStep::Target);
    teaching.select_target(None).unwrap();
    let b = teaching.finish("b".into(), "a".into()).unwrap();
    assert_eq!(
        match_binding(&b, &scene()),
        MatchDecision::ApplicationOnly(ResolvedFields {
            password_id: 1,
            username_id: None
        })
    );
}

#[test]
fn incomplete_uia_traversal_never_proves_uniqueness_or_authorizes_teaching() {
    let mut s = scene();
    s.complete = false;
    reject(&binding(), &s);
    assert!(TeachingSession::begin(&s).is_err());
    assert!(
        FillGate::default()
            .begin(
                UserFillAction::ConfirmedHint,
                &enabled(),
                1,
                true,
                &binding(),
                &s
            )
            .is_err()
    );
    assert!(
        FillGate::default()
            .begin(
                UserFillAction::SelectedAccount,
                &enabled(),
                1,
                true,
                &binding(),
                &s
            )
            .is_err()
    );
    let mut gate = FillGate::default();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &enabled(),
            1,
            true,
            &binding(),
            &scene(),
        )
        .unwrap();
    assert!(gate.validate(&ticket, &enabled(), 1, true, &s).is_err());
}

#[test]
fn selected_scope_never_becomes_exact_or_reusable_teaching() {
    let mut snapshot = scene();
    snapshot.scope = quick_panel::account_fill::SceneScope::SelectedContext;
    let binding = binding();
    assert!(matches!(
        match_binding(&binding, &snapshot),
        MatchDecision::ApplicationOnly(_)
    ));
    let mut teacher = TeachingSession::begin(&snapshot).unwrap();
    teacher
        .select_password(
            snapshot
                .fields
                .iter()
                .find(|f| f.is_password)
                .unwrap()
                .field_id,
        )
        .unwrap();
    teacher.select_username(None).unwrap();
    teacher.select_target(None).unwrap();
    assert!(teacher.finish("small".into(), "account".into()).is_err());
}
