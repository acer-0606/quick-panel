use quick_panel::account_fill::*;
use quick_panel::account_fill_execution::*;
use quick_panel::account_vault::Secret;

fn scene() -> SceneSnapshot {
    SceneSnapshot {
        complete: true,
        scope: quick_panel::account_fill::SceneScope::Window,
        app: AppIdentity {
            canonical_path: "C:\\synthetic.exe".into(),
            file_fingerprint: [1; 32],
        },
        window: WindowIdentity {
            handle: 10,
            pid: 20,
            process_start: 30,
        },
        scene_key: "login".into(),
        origin: SceneOrigin::NativeDesktop,
        fields: vec![LiveField {
            field_id: 1,
            selector: FieldSelector {
                automation_id: Some("password".into()),
                control_type: 50004,
                class_name: None,
                anchors: vec![],
            },
            depth: 1,
            is_password: true,
            visible: true,
            enabled: true,
            writable: true,
        }],
        targets: vec![],
        node_count: 1,
        max_depth: 1,
    }
}
fn binding(s: &SceneSnapshot) -> Binding {
    Binding {
        binding_id: "binding".into(),
        account_id: "account".into(),
        app: s.app.clone(),
        scene_key: s.scene_key.clone(),
        password: s.fields[0].selector.clone(),
        username: None,
        target: None,
    }
}
struct NativeDouble {
    scenes: Vec<SceneSnapshot>,
    writes: Vec<u64>,
    control: FillControl,
    cancel_on_write: bool,
}
impl FillBackend for NativeDouble {
    fn sample(&mut self) -> Result<SceneSnapshot, FillError> {
        Ok(self.scenes.remove(0))
    }
    fn foreground_matches(&self, _: WindowIdentity) -> bool {
        true
    }
    fn write(&mut self, field: u64, _: &Secret) -> Result<(), FillError> {
        self.writes.push(field);
        if self.cancel_on_write {
            self.control.invalidate(2);
        }
        Ok(())
    }
}
fn run(s: &SceneSnapshot, b: &Binding, backend: &mut NativeDouble, c: &FillControl) -> FillOutcome {
    let lease = c.reserve(1).unwrap();
    execute_fill(
        &mut FillGate::default(),
        backend,
        &lease,
        FillIntent {
            settings: AccountFillSettings {
                enabled: true,
                ..Default::default()
            },
            baseline: s.clone(),
            choice: FillChoice::Saved(b.clone()),
            action: UserFillAction::SelectedAccount,
            browser_password_picked: false,
            username: Secret::new("synthetic-user".into()),
            password: Secret::new("synthetic-password".into()),
        },
    )
}
#[test]
fn cancellation_keeps_actual_slot_and_rejects_old_epoch() {
    let c = FillControl::new(1);
    let lease = c.reserve(1).unwrap();
    c.invalidate(2);
    assert!(!lease.is_current());
    assert!(c.reserve(2).is_err());
    drop(lease);
    assert!(c.reserve(1).is_err());
    assert!(c.reserve(2).is_ok());
}
#[test]
fn changed_window_process_or_field_never_writes() {
    let s = scene();
    let b = binding(&s);
    for change in 0..7 {
        let c = FillControl::new(1);
        let mut changed = s.clone();
        match change {
            0 => changed.window.handle = 11,
            1 => changed.window.pid = 21,
            2 => changed.window.process_start = 31,
            3 => changed.fields.clear(),
            4 => changed.fields[0].field_id = 99,
            5 => changed.complete = false,
            _ => changed.app.file_fingerprint = [2; 32],
        };
        let mut backend = NativeDouble {
            scenes: vec![changed],
            writes: vec![],
            control: c.clone(),
            cancel_on_write: false,
        };
        assert!(!run(&s, &b, &mut backend, &c).password_written);
        assert!(backend.writes.is_empty());
    }
}
#[test]
fn invalid_password_and_ambiguous_candidates_never_write() {
    for change in 0..5 {
        let mut s = scene();
        match change {
            0 => s.fields[0].visible = false,
            1 => s.fields[0].enabled = false,
            2 => s.fields[0].writable = false,
            3 => s.fields[0].is_password = false,
            _ => {
                let mut f = s.fields[0].clone();
                f.field_id = 2;
                s.fields.push(f);
                s.node_count = 2;
            }
        };
        let b = binding(&s);
        let c = FillControl::new(1);
        let mut backend = NativeDouble {
            scenes: vec![s.clone()],
            writes: vec![],
            control: c.clone(),
            cancel_on_write: false,
        };
        assert!(!run(&s, &b, &mut backend, &c).password_written);
        assert!(backend.writes.is_empty());
    }
}
#[test]
fn browser_requires_explicit_password_point_selection() {
    let mut s = scene();
    s.origin = SceneOrigin::UnverifiedBrowser;
    let b = binding(&s);
    let c = FillControl::new(1);
    let mut backend = NativeDouble {
        scenes: vec![s.clone()],
        writes: vec![],
        control: c.clone(),
        cancel_on_write: false,
    };
    assert!(!run(&s, &b, &mut backend, &c).password_written);
    assert!(backend.writes.is_empty());
}
#[test]
fn cancellation_after_username_reports_partial_and_stops_password() {
    let mut s = scene();
    let mut user = s.fields[0].clone();
    user.field_id = 2;
    user.is_password = false;
    user.selector.automation_id = Some("username".into());
    s.fields.push(user);
    s.node_count = 2;
    let mut b = binding(&s);
    b.username = Some(s.fields[1].selector.clone());
    let c = FillControl::new(1);
    let mut backend = NativeDouble {
        scenes: vec![s.clone(), s.clone()],
        writes: vec![],
        control: c.clone(),
        cancel_on_write: true,
    };
    let result = run(&s, &b, &mut backend, &c);
    assert!(result.username_written);
    assert!(!result.password_written);
    assert_eq!(backend.writes, vec![2]);
    assert!(result.error.is_some());
}
#[test]
fn valid_explicit_fill_writes_once() {
    let s = scene();
    let b = binding(&s);
    let c = FillControl::new(1);
    let mut backend = NativeDouble {
        scenes: vec![s.clone()],
        writes: vec![],
        control: c.clone(),
        cancel_on_write: false,
    };
    assert!(run(&s, &b, &mut backend, &c).password_written);
    assert_eq!(backend.writes, vec![1]);
}

