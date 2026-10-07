#![cfg(windows)]
//! Synthetic controls only. These tests require an interactive Windows desktop.
use quick_panel::{
    account_fill::*,
    account_fill_execution::*,
    account_vault::Secret,
    platform::windows::{account_picker::*, account_target::*, account_worker::*},
};
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::*,
        UI::{HiDpi::*, Input::KeyboardAndMouse::SetFocus, WindowsAndMessaging::*},
    },
    core::w,
};
struct Fixture {
    root: HWND,
    user: HWND,
    password: HWND,
    server: HWND,
}
impl Fixture {
    unsafe fn new(x: i32) -> Self {
        unsafe {
            let root = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Synthetic account fixture"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                x,
                100,
                450,
                240,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let user = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER,
                20,
                40,
                280,
                30,
                Some(root),
                Some(HMENU(101usize as *mut _)),
                None,
                None,
            )
            .unwrap();
            let password = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | WINDOW_STYLE(ES_PASSWORD as u32),
                20,
                90,
                280,
                30,
                Some(root),
                Some(HMENU(102usize as *mut _)),
                None,
                None,
            )
            .unwrap();
            let server = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!("db-a"),
                WS_CHILD | WS_VISIBLE | WS_BORDER,
                20,
                140,
                280,
                30,
                Some(root),
                Some(HMENU(103usize as *mut _)),
                None,
                None,
            )
            .unwrap();
            Self {
                root,
                user,
                password,
                server,
            }
        }
    }
    fn activate(&self) {
        unsafe {
            assert!(SetForegroundWindow(self.root).as_bool());
            let _ = SetFocus(Some(self.password));
        }
    }
    fn identity(&self) -> WindowIdentity {
        window_identity(self.root.0 as usize as u64).unwrap()
    }
    fn password_text(&self) -> String {
        unsafe {
            let mut text = [0u16; 100];
            let n = GetWindowTextW(self.password, &mut text);
            String::from_utf16_lossy(&text[..n as usize])
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.root);
        }
    }
}
fn pump() {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
fn wait(worker: &AccountWorker, id: u64) -> AccountReply {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        pump();
        if let Some(reply) = worker.try_recv()
            && reply.request_id == id
        {
            return reply;
        }
        assert!(Instant::now() < deadline, "synthetic UIA worker timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn draft(inspection: &Inspection) -> Binding {
    let scene = &inspection.snapshot;
    Binding {
        binding_id: "synthetic-binding".into(),
        account_id: "synthetic-account".into(),
        app: scene.app.clone(),
        scene_key: scene.scene_key.clone(),
        password: scene
            .fields
            .iter()
            .find(|f| f.is_password)
            .unwrap()
            .selector
            .clone(),
        username: scene
            .fields
            .iter()
            .find(|f| !f.is_password && f.selector.control_type == 50004)
            .map(|f| f.selector.clone()),
        target: None,
    }
}
fn command(id: u64, binding: Binding, expected_window: WindowIdentity) -> FillCommand {
    FillCommand {
        inspection_id: id,
        expected_window,
        choice: FillChoice::Saved(binding),
        settings: AccountFillSettings {
            enabled: true,
            ..Default::default()
        },
        action: UserFillAction::SelectedAccount,
        username: Secret::new("synthetic-user".into()),
        password: Secret::new("synthetic-password".into()),
    }
}
#[test]
fn native_synthetic_capture_inspect_write_two_windows_and_cancel() {
    struct Dpi(DPI_AWARENESS_CONTEXT);
    impl Drop for Dpi {
        fn drop(&mut self) {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
    unsafe {
        let _dpi = Dpi(SetThreadDpiAwarenessContext(
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        ));
        let first = Fixture::new(40);
        let second = Fixture::new(550);
        first.activate();
        SetWindowTextW(first.user, w!("synthetic-private-username")).unwrap();
        let captured = capture_foreground(u32::MAX).unwrap();
        assert_eq!(captured, first.identity());
        let mut worker = AccountWorker::new(1).unwrap();
        assert_eq!(AccountWorker::new(1).err(), Some(FillError::Busy));
        second.activate(); // Represents panel showing after capture.
        let id = worker.inspect(1, captured).unwrap();
        let AccountResult::Inspected(inspected) = wait(&worker, id).result else {
            panic!("synthetic inspect failed")
        };
        assert_eq!(inspected.snapshot.window, captured);
        assert!(!format!("{:?}", inspected.snapshot).contains("synthetic-private-username"));
        assert!(inspected.snapshot.targets.is_empty());
        assert_eq!(
            inspected
                .snapshot
                .fields
                .iter()
                .filter(|f| f.is_password)
                .count(),
            1
        );
        assert!(
            inspected
                .snapshot
                .fields
                .iter()
                .all(|f| f.selector.automation_id.as_deref() != Some("synthetic-password"))
        );
        let binding = draft(&inspected);
        assert_eq!(
            worker.fill(1, command(id, binding, captured)).err(),
            Some(FillError::TargetChanged)
        );
        assert_eq!(first.password_text(), "");
        assert_eq!(second.password_text(), "");
        first.activate();
        let id = worker.inspect(1, captured).unwrap();
        let AccountResult::Inspected(inspected) = wait(&worker, id).result else {
            panic!("synthetic inspect failed")
        };
        let fill = worker
            .fill(1, command(id, draft(&inspected), captured))
            .unwrap();
        let AccountResult::Filled(result) = wait(&worker, fill).result else {
            panic!("synthetic fill failed")
        };
        assert!(result.password_written, "{result:?}");
        assert_eq!(first.password_text(), "synthetic-password");
        assert_eq!(second.password_text(), "");
        let inspect_id = worker.inspect(1, captured).unwrap();
        let AccountResult::Inspected(scene) = wait(&worker, inspect_id).result else {
            panic!("synthetic target inspect failed")
        };
        let server_selector = scene
            .snapshot
            .fields
            .iter()
            .find(|f| f.selector.automation_id.as_deref() == Some("103"))
            .unwrap()
            .selector
            .clone();
        let mut remembered = draft(&scene);
        remembered.target = Some(TargetCondition {
            selector: server_selector,
            expected: "db-a".into(),
        });
        let target_id = worker
            .inspect_binding(1, captured, remembered.clone())
            .unwrap();
        let AccountResult::Inspected(target_scene) = wait(&worker, target_id).result else {
            panic!("synthetic target read failed")
        };
        assert_eq!(target_scene.snapshot.targets[0].value, "db-a");
        assert!(matches!(
            match_binding(&remembered, &target_scene.snapshot),
            MatchDecision::Exact(_)
        ));
        // Capture native focus only after moving to the username HWND. The old
        // HWND-only write guard would accept this; ConfirmedHint must query UIA focus.
        use quick_panel::account_fill_ui::{HintChecks, HintClock};
        let permit = || {
            let clock = HintClock::default();
            let mut checks = HintChecks::new(clock.clone());
            let v = checks.take_check().unwrap();
            assert!(checks.offer(v));
            (clock, checks.consume(v).unwrap())
        };
        SetWindowTextW(first.password, w!("")).unwrap();
        let _ = SetFocus(Some(first.user));
        let mut hinted = command(target_id, remembered.clone(), captured);
        hinted.action = UserFillAction::ConfirmedHint;
        let (_, grant) = permit();
        let request = worker.fill_hint(1, hinted, grant).unwrap();
        let AccountResult::Filled(wrong_focus) = wait(&worker, request).result else {
            panic!("hint focus result missing")
        };
        assert!(!wrong_focus.username_written && !wrong_focus.password_written);
        assert_eq!(wrong_focus.error, Some(FillError::TargetChanged));
        assert_eq!(first.password_text(), "");
        first.activate();
        let fresh = worker
            .inspect_binding(1, captured, remembered.clone())
            .unwrap();
        assert!(matches!(
            wait(&worker, fresh).result,
            AccountResult::Inspected(_)
        ));
        let mut hinted = command(fresh, remembered.clone(), captured);
        hinted.action = UserFillAction::ConfirmedHint;
        let (clock, grant) = permit();
        clock.event();
        assert_eq!(
            worker.fill_hint(1, hinted, grant).err(),
            Some(FillError::Cancelled)
        );
        let mut hinted = command(fresh, remembered.clone(), captured);
        hinted.action = UserFillAction::ConfirmedHint;
        assert_eq!(worker.fill(1, hinted).err(), Some(FillError::Cancelled));
        let mut hinted = command(fresh, remembered.clone(), captured);
        hinted.action = UserFillAction::ConfirmedHint;
        let (_, grant) = permit();
        let request = worker.fill_hint(1, hinted, grant).unwrap();
        let AccountResult::Filled(correct_focus) = wait(&worker, request).result else {
            panic!("hint fill result missing")
        };
        assert!(correct_focus.password_written);
        let target_id = worker
            .inspect_binding(1, captured, remembered.clone())
            .unwrap();
        assert!(matches!(
            wait(&worker, target_id).result,
            AccountResult::Inspected(_)
        ));
        SetWindowTextW(first.server, w!("db-b")).unwrap();
        let changed_fill = worker
            .fill(1, command(target_id, remembered, captured))
            .unwrap();
        let AccountResult::Filled(changed) = wait(&worker, changed_fill).result else {
            panic!("synthetic discriminator result missing")
        };
        assert!(!changed.password_written);
        let mut picker = pick(
            second.root.0 as usize as u64,
            Some(first.identity()),
            PickRole::Password,
        )
        .unwrap();
        assert_eq!(picker.cancel(), PickEvent::Cancelled);
        let id = worker.inspect(1, captured).unwrap();
        worker.invalidate(2);
        let end = Instant::now() + Duration::from_secs(10);
        while worker.is_busy() {
            pump();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(worker.try_recv().is_none());
        assert!(worker.inspect(1, captured).is_err());
        let _ = id;
        // Destroying/rebuilding a control invalidates the request-local RuntimeId snapshot.
        let id = worker.inspect(2, captured).unwrap();
        let AccountResult::Inspected(inspected) = wait(&worker, id).result else {
            panic!("synthetic inspect failed")
        };
        DestroyWindow(first.password).unwrap();
        let fill = worker
            .fill(2, command(id, draft(&inspected), captured))
            .unwrap();
        let AccountResult::Filled(result) = wait(&worker, fill).result else {
            panic!("synthetic fill result missing")
        };
        assert!(!result.password_written);
        let ambiguous = Fixture::new(50);
        ambiguous.activate();
        SetWindowLongPtrW(ambiguous.user, GWLP_ID, 0);
        SetWindowLongPtrW(ambiguous.password, GWLP_ID, 0);
        let id = worker.inspect(2, ambiguous.identity()).unwrap();
        let AccountResult::Inspected(inspected) = wait(&worker, id).result else {
            panic!("synthetic ambiguous inspect failed")
        };
        assert!(inspected.teaching_required);
        let mut rect = RECT::default();
        GetWindowRect(ambiguous.password, &mut rect).unwrap();
        let taught_id = worker
            .teach_point(
                2,
                Some(id),
                PickPoint {
                    window: ambiguous.identity(),
                    x: rect.left + 10,
                    y: rect.top + 10,
                    role: PickRole::Password,
                },
            )
            .unwrap();
        let AccountResult::Taught(taught) = wait(&worker, taught_id).result else {
            panic!("synthetic point teaching failed")
        };
        assert!(taught.inspection.manual_only);
        assert_eq!(
            taught.inspection.snapshot.scope,
            SceneScope::SelectedContext
        );
        let mut teaching = TeachingSession::begin(&taught.inspection.snapshot).unwrap();
        teaching.select_password(taught.field_id).unwrap();
        teaching.select_username(None).unwrap();
        teaching.select_target(None).unwrap();
        let once = teaching
            .finish_once("once".into(), "account".into())
            .unwrap();
        let fill = worker
            .fill(
                2,
                FillCommand {
                    inspection_id: taught_id,
                    expected_window: ambiguous.identity(),
                    choice: FillChoice::Once(once),
                    settings: AccountFillSettings {
                        enabled: true,
                        ..Default::default()
                    },
                    action: UserFillAction::SelectedAccount,
                    username: Secret::new(String::new()),
                    password: Secret::new("synthetic-once".into()),
                },
            )
            .unwrap();
        let AccountResult::Filled(result) = wait(&worker, fill).result else {
            panic!("synthetic one-time result missing")
        };
        assert!(result.password_written, "{result:?}");
        assert_eq!(ambiguous.password_text(), "synthetic-once");
    }
}
