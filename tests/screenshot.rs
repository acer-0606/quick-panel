use quick_panel::{background_job::BackgroundWorker, screenshot::*};
use std::sync::{Arc, mpsc};

#[test]
fn reverse_drag_keeps_negative_screen_coordinates_and_clips_to_desktop() {
    let desktop = Rect {
        left: -1920,
        top: -200,
        right: 3840,
        bottom: 2160,
    };
    assert_eq!(
        selection((-100, 50), (-500, 300), desktop).unwrap(),
        Some(Rect {
            left: -500,
            top: 50,
            right: -100,
            bottom: 300,
        })
    );
    let rect = selection((i32::MIN, -500), (i32::MAX, 250), desktop)
        .unwrap()
        .unwrap();
    assert_eq!(
        rect,
        Rect {
            left: -1920,
            top: -200,
            right: 3840,
            bottom: 250
        }
    );
    assert_eq!(rect.dimensions().unwrap(), (5760, 450));
}

#[test]
fn empty_or_outside_selection_is_cancelled() {
    let desktop = Rect {
        left: -100,
        top: 0,
        right: 100,
        bottom: 100,
    };
    for (start, end) in [
        ((0, 0), (0, 50)),
        ((0, 1), (50, 1)),
        ((200, 200), (300, 300)),
    ] {
        assert_eq!(selection(start, end, desktop).unwrap(), None);
    }
}

#[test]
fn selected_pixel_budget_accepts_boundary_and_rejects_overflow() {
    assert_eq!(pixel_bytes(4096, 4096).unwrap(), 67_108_864);
    assert!(pixel_bytes(4097, 4096).is_err());
    assert!(pixel_bytes(u32::MAX, u32::MAX).is_err());
    assert!(pixel_bytes(0, 1).is_err());
    let huge = Rect {
        left: i32::MIN,
        top: i32::MIN,
        right: i32::MAX,
        bottom: i32::MAX,
    };
    assert!(selection((i32::MIN, i32::MIN), (i32::MAX, i32::MAX), huge).is_err());
}

#[test]
fn cancel_and_exit_revoke_delivery_without_releasing_actual_receipt_slot() {
    for exit in [false, true] {
        let mut gate = ScreenshotGate::default();
        let ticket = gate.begin(10).unwrap();
        assert!(gate.encoding(&ticket));
        if exit {
            gate.close();
        } else {
            gate.cancel();
        }
        assert!(!ticket.may_write(10));
        assert!(gate.busy());
        assert!(gate.begin(11).is_none());
        assert!(gate.finish(&ticket));
        assert!(!gate.busy());
        assert_eq!(gate.begin(11).is_some(), !exit);
    }
}

#[test]
fn new_clipboard_sequence_blocks_encoding_result_and_stale_receipt_cannot_release_new_task() {
    let mut gate = ScreenshotGate::default();
    let old = gate.begin(20).unwrap();
    assert!(gate.encoding(&old));
    assert!(old.may_write(20));
    assert!(!old.may_write(21));
    assert!(gate.finish(&old));
    assert!(!old.may_write(20));
    let new = gate.begin(22).unwrap();
    assert!(!gate.finish(&old));
    assert!(gate.busy());
    assert!(new.may_write(22));
}

#[test]
fn cancelled_selection_and_failed_submission_allow_retry() {
    let mut gate = ScreenshotGate::default();
    let selection = gate.begin(1).unwrap();
    gate.cancel();
    assert!(!gate.encoding(&selection));
    assert!(gate.finish(&selection));
    let submission = gate.begin(2).unwrap();
    assert!(gate.encoding(&submission));
    assert!(gate.finish(&submission));
    assert!(gate.begin(3).is_some());
}

#[test]
fn controlled_encoder_keeps_occupied_until_cancelled_failure_receipt_is_consumed() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel();
    let worker = BackgroundWorker::new(
        move |ticket: ScreenshotTicket| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            (ticket, Err::<Vec<u8>, _>("encoding failed"))
        },
        Arc::new(move || {
            wake_tx.send(()).unwrap();
        }),
    );
    let mut gate = ScreenshotGate::default();
    let ticket = gate.begin(7).unwrap();
    assert!(gate.encoding(&ticket));
    assert!(worker.submit(ticket));
    started_rx.recv().unwrap();
    gate.cancel();
    assert!(gate.busy());
    assert!(worker.busy());
    assert!(gate.begin(8).is_none());
    release_tx.send(()).unwrap();
    wake_rx.recv().unwrap();
    assert!(worker.busy());
    let (ticket, failure) = worker.try_result().unwrap();
    assert!(failure.is_err());
    assert!(!ticket.may_write(7));
    assert!(gate.finish(&ticket));
    assert!(!worker.busy());
    assert!(gate.begin(8).is_some());
}
