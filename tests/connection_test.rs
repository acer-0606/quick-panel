use quick_panel::connection_test::{
    ConnectionTestGate, ConnectionTestWorker, CredentialSource, credential_source,
};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(3);

// A transport panic must not leave the actual occupied slot stuck forever.
#[test]
fn panic_produces_terminal_error_and_releases_real_slot() {
    let (done, received) = mpsc::channel();
    let worker = quick_panel::organizer_ui::request_worker(
        |_: ()| -> Result<(), String> { panic!("fixture panic") },
        Arc::new(move || {
            done.send(()).unwrap();
        }),
    );
    worker.submit(()).unwrap();
    received.recv_timeout(WAIT).unwrap();
    assert!(!worker.busy());
    assert!(worker.try_result().unwrap().result.is_err());
}

// Removing the active-slot check would run a second request before the first finishes.
#[test]
fn one_click_runs_off_thread_and_in_flight_clicks_are_rejected() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    let releases = Mutex::new(releases);
    let ui_thread = std::thread::current().id();
    let worker = ConnectionTestWorker::new(
        move |input: String| {
            started.send((input, std::thread::current().id())).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(3usize)
        },
        Arc::new(move || finished.send(()).unwrap()),
    );
    assert!(
        starts.try_recv().is_err(),
        "opening a form must not execute a test"
    );
    let id = worker.submit("edited model".into()).unwrap();
    let (input, network_thread) = starts.recv_timeout(WAIT).unwrap();
    assert_eq!(input, "edited model");
    assert_ne!(network_thread, ui_thread);
    assert!(worker.busy());
    assert!(worker.submit("duplicate".into()).is_err());
    assert!(starts.try_recv().is_err());
    release.send(()).unwrap();
    finishes.recv_timeout(WAIT).unwrap();
    assert!(!worker.busy());
    let result = worker.try_result().unwrap();
    assert_eq!(result.id, id);
    assert_eq!(result.result.unwrap(), 3);
    assert!(worker.try_result().is_none());
}

// Clearing the running slot on cancellation would allow overlapping paid calls.
#[test]
fn cancellation_discards_old_result_and_preserves_real_in_flight_slot() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = ConnectionTestWorker::new(
        move |value: usize| {
            started.send(value).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(value)
        },
        Arc::new(move || finished.send(()).unwrap()),
    );
    let old = worker.submit(10).unwrap();
    assert_eq!(starts.recv_timeout(WAIT).unwrap(), 10);
    worker.cancel(old);
    assert!(worker.busy());
    assert!(worker.submit(20).is_err());
    release.send(()).unwrap();
    finishes.recv_timeout(WAIT).unwrap();
    assert!(worker.try_result().is_none());
    let new = worker.submit(20).unwrap();
    assert_ne!(old, new);
    assert_eq!(starts.recv_timeout(WAIT).unwrap(), 20);
    worker.cancel(old); // A closed form cannot cancel a later form's request.
    release.send(()).unwrap();
    finishes.recv_timeout(WAIT).unwrap();
    let result = worker.try_result().unwrap();
    assert_eq!(result.id, new);
    assert_eq!(result.result.unwrap(), 20);
}

// Joining a transport thread during Drop would prevent the UI from closing this form.
#[test]
fn dropping_worker_returns_before_blocked_transport_is_released() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    let (dropped, drops) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = ConnectionTestWorker::new(
        move |_: ()| {
            started.send(()).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(())
        },
        Arc::new(move || finished.send(()).unwrap()),
    );
    worker.submit(()).unwrap();
    starts.recv_timeout(WAIT).unwrap();
    let dropper = std::thread::spawn(move || {
        drop(worker);
        dropped.send(()).unwrap();
    });
    let returned = drops.recv_timeout(WAIT);
    release.send(()).unwrap();
    finishes.recv_timeout(WAIT).unwrap();
    dropper.join().unwrap();
    returned.expect("form close joined the blocked connection test");
}

// Accepting any completion ID would show a cancelled form's result in the next form.
#[test]
fn gate_rejects_stale_cancelled_and_repeated_completions() {
    let mut gate = ConnectionTestGate::default();
    gate.begin(11);
    assert!(gate.busy());
    assert!(!gate.finish(10));
    assert!(gate.busy());
    assert_eq!(gate.cancel(), Some(11));
    assert!(!gate.finish(11));
    gate.begin(12);
    assert!(!gate.finish(11));
    assert!(gate.finish(12));
    assert!(!gate.finish(12));
    assert!(!gate.busy());
}

// A clear-key branch falling back to DPAPI would silently send a credential being removed.
#[test]
fn credential_policy_uses_current_password_and_never_reuses_a_cleared_key() {
    assert_eq!(
        credential_source(true, false, true).unwrap(),
        CredentialSource::Entered
    );
    assert_eq!(
        credential_source(true, false, false).unwrap(),
        CredentialSource::Entered
    );
    assert_eq!(
        credential_source(false, false, true).unwrap(),
        CredentialSource::Saved
    );
    for (entered, clear, saved) in [
        (false, false, false),
        (false, true, true),
        (true, true, true),
    ] {
        assert!(credential_source(entered, clear, saved).is_err());
    }
}
