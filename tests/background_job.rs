use quick_panel::background_job::{ActionPermit, BackgroundWorker};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
#[test]
fn latest_worker_runs_one_inflight_and_only_latest_pending_job() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let worker = BackgroundWorker::new(
        move |job: u32| {
            started_tx.send(job).unwrap();
            release_rx.recv().unwrap();
            job
        },
        Arc::new(|| {}),
    );
    worker.replace(1);
    assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
    for id in 2..=1000 {
        worker.replace(id);
    }
    assert!(started_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert_eq!(
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        1000
    );
    release_tx.send(()).unwrap();
}
#[test]
fn reliable_action_rejects_duplicate_until_receipt_is_consumed() {
    let (release_tx, release_rx) = mpsc::channel();
    let worker = BackgroundWorker::new(
        move |job: u32| {
            release_rx.recv().unwrap();
            job
        },
        Arc::new(|| {}),
    );
    assert!(worker.submit(1));
    assert!(!worker.submit(2));
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(result) = worker.try_result() {
            assert_eq!(result, 1);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(worker.submit(3));
    release_tx.send(()).unwrap();
}
#[test]
fn closing_never_waits_for_blocked_path_and_revokes_late_execution() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let permit = ActionPermit::default();
    let ticket = permit.begin(1);
    let (committed_tx, committed_rx) = mpsc::channel();
    let worker = BackgroundWorker::new(
        move |_: u32| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let result = ticket.commit();
            committed_tx.send(result).unwrap();
            result
        },
        Arc::new(|| {}),
    );
    worker.submit(1);
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    permit.cancel();
    let start = Instant::now();
    drop(worker);
    assert!(start.elapsed() < Duration::from_millis(200));
    release_tx.send(()).unwrap();
    assert!(!committed_rx.recv_timeout(Duration::from_secs(1)).unwrap());
}
#[test]
fn cancelled_action_cannot_commit_but_executed_receipt_survives_later_cancel() {
    let permit = ActionPermit::default();
    let old = permit.begin(1);
    permit.cancel();
    assert!(!old.commit());
    let executed = permit.begin(2);
    assert!(executed.commit());
    permit.cancel();
    assert!(executed.executed());
}

#[test]
fn cancelled_load_keeps_actual_occupancy_and_ignores_unrelated_receipts() {
    use quick_panel::background_job::ActionLoads;
    let mut loads = ActionLoads::default();
    assert!(loads.begin(1));
    assert!(!loads.begin(2));
    loads.complete(99);
    assert!(loads.busy());
    loads.complete(1);
    assert!(!loads.busy());
    assert!(loads.begin(3));
}
