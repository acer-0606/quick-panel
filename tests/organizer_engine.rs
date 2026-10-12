use quick_panel::{engine::Engine, model::*, organizer::*, path_activity::*};
use rusqlite::Connection;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// The actual actor batch must fit the double-encoded Messages body before it
// leaves the actor; local shrinking must never consume interface failures.
#[test]
fn wire_budget_dispatches_maximum_prefix_and_later_queued_sources_without_failures() {
    for escaped in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let e = open(&d.path().join("wire-db"));
        let mut s = config(OrganizerMode::Suggest);
        s.organizer.batch_size = 8;
        s.organizer.instructions = "中😀\"\\".repeat(128);
        s.ai.effort = quick_panel::ai::Effort::XHigh;
        settings(&e, s);
        for i in 0..8 {
            let unit = if escaped {
                "😀\"\\中"
            } else {
                "目目目目"
            };
            e.send(Command::SaveFavorite(FavoriteInput {
                id: Some(format!("wire-{i}")),
                kind: ItemKind::Folder,
                title: unit.repeat(64),
                target: format!("C:/{}", "😀\\中".repeat(341)),
                alias: unit.repeat(64),
                tags: (0..32)
                    .map(|j| format!("{j:02}{}", unit.repeat(15) + "😀中"))
                    .collect(),
                open_with: None,
            }))
            .unwrap();
            wait(&e, |v| matches!(v, Event::Saved(_)));
        }
        let mut seen = std::collections::HashSet::new();
        loop {
            let w = work(&e);
            let body = quick_panel::organizer::prepare(
                &w.ai_settings,
                "fixture-key",
                &w.organizer_settings,
                &w.candidates,
            )
            .expect("actor published a locally oversized request")
            .body;
            assert!(body.len() <= 64 * 1024);
            let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let payload: serde_json::Value =
                serde_json::from_str(envelope["messages"][0]["content"].as_str().unwrap()).unwrap();
            assert_eq!(payload["items"][0]["title"], w.candidates[0].title);
            for c in &w.candidates {
                assert!(seen.insert(c.source.clone()));
            }
            let during = snapshot(&e);
            assert_eq!(during.failures, 0);
            assert_eq!(during.queued + seen.len(), 8);
            if seen.len() == w.candidates.len() {
                assert!(
                    seen.len() < 8,
                    "fixture must require multiple real actor batches"
                );
                let mut larger = w.candidates.clone();
                let mut extra = larger[0].clone();
                extra.source = Source::SavedItem {
                    id: "next-legal-source".into(),
                };
                larger.push(extra);
                assert!(
                    quick_panel::organizer::prepare(
                        &w.ai_settings,
                        "fixture-key",
                        &w.organizer_settings,
                        &larger
                    )
                    .is_err(),
                    "actor did not retain the maximum sendable prefix"
                );
            }
            complete(&e, &w);
            let after = snapshot(&e);
            assert_eq!(after.failures, 0);
            assert!(!after.paused && after.retry_at_ms.is_none());
            assert_eq!(after.proposals.len(), seen.len());
            if seen.len() == 8 {
                break;
            }
            // Advance the existing explicit retry scheduling reset after a
            // successful batch rather than waiting a real minute in a fixture.
            e.send(Command::OrganizerRetry).unwrap();
            snapshot(&e);
        }
        stop(&e);
    }
}

// Invalid local configuration must not publish failing work or spend API
// failures; fixing it must preserve and resume the original queued source.
#[test]
fn wire_preflight_keeps_sources_and_failures_when_local_configuration_is_invalid() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("invalid-config"));
    let mut s = config(OrganizerMode::Suggest);
    for invalid_model in [true, false] {
        s.ai.model = if invalid_model {
            "x".repeat(257)
        } else {
            "fixture".into()
        };
        s.ai.endpoint = if invalid_model {
            "https://example.com"
        } else {
            "invalid://endpoint"
        }
        .into();
        settings(&e, s.clone());
        favorite(&e, "original", "人工别名", vec!["人工标签".into()]);
        e.send(Command::OrganizerTick).unwrap();
        e.send(Command::OrganizerSnapshot { request_id: 987 })
            .unwrap();
        let Event::OrganizerState { snapshot, .. } = wait(&e, |event| {
            assert!(
                !matches!(event, Event::OrganizerWork(_)),
                "invalid local configuration dispatched failing work"
            );
            matches!(
                event,
                Event::OrganizerState {
                    request_id: 987,
                    ..
                }
            )
        }) else {
            unreachable!()
        };
        assert_eq!(snapshot.queued, 1);
        assert_eq!(snapshot.failures, 0);
        assert!(!snapshot.in_flight && !snapshot.saturated && snapshot.retry_at_ms.is_none());
    }
    s.ai.model = "fixture".into();
    s.ai.endpoint = "https://example.com".into();
    settings(&e, s);
    let w = work(&e);
    assert_eq!(w.candidates[0].alias, "人工别名");
    assert_eq!(w.candidates[0].tags, ["人工标签"]);
    complete(&e, &w);
    assert_eq!(snapshot(&e).failures, 0);
    stop(&e);
}

