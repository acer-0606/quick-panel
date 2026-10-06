use quick_panel::{engine::Engine, model::*, organizer::*, organizer_ui::*};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const WAIT: Duration = Duration::from_secs(5);
fn event(e: &Engine, accept: impl Fn(&Event) -> bool) -> Event {
    let end = Instant::now() + WAIT;
    loop {
        match e.try_recv() {
            Ok(v) if accept(&v) => return v,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            Ok(_) => {}
            Err(mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < end, "actor timeout");
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("{error}"),
        }
    }
}
fn snapshot(e: &Engine) -> Snapshot {
    e.send(Command::OrganizerSnapshot { request_id: 900 })
        .unwrap();
    match event(e, |v| {
        matches!(
            v,
            Event::OrganizerState {
                request_id: 900,
                ..
            }
        )
    }) {
        Event::OrganizerState { snapshot, .. } => snapshot,
        _ => unreachable!(),
    }
}
fn captured(e: &Engine, body: &str) {
    e.send(Command::Capture(ClipboardSnapshot {
        text: body.into(),
        representations: vec![],
        source_app: "fixture.exe".into(),
        captured_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
    }))
    .unwrap();
}
fn work(e: &Engine) -> Work {
    e.send(Command::OrganizerTick).unwrap();
    match event(e, |v| matches!(v, Event::OrganizerWork(_))) {
        Event::OrganizerWork(work) => work,
        _ => unreachable!(),
    }
}
fn configuration() -> Settings {
    let mut s = Settings::default();
    s.ai.model = "fixture".into();
    s.ai.encrypted_key = vec![1];
    s.organizer.enabled = true;
    s.organizer.scope = OrganizerScope::AllTextAndPaths;
    s
}
fn preserve_form(s: &Settings, ai: bool) -> Settings {
    let mut saved = s.clone();
    if ai {
        saved.ai = parse_ai_fields(
            &s.ai,
            &["0", &s.ai.endpoint, &s.ai.model, "0", "", "0", "0"].map(str::to_owned),
        )
        .unwrap();
    } else {
        saved.organizer =
            parse_organizer_fields(&["1", "0", "3", "60", "4", "", "1", "4"].map(str::to_owned))
                .unwrap();
    }
    saved
}