#[test]
fn malformed_credential_never_reaches_native_write() {
    let s = scene();
    let b = binding(&s);
    let c = FillControl::new(1);
    let mut backend = NativeDouble {
        scenes: vec![s.clone()],
        writes: vec![],
        control: c.clone(),
        cancel_on_write: false,
    };
    let lease = c.reserve(1).unwrap();
    let result = execute_fill(
        &mut FillGate::default(),
        &mut backend,
        &lease,
        FillIntent {
            settings: AccountFillSettings {
                enabled: true,
                ..Default::default()
            },
            baseline: s,
            choice: FillChoice::Saved(b),
            action: UserFillAction::SelectedAccount,
            browser_password_picked: false,
            username: Secret::new("user".into()),
            password: Secret::new("bad\0password".into()),
        },
    );
    assert!(!result.password_written);
    assert!(backend.writes.is_empty());
}

#[test]
fn selected_context_saved_binding_is_rejected_by_execution_boundary() {
    let mut s = scene();
    s.scope = SceneScope::SelectedContext;
    let b = binding(&s);
    let c = FillControl::new(1);
    let mut backend = NativeDouble {
        scenes: vec![s.clone()],
        writes: vec![],
        control: c.clone(),
        cancel_on_write: false,
    };
    assert!(!run(&s, &b, &mut backend, &c).password_written);
    assert!(backend.writes.is_empty());
}

#[test]
fn manual_once_context_and_browser_need_selected_action_and_password_pick() {
    for browser in [false, true] {
        for picked in [false, true] {
            let mut s = scene();
            s.scope = SceneScope::SelectedContext;
            if browser {
                s.origin = SceneOrigin::UnverifiedBrowser;
            }
            let mut teacher = TeachingSession::begin(&s).unwrap();
            teacher.select_password(1).unwrap();
            teacher.select_username(None).unwrap();
            teacher.select_target(None).unwrap();
            let once = teacher
                .finish_once("once".into(), "account".into())
                .unwrap();
            let c = FillControl::new(1);
            let lease = c.reserve(1).unwrap();
            let mut backend = NativeDouble {
                scenes: vec![s.clone()],
                writes: vec![],
                control: c.clone(),
                cancel_on_write: false,
            };
            let result = execute_fill(
                &mut FillGate::default(),
                &mut backend,
                &lease,
                FillIntent {
                    settings: AccountFillSettings {
                        enabled: true,
                        ..Default::default()
                    },
                    baseline: s,
                    choice: FillChoice::Once(once),
                    action: UserFillAction::SelectedAccount,
                    browser_password_picked: picked,
                    username: Secret::new(String::new()),
                    password: Secret::new("synthetic".into()),
                },
            );
            assert_eq!(result.password_written, !browser || picked);
        }
    }
}

#[test]
fn hiding_panel_revokes_generation_without_changing_unlocked_epoch() {
    let control = FillControl::new(41);
    let old = control.reserve(41).unwrap();
    control.invalidate(41);
    assert!(!old.is_current());
    assert!(control.reserve(41).is_err());
    drop(old);
    let next = control.reserve(41).unwrap();
    assert!(next.is_current());
    assert_eq!(next.epoch(), 41);
}