// Metadata outside the organizer's boundary still belongs to the main history;
// it must not starve the next valid source or be silently erased/truncated.
#[test]
fn invalid_organizer_source_boundary_preserves_favorite_and_allows_later_valid_work() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("boundary-db");
    let e = open(&path);
    settings(&e, config(OrganizerMode::Suggest));
    let alias = "中".repeat(257);
    favorite(&e, "oversized", &alias, vec![]);
    favorite(&e, "valid", "原别名", vec![]);
    let w = work(&e);
    assert_eq!(w.candidates.len(), 1);
    assert_eq!(
        w.candidates[0].source,
        Source::SavedItem { id: "valid".into() }
    );
    complete(&e, &w);
    let state = snapshot(&e);
    assert_eq!(state.failures, 0);
    assert_eq!(state.proposals.len(), 1);
    stop(&e);
    let db = Connection::open(&path).unwrap();
    let saved_alias: String = db
        .query_row(
            "SELECT json_extract(summary,'$.alias') FROM items WHERE id='oversized'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved_alias, alias);
}

// An unrelated Saved event cannot confirm this decision; its explicit receipt must follow a committed SQLite update.
#[test]
fn reviewed_decision_receipt_confirms_only_its_own_commit_and_reports_stale() {
    use quick_panel::organizer_ui::ReviewDecision;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    capture(&e, "receipt body");
    let w = work(&e);
    complete(&e, &w);
    let proposals = snapshot(&e).proposals;
    e.send(Command::OrganizerReview {
        request_id: 41,
        decision: ReviewDecision::Apply(proposals.clone()),
    })
    .unwrap();
    assert!(matches!(
        wait(&e, |v| matches!(
            v,
            Event::OrganizerReceipt { request_id: 41, .. }
        )),
        Event::OrganizerReceipt {
            result: Ok(()),
            pending_save: false,
            ..
        }
    ));
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT json_extract(summary,'$.alias') FROM items",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "项目资料"
    );
    e.send(Command::OrganizerReview {
        request_id: 42,
        decision: ReviewDecision::Apply(proposals),
    })
    .unwrap();
    assert!(matches!(
        wait(&e, |v| matches!(
            v,
            Event::OrganizerReceipt { request_id: 42, .. }
        )),
        Event::OrganizerReceipt {
            result: Err(_),
            pending_save: false,
            ..
        }
    ));
    stop(&e);
}