// The real shared coordinator must preserve completion across unchanged AI/organizer saves; suppressing it leaves the actor in flight.
#[test]
fn unchanged_settings_forms_preserve_blocked_work_completion_and_next_batch() {
    for ai in [true, false] {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::start(EngineOptions::new(d.path().join("db")), Arc::new(|| {})).unwrap();
        event(&e, |v| matches!(v, Event::Ready(_)));
        let s = configuration();
        e.send(Command::SaveSettings(s.clone())).unwrap();
        event(&e, |v| matches!(v, Event::Saved(_)));
        captured(&e, "first");
        let first = work(&e);
        let (started, starts) = mpsc::channel();
        let (release, releases) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let releases = Mutex::new(releases);
        let worker = request_worker(
            move |work: Work| {
                started.send(work.token).unwrap();
                releases.lock().unwrap().recv().unwrap();
                Ok((
                    work.token,
                    work.candidates
                        .into_iter()
                        .map(|candidate| Proposal {
                            candidate,
                            alias: Some("归档".into()),
                            tags: vec![],
                            reason: "上下文".into(),
                        })
                        .collect::<Vec<_>>(),
                ))
            },
            Arc::new(move || done.send(()).unwrap()),
        );
        let mut session = OrganizerSession::default();
        let id = worker.submit(first.clone()).unwrap();
        session.begin(id, first.token);
        assert_eq!(starts.recv_timeout(WAIT).unwrap(), first.token);
        let next = preserve_form(&s, ai);
        if let Some(id) = session.settings_changed(&s, &next) {
            worker.cancel(id);
        }
        e.send(Command::SaveSettings(next)).unwrap();
        event(&e, |v| matches!(v, Event::Saved(_)));
        assert!(worker.busy());
        assert!(worker.submit(first.clone()).is_err());
        release.send(()).unwrap();
        completed.recv_timeout(WAIT).unwrap();
        let receipt = worker
            .try_result()
            .expect("unchanged save discarded physical completion");
        let token = session
            .finish(receipt.id)
            .expect("unchanged save revoked actor completion");
        let (_, proposals) = receipt.result.unwrap();
        e.send(Command::OrganizerComplete {
            token,
            result: Ok(proposals),
        })
        .unwrap();
        let state = snapshot(&e);
        assert!(!state.in_flight);
        assert_eq!(state.proposals.len(), 1);
        assert_eq!(state.failures, 0);
        // Reset only the persisted scheduling deadline after asserting the actor's first Work completed.
        captured(&e, "second");
        e.send(Command::OrganizerRetry).unwrap();
        event(&e, |v| matches!(v, Event::Saved(_)));
        let second = work(&e);
        let id = worker.submit(second.clone()).unwrap();
        session.begin(id, second.token);
        assert_eq!(starts.recv_timeout(WAIT).unwrap(), second.token);
        release.send(()).unwrap();
        completed.recv_timeout(WAIT).unwrap();
        let receipt = worker.try_result().unwrap();
        let token = session.finish(receipt.id).unwrap();
        let (_, proposals) = receipt.result.unwrap();
        e.send(Command::OrganizerComplete {
            token,
            result: Ok(proposals),
        })
        .unwrap();
        assert_eq!(snapshot(&e).proposals.len(), 2);
        captured(&e, "third");
        e.send(Command::OrganizerRetry).unwrap();
        event(&e, |v| matches!(v, Event::Saved(_)));
        let third = work(&e);
        let id = worker.submit(third.clone()).unwrap();
        session.begin(id, third.token);
        assert_eq!(starts.recv_timeout(WAIT).unwrap(), third.token);
        let mut changed = s.clone();
        if ai {
            changed.ai.model = "changed-model".into();
        } else {
            changed.organizer.instructions = "新说明".into();
        }
        let revoked = session.settings_changed(&s, &changed).unwrap();
        worker.cancel(revoked);
        e.send(Command::SaveSettings(changed)).unwrap();
        event(&e, |v| matches!(v, Event::Saved(_)));
        assert!(worker.busy());
        assert!(worker.submit(third.clone()).is_err());
        let current = snapshot(&e);
        assert!(!current.in_flight);
        assert_eq!(current.queued, 1);
        assert_eq!(current.failures, 0);
        e.send(Command::OrganizerComplete {
            token: third.token,
            result: Ok(third
                .candidates
                .iter()
                .map(|candidate| Proposal {
                    candidate: candidate.clone(),
                    alias: None,
                    tags: vec![],
                    reason: "旧结果".into(),
                })
                .collect()),
        })
        .unwrap();
        assert_eq!(snapshot(&e).proposals.len(), 2);
        release.send(()).unwrap();
        completed.recv_timeout(WAIT).unwrap();
        assert!(worker.try_result().is_none());
        assert!(session.finish(id).is_none());
        assert!(!worker.busy());
        let fourth = work(&e);
        assert_ne!(fourth.token, third.token);
        let id = worker.submit(fourth.clone()).unwrap();
        session.begin(id, fourth.token);
        assert_eq!(starts.recv_timeout(WAIT).unwrap(), fourth.token);
        release.send(()).unwrap();
        completed.recv_timeout(WAIT).unwrap();
        let receipt = worker.try_result().unwrap();
        let token = session.finish(receipt.id).unwrap();
        let (_, proposals) = receipt.result.unwrap();
        e.send(Command::OrganizerComplete {
            token,
            result: Ok(proposals),
        })
        .unwrap();
        let current = snapshot(&e);
        assert_eq!(current.proposals.len(), 3);
        assert_eq!(current.failures, 0);
        assert!(!current.in_flight);
        e.send(Command::Shutdown).unwrap();
        event(&e, |v| matches!(v, Event::ExitReady));
    }
}

// Actual config changes revoke logical publication while the physical worker remains occupied and no synthetic failure is sent.
#[test]
fn changed_settings_revoke_completion_without_releasing_real_worker_slot() {
    let (started, starts) = mpsc::channel();
    let (release, releases) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let releases = Mutex::new(releases);
    let worker = request_worker(
        move |value: u64| {
            started.send(value).unwrap();
            releases.lock().unwrap().recv().unwrap();
            Ok(value)
        },
        Arc::new(move || done.send(()).unwrap()),
    );
    let mut session = OrganizerSession::default();
    let id = worker.submit(11).unwrap();
    session.begin(id, 11);
    starts.recv_timeout(WAIT).unwrap();
    let old = configuration();
    let mut new = old.clone();
    new.organizer.instructions = "新说明".into();
    let revoked = session.settings_changed(&old, &new).unwrap();
    worker.cancel(revoked);
    assert!(worker.busy());
    assert!(worker.submit(22).is_err());
    assert!(session.finish(id).is_none());
    release.send(()).unwrap();
    completed.recv_timeout(WAIT).unwrap();
    assert!(worker.try_result().is_none());
    assert!(!worker.busy());
}
