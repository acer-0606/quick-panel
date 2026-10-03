use quick_panel::{embedding_jobs::*, semantic::*};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
fn request(text: &str) -> EmbeddingRequest {
    EmbeddingRequest {
        settings: EmbeddingSettings {
            enabled: true,
            model: "fixture-model".into(),
            encrypted_key: vec![1],
            ..Default::default()
        },
        inputs: vec![text.into()],
    }
}
#[test]
fn latest_pending_request_replaces_older_work_and_only_latest_publishes() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (notified, notices) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = EmbeddingWorker::start(
        move |request| {
            started.send(request.inputs).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(vec![vec![1.0, 0.0]])
        },
        Arc::new(move || {
            notified.send(()).unwrap();
        }),
    );
    worker.submit(request("A")).unwrap();
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), ["A"]);
    worker.submit(request("B")).unwrap();
    let latest = worker.submit(request("C")).unwrap();
    release.send(()).unwrap();
    assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), ["C"]);
    assert!(worker.try_result().is_none());
    assert!(notices.try_recv().is_err());
    release.send(()).unwrap();
    notices.recv_timeout(Duration::from_secs(2)).unwrap();
    let completion = worker.try_result().unwrap();
    assert_eq!(completion.id, latest);
    assert_eq!(completion.result.unwrap(), vec![vec![1.0, 0.0]]);
    assert!(worker.try_result().is_none());
    assert!(starts.try_recv().is_err());
}
#[test]
fn cancel_and_drop_return_while_network_is_blocked_and_suppress_late_publication() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    let (notified, notices) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = EmbeddingWorker::start(
        move |_| {
            started.send(()).unwrap();
            releases.lock().unwrap().recv().unwrap();
            finished.send(()).unwrap();
            Ok(vec![vec![1.0]])
        },
        Arc::new(move || {
            let _ = notified.send(());
        }),
    );
    worker.submit(request("A")).unwrap();
    starts.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.submit(request("B")).unwrap();
    let begin = Instant::now();
    worker.cancel();
    assert!(begin.elapsed() < Duration::from_millis(200));
    assert!(worker.try_result().is_none());
    let begin = Instant::now();
    drop(worker);
    assert!(begin.elapsed() < Duration::from_millis(200));
    release.send(()).unwrap();
    finishes.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(notices.recv_timeout(Duration::from_millis(200)).is_err());
    assert!(starts.try_recv().is_err());
}
#[test]
fn admission_rejects_bad_settings_and_unbounded_inputs_before_transport() {
    let worker = EmbeddingWorker::start(
        |_| panic!("rejected request reached transport"),
        Arc::new(|| {}),
    );
    for text in ["", " ", "x\0y"] {
        assert!(worker.submit(request(text)).is_err());
    }
    assert!(worker.submit(request(&"中".repeat(4097))).is_err());
    let mut bad = request("x");
    bad.inputs = vec!["x".into(); 17];
    assert!(worker.submit(bad).is_err());
    let mut bad = request("x");
    bad.inputs.clear();
    assert!(worker.submit(bad).is_err());
    let mut bad = request("x");
    bad.settings.enabled = false;
    assert!(worker.submit(bad).is_err());
    let mut bad = request("x");
    bad.settings.encrypted_key.clear();
    assert!(worker.submit(bad).is_err());
    let mut bad = request("x");
    bad.settings.encrypted_key = vec![0; 16 * 1024 + 1];
    assert!(worker.submit(bad).is_err());
    let mut bad = request("x");
    bad.settings.endpoint = "https://user:secret@fixture.invalid".into();
    assert!(worker.submit(bad).is_err());
}
#[test]
fn runner_failure_is_delivered_as_latest_completion() {
    let (notified, notices) = mpsc::channel();
    let worker = EmbeddingWorker::start(
        |_| Err("受控失败".into()),
        Arc::new(move || {
            notified.send(()).unwrap();
        }),
    );
    let id = worker.submit(request("A")).unwrap();
    notices.recv_timeout(Duration::from_secs(2)).unwrap();
    let completion = worker.try_result().unwrap();
    assert_eq!(completion.id, id);
    assert_eq!(completion.result.unwrap_err(), "受控失败");
}

#[test]
fn cancellation_suppresses_inflight_result_and_worker_accepts_fresh_work() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (notified, notices) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = EmbeddingWorker::start(
        move |request| {
            started.send(request.inputs).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(vec![vec![1.0]])
        },
        Arc::new(move || {
            notified.send(()).unwrap();
        }),
    );
    worker.submit(request("cancelled")).unwrap();
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        ["cancelled"]
    );
    worker.cancel();
    release.send(()).unwrap();
    assert!(notices.recv_timeout(Duration::from_millis(200)).is_err());
    assert!(worker.try_result().is_none());
    let fresh = worker.submit(request("fresh")).unwrap();
    assert_eq!(
        starts.recv_timeout(Duration::from_secs(2)).unwrap(),
        ["fresh"]
    );
    release.send(()).unwrap();
    notices.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(worker.try_result().unwrap().id, fresh);
}