// A failed transaction must acknowledge failure for this request, preserve both disk fields and Ready, and allow a correlated flush retry.
#[test]
fn reviewed_failure_receipt_is_not_success_when_ready_has_disappeared_in_ram() {
    use quick_panel::organizer_ui::ReviewDecision;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    favorite(&e, "receipt", "", vec![]);
    let w = work(&e);
    complete(&e, &w);
    let proposals = snapshot(&e).proposals;
    flush(&e);
    let db = Connection::open(&p).unwrap();
    db.execute_batch("CREATE TRIGGER fail_review BEFORE UPDATE ON items BEGIN SELECT RAISE(ABORT,'fixture'); END").unwrap();
    e.send(Command::OrganizerReview {
        request_id: 51,
        decision: ReviewDecision::Apply(proposals),
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Error { .. }));
    assert!(matches!(
        wait(&e, |v| matches!(
            v,
            Event::OrganizerReceipt { request_id: 51, .. }
        )),
        Event::OrganizerReceipt {
            result: Err(_),
            pending_save: true,
            ..
        }
    ));
    assert!(snapshot(&e).proposals.is_empty());
    assert_eq!(
        db.query_row(
            "SELECT json_extract(summary,'$.alias') FROM items WHERE id='receipt'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        ""
    );
    let raw: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    assert!(raw.contains("Ready"));
    db.execute_batch("DROP TRIGGER fail_review").unwrap();
    e.send(Command::OrganizerReview {
        request_id: 52,
        decision: ReviewDecision::SavePending,
    })
    .unwrap();
    assert!(matches!(
        wait(&e, |v| matches!(
            v,
            Event::OrganizerReceipt { request_id: 52, .. }
        )),
        Event::OrganizerReceipt {
            result: Ok(()),
            pending_save: false,
            ..
        }
    ));
    assert_eq!(
        db.query_row(
            "SELECT json_extract(summary,'$.alias') FROM items WHERE id='receipt'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "项目资料"
    );
    stop(&e);
}

// A stale multi-dismiss must fail atomically instead of reporting a no-op as a saved decision.
#[test]
fn reviewed_dismiss_rejects_a_missing_source_without_ignoring_other_ready_rows() {
    use quick_panel::organizer_ui::ReviewDecision;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("db"));
    settings(&e, config(OrganizerMode::Suggest));
    capture(&e, "dismiss fixture");
    let w = work(&e);
    complete(&e, &w);
    let proposals = snapshot(&e).proposals;
    e.send(Command::OrganizerReview {
        request_id: 53,
        decision: ReviewDecision::Dismiss(vec![
            proposals[0].candidate.source.clone(),
            Source::SavedItem {
                id: "missing".into(),
            },
        ]),
    })
    .unwrap();
    assert!(matches!(
        wait(&e, |v| matches!(
            v,
            Event::OrganizerReceipt { request_id: 53, .. }
        )),
        Event::OrganizerReceipt {
            result: Err(_),
            pending_save: false,
            ..
        }
    ));
    assert_eq!(snapshot(&e).proposals.len(), 1);
    stop(&e);
}

// A review detail read must return original case beyond the AI excerpt, without writes or a BLOB load, and reject a changed source version.
#[test]
fn organizer_details_are_versioned_local_metadata_reads_with_full_original_targets() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    let target = format!(
        "C:/MiXeD/{}/Logs",
        (0..8)
            .map(|i| format!("P{i}{}", "a".repeat(190)))
            .collect::<Vec<_>>()
            .join("/")
    );
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some("long".into()),
        kind: ItemKind::Folder,
        title: "long".into(),
        target: target.clone(),
        alias: String::new(),
        tags: vec![],
        open_with: None,
    }))
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    observe(&e, &target);
    let w = work(&e);
    complete(&e, &w);
    let proposals = snapshot(&e).proposals;
    flush(&e);
    let db = Connection::open(&p).unwrap();
    let before: i64 = db
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    for (i, p) in proposals.iter().enumerate() {
        assert_eq!(p.candidate.context.chars().count(), 1024);
        e.send(Command::OrganizerDetail {
            request_id: 70 + i as u64,
            source: p.candidate.source.clone(),
            expected_version: p.candidate.expected_version.clone(),
        })
        .unwrap();
        assert!(
            matches!(wait(&e,|v|matches!(v,Event::OrganizerDetail {request_id,..} if *request_id==70+i as u64)),Event::OrganizerDetail {result:Ok(ref full),..} if full==&target || full==&target.replace('/',"\\"))
        );
    }
    let after: i64 = db
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    let source = proposals
        .iter()
        .find(|p| matches!(&p.candidate.source, Source::SavedItem { .. }))
        .unwrap();
    favorite(&e, "long", "changed", vec![]);
    e.send(Command::OrganizerDetail {
        request_id: 79,
        source: source.candidate.source.clone(),
        expected_version: source.candidate.expected_version.clone(),
    })
    .unwrap();
    assert!(matches!(
        wait(&e, |v| matches!(
            v,
            Event::OrganizerDetail { request_id: 79, .. }
        )),
        Event::OrganizerDetail { result: Err(_), .. }
    ));
    stop(&e);
}
#[test]
fn reading_a_dirty_snapshot_does_not_emit_change_and_retrigger_itself() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("db"));
    settings(&e, config(OrganizerMode::Suggest));
    capture(&e, "snapshot fixture");
    snapshot(&e);
    e.send(Command::OrganizerSnapshot { request_id: 19 })
        .unwrap();
    let mut changed = 0;
    wait(&e, |event| {
        assert!(
            !matches!(event, Event::OrganizerChanged),
            "read-only snapshot must not publish another change"
        );
        matches!(event, Event::OrganizerState { request_id: 19, .. })
    });
    while let Ok(event) = e.try_recv() {
        if matches!(event, Event::OrganizerChanged) {
            changed += 1;
        }
    }
    assert_eq!(changed, 0);
    stop(&e);
}
#[test]
fn empty_ticks_are_read_only_and_do_not_create_the_optional_table() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    for _ in 0..3 {
        e.send(Command::OrganizerTick).unwrap();
        let s = snapshot(&e);
        assert!(!s.saturated);
        assert!(!s.in_flight);
    }
    stop(&e);
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='organizer_state'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn automatic_path_metadata_uses_normal_deadline_without_semantic_invalidation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut options = EngineOptions::new(p.clone());
    options.save_interval_ms = 300;
    let e = Engine::start(options, Arc::new(|| {})).unwrap();
    wait(&e, |v| matches!(v, Event::Ready(_)));
    settings(&e, config(OrganizerMode::Automatic));
    observe(&e, "C:/Deadline");
    let w = work(&e);
    complete(&e, &w);
    snapshot(&e);
    let event = wait(&e, |v| matches!(v, Event::Saved(_)));
    assert!(matches!(event,Event::Saved(s) if !s.semantic_status_changed));
    let db = Connection::open(&p).unwrap();
    let alias: String = db
        .query_row("SELECT alias FROM path_activity", [], |r| r.get(0))
        .unwrap();
    assert_eq!(alias, "项目资料");
    stop(&e);
}

#[test]
fn excluded_application_and_expired_observation_never_produce_new_work() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    let mut s = config(OrganizerMode::Suggest);
    s.excluded_apps = vec!["fixture.exe".into()];
    settings(&e, s);
    capture(&e, "excluded body");
    assert_eq!(snapshot(&e).queued, 0);
    observe(&e, "C:/Expiry");
    let _w = work(&e);
    flush(&e);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute("UPDATE path_activity SET last_seen_ms=0", [])
        .unwrap();
    drop(db);
    let e = open(&p);
    e.send(Command::OrganizerRetry).unwrap();
    e.send(Command::OrganizerTick).unwrap();
    let s = snapshot(&e);
    assert_eq!(s.queued, 0);
    assert!(!s.in_flight);
    stop(&e);
}
#[track_caller]
fn wait(e: &Engine, f: impl Fn(&Event) -> bool) -> Event {
    let start = Instant::now();
    let caller = std::panic::Location::caller();
    let mut observed = Vec::new();
    let end = start + Duration::from_secs(5);
    loop {
        match e.try_recv() {
            Ok(v) if f(&v) => return v,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            Ok(event) => observed.push(format!("{:?}", std::mem::discriminant(&event))),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < end,
                    "timeout caller={caller}, elapsed={:?}, observed event variants={observed:?}",
                    start.elapsed()
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(x) => panic!("{x}"),
        }
    }
}
fn progress(e: &Engine) -> quick_panel::organizer_display::Progress {
    e.send(Command::OrganizerProgress { request_id: 918 })
        .unwrap();
    match wait(e, |v| {
        matches!(
            v,
            Event::OrganizerProgress {
                request_id: 918,
                ..
            }
        )
    }) {
        Event::OrganizerProgress { progress, .. } => progress,
        _ => unreachable!(),
    }
}

// Status reads must describe only accepted work and cannot commit pending data.
#[test]
fn progress_tracks_real_queue_without_marking_old_or_out_of_scope_content() {
    use quick_panel::organizer_display::Stage;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("progress-db");
    let e = open(&path);
    favorite(&e, "old", "", vec![]);
    let mut s = config(OrganizerMode::Suggest);
    s.organizer.scope = OrganizerScope::SavedAndPaths;
    settings(&e, s.clone());
    capture(&e, "ordinary clipboard outside organizer scope");
    assert!(progress(&e).items.is_empty());
    favorite(&e, "new", "", vec![]);
    let source = Source::SavedItem { id: "new".into() };
    assert_eq!(progress(&e).items, [(source.clone(), Stage::Waiting)]);
    let db = Connection::open(&path).unwrap();
    let version: i64 = db
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    let w = work(&e);
    assert_eq!(progress(&e).items, [(source.clone(), Stage::Processing)]);
    assert_eq!(load(&e, "new").summary.alias, "");
    complete(&e, &w);
    for _ in 0..5 {
        assert_eq!(progress(&e).items, [(source.clone(), Stage::Ready)]);
    }
    assert_eq!(load(&e, "new").summary.alias, "");
    assert_eq!(
        version,
        db.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
            .unwrap()
    );
    s.organizer.enabled = false;
    settings(&e, s.clone());
    assert!(progress(&e).items.is_empty());
    s.organizer.enabled = true;
    s.organizer.scope = OrganizerScope::PathsOnly;
    settings(&e, s);
    assert!(progress(&e).items.is_empty());
    observe(&e, "C:/ObservedCase/Logs");
    assert_eq!(
        progress(&e).items,
        [(
            Source::ObservedPath {
                normalized_path: "c:\\observedcase\\logs".into()
            },
            Stage::Waiting
        )]
    );
    stop(&e);
}

#[test]
fn progress_reports_cooldown_and_resume_without_losing_usable_sources() {
    use quick_panel::organizer_display::Stage;
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("cooldown-db"));
    settings(&e, config(OrganizerMode::Suggest));
    favorite(&e, "original", "", vec![]);
    let w = work(&e);
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Err("fixture failure".into()),
    })
    .unwrap();
    let p = progress(&e);
    assert!(p.cooling);
    assert_eq!(p.items[0].1, Stage::Waiting);
    assert_eq!(
        load(&e, "original").target.as_deref(),
        Some("C:/Fixture/original")
    );
    e.send(Command::OrganizerRetry).unwrap();
    assert!(!progress(&e).cooling);
    let w = work(&e);
    assert_eq!(progress(&e).items[0].1, Stage::Processing);
    complete(&e, &w);
    e.send(Command::OrganizerDismiss {
        sources: vec![w.candidates[0].source.clone()],
    })
    .unwrap();
    assert!(progress(&e).items.is_empty());
    stop(&e);
}

// The UI needs a metadata patch, not a requery notification that reorders its live rows.
#[test]
fn organizer_metadata_is_emitted_as_guarded_patches_instead_of_search_refreshes() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("patch-db"));
    settings(&e, config(OrganizerMode::Automatic));
    favorite(&e, "saved", "", vec![]);
    observe(&e, "C:/OriginalCase/Logs");
    let w = work(&e);
    complete(&e, &w);
    let event = wait(&e, |event| {
        assert!(
            !matches!(event, Event::Changed | Event::PathActivityChanged),
            "AI metadata must not reorder the active query"
        );
        matches!(event, Event::OrganizerMetadataUpdated(_))
    });
    let Event::OrganizerMetadataUpdated(updates) = event else {
        unreachable!()
    };
    assert_eq!(updates.len(), 2);
    for update in &updates {
        assert_eq!(update.previous_alias, "");
        assert!(update.previous_tags.is_empty());
        assert_eq!(update.alias, "项目资料");
        assert_eq!(update.tags, ["工作"]);
    }
    assert!(progress(&e).items.is_empty());
    assert_eq!(load(&e, "saved").summary.alias, "项目资料");
    assert_eq!(
        load(&e, "saved").target.as_deref(),
        Some("C:/Fixture/saved")
    );
    stop(&e);
}
#[track_caller]
fn open(p: &std::path::Path) -> Engine {
    let e = Engine::start(EngineOptions::new(p.into()), Arc::new(|| {})).unwrap();
    wait(&e, |v| matches!(v, Event::Ready(_)));
    e
}
#[track_caller]
fn settings(e: &Engine, s: Settings) {
    e.send(Command::SaveSettings(s)).unwrap();
    wait(e, |v| matches!(v, Event::Saved(_)));
}
fn config(mode: OrganizerMode) -> Settings {
    let mut s = Settings::default();
    s.organizer.enabled = true;
    s.organizer.mode = mode;
    s.organizer.scope = OrganizerScope::AllTextAndPaths;
    s.path_activity.enabled = true;
    s.ai.model = "fixture".into();
    s.ai.encrypted_key = "fixture".into();
    s
}
#[track_caller]
fn snapshot(e: &Engine) -> Snapshot {
    e.send(Command::OrganizerSnapshot { request_id: 17 })
        .unwrap();
    match wait(e, |v| {
        matches!(v, Event::OrganizerState { request_id: 17, .. })
    }) {
        Event::OrganizerState { snapshot, .. } => snapshot,
        _ => unreachable!(),
    }
}
#[track_caller]
fn work(e: &Engine) -> Work {
    e.send(Command::OrganizerTick).unwrap();
    match wait(e, |v| matches!(v, Event::OrganizerWork(_))) {
        Event::OrganizerWork(w) => w,
        _ => unreachable!(),
    }
}
fn complete(e: &Engine, w: &Work) {
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Ok(w
            .candidates
            .iter()
            .map(|c| Proposal {
                candidate: c.clone(),
                alias: Some("项目资料".into()),
                tags: vec!["工作".into()],
                reason: "已有项目资料上下文".into(),
            })
            .collect()),
    })
    .unwrap();
}
fn capture(e: &Engine, text: &str) {
    e.send(Command::Capture(ClipboardSnapshot {
        text: text.into(),
        representations: vec![Representation {
            format: "HTML Format".into(),
            data: vec![1, 2, 3],
        }],
        source_app: "fixture.exe".into(),
        captured_at_ms: now(),
    }))
    .unwrap();
}
#[track_caller]
fn stop(e: &Engine) {
    e.send(Command::Shutdown).unwrap();
    wait(e, |v| matches!(v, Event::ExitReady));
}
#[track_caller]
fn flush(e: &Engine) {
    e.send(Command::Flush).unwrap();
    wait(e, |v| matches!(v, Event::Saved(_)));
}
fn load(e: &Engine, id: &str) -> StoredItem {
    e.send(Command::Load {
        id: id.into(),
        request_id: 7,
    })
    .unwrap();
    match wait(e, |v| matches!(v, Event::Loaded { request_id: 7, .. })) {
        Event::Loaded { item, .. } => item.unwrap(),
        _ => unreachable!(),
    }
}
fn observe(e: &Engine, path: &str) {
    e.send(Command::ObservePaths(vec![PathObservation {
        path: path.into(),
        kind: ItemKind::Folder,
        observed_at_ms: now(),
        source: ObservationSource::Explorer,
    }]))
    .unwrap();
}
#[test]
fn suggest_manual_apply_preserves_pending_body_formats_and_deduplicates() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    capture(&e, &"正文".repeat(3000));
    let w = work(&e);
    assert_eq!(w.candidates[0].context.chars().count(), 1024);
    assert!(!w.candidates[0].expected_version.is_empty());
    complete(&e, &w);
    let s = snapshot(&e);
    assert_eq!(s.proposals.len(), 1);
    assert!(
        load(&e, &w.candidates[0].source.saved_id())
            .summary
            .alias
            .is_empty()
    );
    e.send(Command::OrganizerApply {
        proposals: s.proposals,
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    let id = w.candidates[0].source.saved_id();
    let item = load(&e, &id);
    assert_eq!(item.text, "正文".repeat(3000));
    assert_eq!(item.representations[0].data, vec![1, 2, 3]);
    assert_eq!(item.summary.alias, "项目资料");
    capture(&e, &item.text);
    e.send(Command::Used(id)).unwrap();
    assert_eq!(snapshot(&e).queued, 0);
    stop(&e);
    let e = open(&p);
    assert!(snapshot(&e).proposals.is_empty());
    stop(&e);
}
trait SavedId {
    fn saved_id(&self) -> String;
}
impl SavedId for Source {
    fn saved_id(&self) -> String {
        match self {
            Source::SavedItem { id } => id.clone(),
            _ => panic!("saved"),
        }
    }
}
#[test]
fn automatic_path_alias_is_searchable_without_favorite_and_is_deferred() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Automatic));
    observe(&e, r"C:\Projects\CaseSensitive\Logs");
    let w = work(&e);
    complete(&e, &w);
    assert!(snapshot(&e).proposals.is_empty());
    e.send(Command::Search(SearchQuery {
        text: "项目资料".into(),
        scope: Scope::All,
        request_id: 8,
    }))
    .unwrap();
    let paths = match wait(&e, |v| {
        matches!(v, Event::PathSuggestions { request_id: 8, .. })
    }) {
        Event::PathSuggestions { items, .. } => items,
        _ => unreachable!(),
    };
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].path, r"C:\Projects\CaseSensitive\Logs");
    assert_eq!(paths[0].alias, "项目资料");
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='organizer_state'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    stop(&e);
    let e = open(&p);
    observe(&e, r"C:\Projects\CaseSensitive\Logs");
    assert_eq!(snapshot(&e).queued, 0);
    stop(&e);
}
#[test]
fn settings_stale_delete_and_closed_organizer_gate_results() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("db"));
    let mut s = config(OrganizerMode::Suggest);
    settings(&e, s.clone());
    capture(&e, "stale");
    let w = work(&e);
    s.organizer.enabled = false;
    settings(&e, s.clone());
    complete(&e, &w);
    assert!(snapshot(&e).proposals.is_empty());
    s.organizer.enabled = true;
    settings(&e, s);
    let w = work(&e);
    e.send(Command::Delete(w.candidates[0].source.saved_id()))
        .unwrap();
    complete(&e, &w);
    assert!(snapshot(&e).proposals.is_empty());
    stop(&e);
}
#[test]
fn failure_cooldown_and_ready_survive_restart_without_retroactive_history() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    capture(&e, "old history");
    settings(&e, config(OrganizerMode::Suggest));
    assert_eq!(snapshot(&e).queued, 0);
    capture(&e, "new history");
    let w = work(&e);
    e.send(Command::OrganizerComplete {
        token: w.token,
        result: Err("fixture failure".into()),
    })
    .unwrap();
    let s = snapshot(&e);
    assert_eq!(s.failures, 1);
    assert!(s.retry_at_ms.unwrap() >= now() + 299_000);
    stop(&e);
    let e = open(&p);
    let s = snapshot(&e);
    assert_eq!(s.failures, 1);
    assert_eq!(s.queued, 1);
    e.send(Command::OrganizerRetry).unwrap();
    let w = work(&e);
    complete(&e, &w);
    let s = snapshot(&e);
    assert_eq!(s.proposals.len(), 1);
    stop(&e);
    let e = open(&p);
    assert_eq!(snapshot(&e).proposals.len(), 1);
    let mut s = config(OrganizerMode::Automatic);
    s.organizer.enabled = false;
    settings(&e, s);
    let s = snapshot(&e);
    assert_eq!(s.proposals.len(), 1);
    e.send(Command::OrganizerDismiss {
        sources: s
            .proposals
            .iter()
            .map(|p| p.candidate.source.clone())
            .collect(),
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    stop(&e);
}

#[test]
fn queue_budget_preserves_source_content_and_exposes_saturation() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    let mut s = config(OrganizerMode::Suggest);
    s.organizer.batch_size = 8;
    settings(&e, s);
    for i in 0..260 {
        capture(&e, &format!("bounded fixture {i}"));
    }
    let state = snapshot(&e);
    assert_eq!(state.queued, 256);
    assert!(state.saturated);
    for _ in 0..16 {
        e.send(Command::OrganizerRetry).unwrap();
        let w = work(&e);
        assert_eq!(w.candidates.len(), 8);
        complete(&e, &w);
        snapshot(&e);
    }
    let state = snapshot(&e);
    assert_eq!(state.proposals.len(), 128);
    assert_eq!(state.queued, 128);
    e.send(Command::OrganizerRetry).unwrap();
    e.send(Command::OrganizerTick).unwrap();
    assert!(!snapshot(&e).in_flight);
    let mut automatic = config(OrganizerMode::Automatic);
    automatic.organizer.batch_size = 8;
    settings(&e, automatic);
    let w = work(&e);
    assert_eq!(w.candidates.len(), 8);
    complete(&e, &w);
    let after = snapshot(&e);
    assert_eq!(after.proposals.len(), 128);
    assert_eq!(after.queued, 120);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        260
    );
    let bytes: i64 = db
        .query_row(
            "SELECT length(CAST(data AS BLOB)) FROM organizer_state",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes <= MAX_STORE_BYTES as i64);
    let raw: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    let mut cached: serde_json::Value = serde_json::from_str(&raw).unwrap();
    cached["saturated"] = serde_json::json!(true);
    db.execute("UPDATE organizer_state SET data=?1", [cached.to_string()])
        .unwrap();
    let e = open(&p);
    assert_eq!(snapshot(&e).proposals.len(), 128);
    let first = snapshot(&e).proposals[0].candidate.source.clone();
    e.send(Command::OrganizerDismiss {
        sources: vec![first],
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    let remaining = snapshot(&e);
    assert_eq!(remaining.proposals.len(), 127);
    assert!(
        !remaining.saturated,
        "dismiss must release visible capacity"
    );
    e.send(Command::OrganizerApply {
        proposals: vec![remaining.proposals[0].clone()],
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    let remaining = snapshot(&e);
    assert_eq!(remaining.proposals.len(), 126);
    assert!(!remaining.saturated, "apply must release visible capacity");
    stop(&e);
}
fn favorite(e: &Engine, id: &str, alias: &str, tags: Vec<String>) {
    e.send(Command::SaveFavorite(FavoriteInput {
        id: Some(id.into()),
        kind: ItemKind::Folder,
        title: id.into(),
        target: format!("C:/Fixture/{id}"),
        alias: alias.into(),
        tags,
        open_with: Some("fixture-open".into()),
    }))
    .unwrap();
    wait(e, |v| matches!(v, Event::Saved(_)));
}
#[test]
fn automatic_saved_alias_preserves_manual_metadata_target_and_disk_representations() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Automatic));
    favorite(&e, "saved", "人工名", vec!["原标签".into()]);
    let w = work(&e);
    complete(&e, &w);
    assert_eq!(snapshot(&e).queued, 0);
    let item = load(&e, "saved");
    assert_eq!(item.summary.alias, "人工名");
    assert_eq!(item.summary.tags, vec!["原标签", "工作"]);
    assert_eq!(item.target.as_deref(), Some("C:/Fixture/saved"));
    assert_eq!(item.open_with.as_deref(), Some("fixture-open"));
    flush(&e);
    e.send(Command::OrganizerRetry).unwrap();
    capture(&e, "disk body fixture");
    flush(&e);
    let w = work(&e);
    complete(&e, &w);
    snapshot(&e);
    flush(&e);
    let item = load(&e, &w.candidates[0].source.saved_id());
    assert_eq!(item.text, "disk body fixture");
    assert_eq!(item.representations[0].data, vec![1, 2, 3]);
    stop(&e);
}
#[test]
fn transaction_failure_preserves_atomic_receipts() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    favorite(&e, "one", "", vec![]);
    favorite(&e, "two", "", vec![]);
    let w = work(&e);
    complete(&e, &w);
    let proposals = snapshot(&e).proposals;
    assert_eq!(proposals.len(), 2);
    flush(&e);
    let db = Connection::open(&p).unwrap();
    db.execute_batch("CREATE TRIGGER fail_organizer BEFORE UPDATE ON items BEGIN SELECT RAISE(ABORT,'fixture'); END").unwrap();
    e.send(Command::OrganizerApply {
        proposals: proposals.clone(),
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Error { .. }));
    let summary: String = db
        .query_row("SELECT summary FROM items WHERE id='one'", [], |r| r.get(0))
        .unwrap();
    assert!(
        serde_json::from_str::<ItemSummary>(&summary)
            .unwrap()
            .alias
            .is_empty()
    );
    let saved: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    assert_eq!(saved.matches("\"Ready\"").count(), 2);
    db.execute_batch("DROP TRIGGER fail_organizer").unwrap();
    flush(&e);
    assert_eq!(load(&e, "one").summary.alias, "项目资料");
    let saved: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    assert!(!saved.contains("\"Ready\""));
    stop(&e);
}
#[test]
fn invalid_partial_results_pause_after_three_persisted_failures() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    capture(&e, "failure fixture");
    for failure in 1..=3 {
        let w = work(&e);
        e.send(Command::OrganizerComplete {
            token: w.token,
            result: Ok(vec![]),
        })
        .unwrap();
        let state = snapshot(&e);
        assert_eq!(state.failures, failure);
        assert_eq!(state.paused, failure == 3);
        assert!(state.retry_at_ms.unwrap() >= now() + if failure == 1 { 299_000 } else { 599_000 });
        stop(&e);
        drop(e);
        let db = Connection::open(&p).unwrap();
        let raw: String = db
            .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value["retry_at_ms"] = 0.into();
        value["next_at_ms"] = 0.into();
        db.execute("UPDATE organizer_state SET data=?1", [value.to_string()])
            .unwrap();
        drop(db);
        e = open(&p);
    }
    e.send(Command::OrganizerTick).unwrap();
    let state = snapshot(&e);
    assert!(state.paused);
    assert!(!state.in_flight);
    e.send(Command::OrganizerRetry).unwrap();
    let w = work(&e);
    complete(&e, &w);
    assert_eq!(snapshot(&e).proposals.len(), 1);
    stop(&e);
}

#[test]
fn config_clear_and_scope_changes_revoke_work_but_keep_ready_when_switching_modes() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("db"));
    let mut s = config(OrganizerMode::Suggest);
    settings(&e, s.clone());
    observe(&e, "C:/Ready");
    let w = work(&e);
    complete(&e, &w);
    assert_eq!(snapshot(&e).proposals.len(), 1);
    s.organizer.mode = OrganizerMode::Automatic;
    settings(&e, s.clone());
    assert_eq!(snapshot(&e).proposals.len(), 1);
    observe(&e, "C:/Inflight");
    let w = work(&e);
    s.ai.model = "different".into();
    settings(&e, s.clone());
    complete(&e, &w);
    assert_eq!(snapshot(&e).proposals.len(), 1);
    e.send(Command::ClearPathActivity).unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    assert!(snapshot(&e).proposals.is_empty());
    assert_eq!(snapshot(&e).queued, 0);
    s.organizer.scope = OrganizerScope::PathsOnly;
    settings(&e, s);
    favorite(&e, "excluded-saved", "", vec![]);
    assert_eq!(snapshot(&e).queued, 0);
    stop(&e);
}
#[test]
fn default_scope_pin_and_exclusion_gate_and_stale_multi_selection_is_atomic() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("db"));
    let mut s = config(OrganizerMode::Suggest);
    s.organizer.scope = OrganizerScope::SavedAndPaths;
    s.path_activity.excluded_paths = vec!["C:/Private".into()];
    settings(&e, s.clone());
    capture(&e, "plain");
    assert_eq!(snapshot(&e).queued, 0);
    e.send(Command::Search(SearchQuery {
        text: "plain".into(),
        scope: Scope::Clipboard,
        request_id: 1,
    }))
    .unwrap();
    let id = match wait(&e, |v| matches!(v, Event::Results { request_id: 1, .. })) {
        Event::Results { items, .. } => items[0].id.clone(),
        _ => unreachable!(),
    };
    e.send(Command::Pin {
        id: id.clone(),
        pinned: true,
    })
    .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    favorite(&e, "other", "", vec![]);
    let w = work(&e);
    complete(&e, &w);
    let proposals = snapshot(&e).proposals;
    assert_eq!(proposals.len(), 2);
    e.send(Command::Pin { id, pinned: false }).unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    e.send(Command::OrganizerApply { proposals }).unwrap();
    wait(&e, |v| matches!(v, Event::Error { .. }));
    assert!(load(&e, "other").summary.alias.is_empty());
    observe(&e, "C:/Private/Secret");
    observe(&e, "C:/Allowed/Logs");
    assert_eq!(snapshot(&e).queued, 1);
    e.send(Command::OrganizerRetry).unwrap();
    let w = work(&e);
    e.send(Command::SuppressPath("C:/Allowed/Logs".into()))
        .unwrap();
    wait(&e, |v| matches!(v, Event::Saved(_)));
    complete(&e, &w);
    assert_eq!(snapshot(&e).proposals.len(), 1);
    stop(&e);
}
#[test]
fn inflight_restart_recovers_without_ignoring_interval_and_done_receipts_evict_first() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Automatic));
    capture(&e, "inflight");
    let _w = work(&e);
    flush(&e);
    let db = Connection::open(&p).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    assert!(raw.contains("Processing"));
    stop(&e);
    // Simulate a crash image with Processing as captured above, not graceful shutdown recovery.
    db.execute("UPDATE organizer_state SET data=?1", [raw])
        .unwrap();
    drop(db);
    let e = open(&p);
    assert_eq!(snapshot(&e).queued, 1);
    e.send(Command::OrganizerTick).unwrap();
    assert!(!snapshot(&e).in_flight);
    e.send(Command::OrganizerRetry).unwrap();
    let w = work(&e);
    complete(&e, &w);
    snapshot(&e);
    for i in 0..256 {
        capture(&e, &format!("after receipt {i}"));
    }
    let s = snapshot(&e);
    assert_eq!(s.queued, 256);
    assert!(!s.saturated);
    stop(&e);
}

#[test]
fn byte_budget_stops_network_before_ready_growth_and_preserves_all_sources() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    for i in 0..240 {
        e.send(Command::SaveFavorite(FavoriteInput {
            id: Some(format!("large-{i}")),
            kind: ItemKind::Folder,
            title: format!("large-{i}"),
            target: format!("C:/{}", "路径".repeat(510)),
            alias: "人".repeat(256),
            tags: (0..32)
                .map(|n| format!("{n:02}{}", "标".repeat(62)))
                .collect(),
            open_with: None,
        }))
        .unwrap();
        wait(&e, |v| matches!(v, Event::Saved(_)));
    }
    let s = snapshot(&e);
    assert!(s.queued < 240);
    assert!(s.saturated);
    e.send(Command::OrganizerTick).unwrap();
    assert!(
        !snapshot(&e).in_flight,
        "must reserve room for bounded proposal before sending source data"
    );
    stop(&e);
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        240
    );
    let n: i64 = db
        .query_row(
            "SELECT length(CAST(data AS BLOB)) FROM organizer_state",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(n <= MAX_STORE_BYTES as i64);
}

#[test]
fn ready_reservation_reclaims_done_before_stalling_and_preserves_ready_sources() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    settings(&e, config(OrganizerMode::Suggest));
    favorite(&e, "small-ready", "", vec![]);
    let w = work(&e);
    complete(&e, &w);
    let ready = snapshot(&e).proposals;
    for i in 0..180 {
        eprintln!("ready reservation seed {i}/180");
        e.send(Command::SaveFavorite(FavoriteInput {
            id: Some(format!("reclaim-{i}")),
            kind: ItemKind::Folder,
            title: "𐐀".repeat(256),
            target: format!("C:/{}", "𐐀".repeat(1021)),
            alias: "𐐀".repeat(256),
            tags: (0..32)
                .map(|n| format!("{n:02}{}", "𐐀".repeat(62)))
                .collect(),
            open_with: None,
        }))
        .unwrap();
        wait(&e, |v| matches!(v, Event::Saved(_)));
    }
    let full = snapshot(&e);
    assert!(full.saturated);
    assert!(full.queued < 180);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&raw).unwrap();
    // A valid cache image after metadata-neutral Automatic completions, followed
    // by switching to Suggest: identity/version/Ready/source rows remain real.
    let records = state["records"].as_array_mut().unwrap();
    let done_sources: Vec<_> = records
        .iter_mut()
        .filter(|r| r["status"] == "Queued")
        .take(16)
        .map(|r| {
            r["status"] = "Done".into();
            r["candidate"]["source"].clone()
        })
        .collect();
    let before = records.len();
    state["next_at_ms"] = 0.into();
    db.execute("UPDATE organizer_state SET data=?1", [state.to_string()])
        .unwrap();
    drop(db);
    let e = open(&p);
    eprintln!("ready reservation reopened; requesting work");
    let w = work(&e);
    assert!(!w.candidates.is_empty());
    assert_eq!(snapshot(&e).proposals, ready);
    flush(&e);
    let db = Connection::open(&p).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM organizer_state", [], |r| r.get(0))
        .unwrap();
    let after: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let records = after["records"].as_array().unwrap();
    assert!(records.len() < before);
    assert!(
        !records
            .iter()
            .any(|r| r["candidate"]["source"] == done_sources[0])
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        181
    );
    stop(&e);
}
#[test]
fn oversized_durable_state_pauses_only_organizer_without_discarding_it() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let e = open(&p);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    db.execute_batch("CREATE TABLE organizer_state(id INTEGER PRIMARY KEY,data TEXT)")
        .unwrap();
    db.execute(
        "INSERT INTO organizer_state VALUES(1,?1)",
        [" ".repeat(MAX_STORE_BYTES + 1)],
    )
    .unwrap();
    drop(db);
    let e = open(&p);
    let s = snapshot(&e);
    assert!(s.paused);
    assert!(s.storage_error.is_some());
    settings(&e, config(OrganizerMode::Suggest));
    e.send(Command::OrganizerRetry).unwrap();
    capture(&e, "usable original history");
    e.send(Command::OrganizerTick).unwrap();
    assert!(!snapshot(&e).in_flight);
    stop(&e);
    let db = Connection::open(&p).unwrap();
    assert_eq!(
        db.query_row("SELECT length(data) FROM organizer_state", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        MAX_STORE_BYTES as i64 + 1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn retention_removal_revokes_inflight_without_error_or_resurrection() {
    let d = tempfile::tempdir().unwrap();
    let e = open(&d.path().join("db"));
    let mut s = config(OrganizerMode::Automatic);
    s.max_history = 1;
    settings(&e, s);
    capture(&e, "expired first");
    let w = work(&e);
    capture(&e, "new second");
    flush(&e);
    complete(&e, &w);
    let s = snapshot(&e);
    assert!(!s.in_flight);
    assert_eq!(s.queued, 1);
    assert!(s.proposals.is_empty());
    stop(&e);
}
