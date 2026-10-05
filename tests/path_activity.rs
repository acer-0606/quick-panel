use quick_panel::{engine::Engine, model::*, path_activity::*};
use rusqlite::Connection;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
fn wait(engine: &Engine, accept: impl Fn(&Event) -> bool) -> Event {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match engine.try_recv() {
            Ok(event) if accept(&event) => return event,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            Ok(_) => (),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < end, "event timeout");
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("{e}"),
        }
    }
}
fn open(path: &std::path::Path, interval: u64) -> Engine {
    let mut options = EngineOptions::new(path.into());
    options.save_interval_ms = interval;
    let engine = Engine::start(options, Arc::new(|| {})).unwrap();
    wait(&engine, |e| matches!(e, Event::Ready(_)));
    engine
}
fn enable(engine: &Engine) -> Settings {
    let mut settings = Settings::default();
    settings.path_activity.enabled = true;
    save_settings(engine, settings.clone());
    settings
}
fn save_settings(engine: &Engine, settings: Settings) {
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(engine, |e| matches!(e, Event::Saved(_)));
}
fn observe(path: &str, kind: ItemKind, time: i64, source: ObservationSource) -> PathObservation {
    PathObservation {
        path: path.into(),
        kind,
        observed_at_ms: time,
        source,
    }
}
fn query(engine: &Engine, text: &str, scope: Scope) -> Vec<PathSuggestion> {
    engine
        .send(Command::Search(SearchQuery {
            text: text.into(),
            scope,
            request_id: 9,
        }))
        .unwrap();
    match wait(engine, |e| {
        matches!(e, Event::PathSuggestions { request_id: 9, .. })
    }) {
        Event::PathSuggestions { items, .. } => items,
        _ => unreachable!(),
    }
}
fn shutdown(engine: &Engine) {
    engine.send(Command::Shutdown).unwrap();
    wait(engine, |e| matches!(e, Event::ExitReady));
}
fn count(db: &Connection, table: &str) -> i64 {
    if db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name=?1",
            [table],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 0
    {
        return 0;
    }
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn absolute_windows_path_is_normalized_without_target_io() {
    for (input, expected) in [
        (r"C:\\Work\\Project\\", r"c:\work\project"),
        ("C:/Work/./x/../", r"c:\work"),
        ("C:/", r"c:\"),
        (r"\\Server\Share\a\..", r"\\server\share\"),
        (r"\\Server\\Share\\a", r"\\server\share\a"),
    ] {
        assert_eq!(normalize_path(input).unwrap(), expected);
    }
    for invalid in [
        r"relative\file.txt",
        r"C:\secret.txt:stream",
        r"C:foo",
        r"\\?\C:\x",
        r"\\.\pipe\x",
        r"\\server",
        r"C:\..\x",
        "C:/x\0",
        "C:/x\n",
    ] {
        assert!(normalize_path(invalid).is_err(), "{invalid:?}");
    }
    assert!(normalize_path(&format!("C:/{}", "中".repeat(2046))).is_err());
}
#[test]
fn old_settings_default_to_disabled() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert!(!s.path_activity.enabled);
    assert!(s.path_activity.recent_documents);
    assert!(s.path_activity.excluded_paths.is_empty());
}
#[test]
fn external_observations_are_merged_in_ram_then_saved_on_exit_without_index_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    engine
        .send(Command::ObservePaths(vec![
            observe(
                r"C:\Work\项目",
                ItemKind::Folder,
                now(),
                ObservationSource::Explorer,
            ),
            observe(
                r"c:/work/项目/",
                ItemKind::Folder,
                now() + 10_000,
                ObservationSource::RecentDocuments,
            ),
        ]))
        .unwrap();
    let items = query(&engine, "xiangmu", Scope::All);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].visits, 1);
    let db = Connection::open(&path).unwrap();
    assert_eq!(count(&db, "path_activity"), 0);
    assert_eq!(count(&db, "items"), 0);
    shutdown(&engine);
    assert_eq!(count(&db, "path_activity"), 1);
    assert_eq!(count(&db, "items"), 0);
    assert_eq!(count(&db, "item_search"), 0);
    let reopened = open(&path, 60_000);
    assert_eq!(query(&reopened, "项目", Scope::Folder)[0].visits, 1);
    assert!(query(&reopened, "项目", Scope::Document).is_empty());
    shutdown(&reopened);
}
#[test]
fn sustained_observations_do_not_postpone_first_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 300);
    enable(&engine);
    let start = Instant::now();
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/first",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    assert_eq!(query(&engine, "first", Scope::All).len(), 1);
    thread::sleep(Duration::from_millis(190));
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/second",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    let saved = wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert!(start.elapsed() < Duration::from_millis(460));
    assert!(matches!(saved, Event::Saved(s) if !s.semantic_status_changed));
    assert_eq!(count(&Connection::open(path).unwrap(), "path_activity"), 2);
    shutdown(&engine);
}
#[test]
fn failed_commit_keeps_ram_and_flush_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/locked",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert_eq!(query(&engine, "locked", Scope::All).len(), 1);
    assert_eq!(count(&db, "path_activity"), 0);
    db.execute_batch("ROLLBACK").unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&db, "path_activity"), 1);
    shutdown(&engine);
}
#[test]
fn disabled_excluded_suppressed_and_cleared_paths_stay_independent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/disabled",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    assert!(query(&engine, "", Scope::All).is_empty());
    let mut settings = enable(&engine);
    settings.path_activity.excluded_paths = vec!["C:/Work".into()];
    settings.path_activity.recent_documents = false;
    save_settings(&engine, settings.clone());
    engine
        .send(Command::ObservePaths(vec![
            observe(
                "C:/Work/private",
                ItemKind::Folder,
                now(),
                ObservationSource::Explorer,
            ),
            observe(
                "C:/Worker/visible",
                ItemKind::Folder,
                now(),
                ObservationSource::Explorer,
            ),
            observe(
                "C:/recent",
                ItemKind::Document,
                now(),
                ObservationSource::RecentDocuments,
            ),
        ]))
        .unwrap();
    assert_eq!(query(&engine, "", Scope::All).len(), 1);
    settings.path_activity.enabled = false;
    save_settings(&engine, settings.clone());
    assert!(query(&engine, "", Scope::All).is_empty());
    settings.path_activity.enabled = true;
    save_settings(&engine, settings);
    let key = query(&engine, "", Scope::All)[0].key.clone();
    engine.send(Command::SuppressPath(key)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    shutdown(&engine);
    let engine = open(&path, 60_000);
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/Worker/visible",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    assert!(query(&engine, "", Scope::All).is_empty());
    engine.send(Command::ClearPathActivity).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/Worker/visible",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    assert_eq!(query(&engine, "", Scope::All).len(), 1);
    shutdown(&engine);
}
#[test]
fn ranking_decays_and_clock_anomalies_do_not_poison_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(&dir.path().join("db"), 60_000);
    enable(&engine);
    let t = now();
    engine
        .send(Command::ObservePaths(vec![
            observe(
                "C:/old",
                ItemKind::Folder,
                t - 14 * 86_400_000,
                ObservationSource::Explorer,
            ),
            observe(
                "C:/recent",
                ItemKind::Folder,
                t,
                ObservationSource::RecentDocuments,
            ),
            observe(
                "C:/future",
                ItemKind::Folder,
                i64::MAX,
                ObservationSource::Explorer,
            ),
        ]))
        .unwrap();
    query(&engine, "", Scope::All);
    for time in [t - 60_000, t] {
        engine
            .send(Command::ObservePaths(vec![observe(
                "C:/future",
                ItemKind::Folder,
                time,
                ObservationSource::Explorer,
            )]))
            .unwrap();
        query(&engine, "", Scope::All);
    }
    let items = query(&engine, "", Scope::All);
    assert_eq!(items[0].path, r"c:\future");
    assert_eq!(items[0].visits, 3);
    assert!(items[0].last_seen_ms <= now());
    assert_eq!(items[1].path, r"c:\recent");
    assert_eq!(items[2].path, r"c:\old");
    shutdown(&engine);
}
#[test]
fn batches_and_total_history_are_bounded_without_discarding_accepted_records() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    assert!(
        engine
            .send(Command::ObservePaths(
                (0..33)
                    .map(|i| observe(
                        &format!("C:/oversized{i}"),
                        ItemKind::Folder,
                        now(),
                        ObservationSource::Explorer
                    ))
                    .collect()
            ))
            .is_err()
    );
    for batch in 0..33 {
        engine
            .send(Command::ObservePaths(
                (0..32)
                    .map(|i| {
                        observe(
                            &format!("C:/item{:04}", batch * 32 + i),
                            ItemKind::Folder,
                            1_790_000_000_000,
                            ObservationSource::Explorer,
                        )
                    })
                    .collect(),
            ))
            .unwrap();
        query(&engine, "item", Scope::All);
    }
    assert_eq!(query(&engine, "", Scope::All).len(), 5);
    assert_eq!(query(&engine, "item", Scope::All).len(), 10);
    assert_eq!(query(&engine, "item0000", Scope::All).len(), 1);
    assert!(query(&engine, "item1024", Scope::All).is_empty());
    shutdown(&engine);
    assert_eq!(
        count(&Connection::open(path).unwrap(), "path_activity"),
        1024
    );
}
#[test]
fn idle_search_never_creates_path_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    assert!(query(&engine, "", Scope::All).is_empty());
    shutdown(&engine);
    let db = Connection::open(path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='path_activity'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn ai_and_semantic_queries_publish_matching_local_candidates_without_embedding_sources() {
    use quick_panel::semantic::{IndexCursor, SemanticSearchQuery, profile_id};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    let mut settings = enable(&engine);
    settings.embedding.enabled = true;
    settings.embedding.model = "local-fixture".into();
    save_settings(&engine, settings.clone());
    let t = now();
    engine
        .send(Command::ObservePaths(vec![
            observe(
                "C:/资料/预算.txt",
                ItemKind::Document,
                t,
                ObservationSource::Explorer,
            ),
            observe(
                "C:/资料/旧预算.txt",
                ItemKind::Document,
                t - 86_400_000,
                ObservationSource::Explorer,
            ),
        ]))
        .unwrap();
    engine
        .send(Command::AiSearch(AiSearchQuery {
            queries: vec!["yusuan".into()],
            scope: Scope::Document,
            from_ms: Some(t - 60_000),
            request_id: 51,
            ..Default::default()
        }))
        .unwrap();
    let event = wait(&engine, |e| {
        matches!(e, Event::PathSuggestions { request_id: 51, .. })
    });
    assert!(
        matches!(event, Event::PathSuggestions { items, .. } if items.len() == 1 && items[0].path == "c:\\资料\\预算.txt")
    );
    assert!(
        matches!(wait(&engine, |e| matches!(e, Event::Results { request_id: 51, .. })), Event::Results {items, ..} if items.is_empty())
    );
    engine
        .send(Command::SemanticSearch(SemanticSearchQuery {
            text: "预算".into(),
            scope: Scope::Document,
            request_id: 52,
            profile: profile_id(&settings.embedding).unwrap(),
            vector: vec![1., 0.],
        }))
        .unwrap();
    assert!(
        matches!(wait(&engine, |e| matches!(e, Event::PathSuggestions { request_id: 52, .. })), Event::PathSuggestions {items, ..} if items.len() == 2)
    );
    let mut cursor = IndexCursor::default();
    loop {
        engine
            .send(Command::PrepareSemanticIndex {
                request_id: 53,
                cursor,
            })
            .unwrap();
        let Event::SemanticBatch(batch) = wait(&engine, |e| matches!(e, Event::SemanticBatch(_)))
        else {
            unreachable!()
        };
        assert!(batch.candidates.is_empty());
        if batch.done {
            break;
        }
        cursor = batch.cursor;
    }
    shutdown(&engine);
    let db = Connection::open(path).unwrap();
    for table in ["items", "item_search", "semantic_vectors"] {
        assert_eq!(count(&db, table), 0);
    }
}
#[test]
fn suppression_limit_preserves_all_hidden_records_and_clear_recovers_capacity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    for i in 0..128 {
        engine
            .send(Command::SuppressPath(format!("C:/hidden{i}")))
            .unwrap();
        wait(&engine, |e| matches!(e, Event::Saved(_)));
    }
    engine
        .send(Command::SuppressPath("C:/overflow".into()))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    shutdown(&engine);
    let engine = open(&path, 60_000);
    engine
        .send(Command::ObservePaths(vec![
            observe(
                "C:/hidden0",
                ItemKind::Folder,
                now(),
                ObservationSource::Explorer,
            ),
            observe(
                "C:/overflow",
                ItemKind::Folder,
                now(),
                ObservationSource::Explorer,
            ),
        ]))
        .unwrap();
    let items = query(&engine, "", Scope::All);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, r"c:\overflow");
    engine.send(Command::ClearPathActivity).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    shutdown(&engine);
    assert_eq!(count(&Connection::open(path).unwrap(), "path_activity"), 0);
}
#[test]
fn long_path_byte_budget_refuses_new_rows_but_preserves_previous_history_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    for batch in 0..12 {
        engine
            .send(Command::ObservePaths(
                (0..32)
                    .map(|i| {
                        observe(
                            &format!("C:/{}{:04}", "长".repeat(1900), batch * 32 + i),
                            ItemKind::Folder,
                            now(),
                            ObservationSource::Explorer,
                        )
                    })
                    .collect(),
            ))
            .unwrap();
        query(&engine, "不存在", Scope::All);
    }
    assert_eq!(query(&engine, "0000", Scope::All).len(), 1);
    assert!(query(&engine, "0383", Scope::All).is_empty());
    shutdown(&engine);
    let db = Connection::open(&path).unwrap();
    let accepted = count(&db, "path_activity");
    assert!(accepted > 0 && accepted < 384);
    let stored_bytes: i64 = db
        .query_row(
            "SELECT sum(length(cast(path AS BLOB))) FROM path_activity",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored_bytes < 2 * 1024 * 1024);
    let engine = open(&path, 60_000);
    assert_eq!(query(&engine, "0000", Scope::All).len(), 1);
    shutdown(&engine);
    assert_eq!(count(&db, "path_activity"), accepted);
}
#[test]
fn repeated_batch_and_equal_scores_are_deterministic_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    let t = now() - 60_000;
    engine
        .send(Command::ObservePaths(vec![
            observe("C:/z", ItemKind::Folder, t, ObservationSource::Explorer),
            observe("C:/a", ItemKind::Folder, t, ObservationSource::Explorer),
            observe(
                "C:/A",
                ItemKind::Folder,
                t - 60_000,
                ObservationSource::QuickPanel,
            ),
        ]))
        .unwrap();
    let items = query(&engine, "", Scope::All);
    assert_eq!(items[0].path, r"c:\a");
    assert_eq!(items[0].visits, 1);
    assert_eq!(items[1].path, r"c:\z");
    shutdown(&engine);
    let engine = open(&path, 60_000);
    assert_eq!(query(&engine, "", Scope::All), items);
    shutdown(&engine);
}
#[test]
fn path_write_failure_rolls_back_content_and_explicit_clear_recovers_together() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let engine = open(&path, 60_000);
    enable(&engine);
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/first",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_path BEFORE INSERT ON path_activity BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
    engine
        .send(Command::Capture(ClipboardSnapshot {
            text: "transaction fixture".into(),
            representations: vec![],
            source_app: "fixture".into(),
            captured_at_ms: now(),
        }))
        .unwrap();
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/second",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert_eq!(count(&db, "items"), 0);
    assert_eq!(query(&engine, "second", Scope::All).len(), 1);
    db.execute_batch("DROP TRIGGER fail_path; BEGIN IMMEDIATE;")
        .unwrap();
    engine.send(Command::ClearPathActivity).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert!(query(&engine, "", Scope::All).is_empty());
    assert_eq!(count(&db, "path_activity"), 1);
    db.execute_batch("ROLLBACK").unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&db, "path_activity"), 0);
    assert_eq!(count(&db, "items"), 1);
    shutdown(&engine);
}

#[test]
fn observation_queue_budget_is_separate_and_latest_query_modes_share_one_slot() {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let block = Arc::new(AtomicBool::new(false));
    let notifier_block = block.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let mut options = EngineOptions::new(path.clone());
    options.max_pending_bytes = 1024;
    let engine = Engine::start(
        options,
        Arc::new(move || {
            if notifier_block.swap(false, Ordering::AcqRel) {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        }),
    )
    .unwrap();
    wait(&engine, |e| matches!(e, Event::Ready(_)));
    enable(&engine);
    block.store(true, Ordering::Release);
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/barrier",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let batch = || {
        (0..32)
            .map(|i| {
                observe(
                    &format!("C:/{}{:02}", "长".repeat(1900), i),
                    ItemKind::Folder,
                    now(),
                    ObservationSource::Explorer,
                )
            })
            .collect()
    };
    let first = engine.send(Command::ObservePaths(batch()));
    let second = engine.send(Command::ObservePaths(batch()));
    let capture = engine.send(Command::Capture(ClipboardSnapshot {
        text: "clip".into(),
        representations: vec![],
        source_app: "fixture".into(),
        captured_at_ms: now(),
    }));
    engine
        .send(Command::Search(SearchQuery {
            request_id: 61,
            ..Default::default()
        }))
        .unwrap();
    engine
        .send(Command::AiSearch(AiSearchQuery {
            queries: vec!["old".into()],
            request_id: 62,
            ..Default::default()
        }))
        .unwrap();
    engine
        .send(Command::Search(SearchQuery {
            text: "barrier".into(),
            request_id: 63,
            ..Default::default()
        }))
        .unwrap();
    release_tx.send(()).unwrap();
    assert!(first.is_ok());
    assert!(second.is_err());
    assert!(capture.is_ok());
    let mut paths_seen = false;
    loop {
        match wait(&engine, |e| {
            matches!(e, Event::PathSuggestions { .. } | Event::Results { .. })
        }) {
            Event::PathSuggestions { request_id, items } => {
                assert_eq!(request_id, 63);
                assert_eq!(items.len(), 1);
                paths_seen = true;
            }
            Event::Results { request_id, .. } => {
                assert_eq!(request_id, 63);
                break;
            }
            _ => unreachable!(),
        }
    }
    assert!(paths_seen);
    shutdown(&engine);
    assert_eq!(count(&Connection::open(path).unwrap(), "items"), 1);
}
#[test]
fn path_only_commit_does_not_restart_an_in_progress_embedding_scan() {
    use quick_panel::semantic::IndexCursor;
    let dir = tempfile::tempdir().unwrap();
    let engine = open(&dir.path().join("db"), 60_000);
    let mut settings = enable(&engine);
    settings.embedding.enabled = true;
    settings.embedding.model = "local-fixture".into();
    save_settings(&engine, settings);
    for i in 0..20 {
        engine
            .send(Command::SaveFavorite(FavoriteInput {
                id: Some(format!("favorite{i}")),
                kind: ItemKind::Document,
                title: format!("fixture{i}"),
                target: format!("C:/favorite{i}"),
                alias: String::new(),
                tags: vec![],
                open_with: None,
            }))
            .unwrap();
        wait(&engine, |e| matches!(e, Event::Saved(_)));
    }
    engine
        .send(Command::PrepareSemanticIndex {
            request_id: 71,
            cursor: IndexCursor::default(),
        })
        .unwrap();
    let Event::SemanticBatch(first) = wait(&engine, |e| matches!(e, Event::SemanticBatch(_)))
    else {
        unreachable!()
    };
    assert_eq!(first.candidates.len(), 16);
    engine
        .send(Command::ObservePaths(vec![observe(
            "C:/automatic",
            ItemKind::Folder,
            now(),
            ObservationSource::Explorer,
        )]))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    assert!(
        matches!(wait(&engine, |e| matches!(e, Event::Saved(_))), Event::Saved(s) if !s.semantic_status_changed)
    );
    engine
        .send(Command::PrepareSemanticIndex {
            request_id: 72,
            cursor: first.cursor,
        })
        .unwrap();
    let Event::SemanticBatch(second) = wait(&engine, |e| matches!(e, Event::SemanticBatch(_)))
    else {
        unreachable!()
    };
    assert_eq!(second.candidates.len(), 4);
    assert!(
        second
            .candidates
            .iter()
            .all(|candidate| !first.candidates.iter().any(|old| old.id == candidate.id))
    );
    shutdown(&engine);
}

#[test]
fn queue_budget_counts_retained_string_and_batch_allocations() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(&dir.path().join("db"), 60_000);
    enable(&engine);
    let mut path = String::with_capacity(300 * 1024);
    path.push_str("C:/small");
    assert!(
        engine
            .send(Command::ObservePaths(vec![PathObservation {
                path,
                kind: ItemKind::Folder,
                observed_at_ms: now(),
                source: ObservationSource::Explorer
            }]))
            .is_err()
    );
    let mut observations = Vec::with_capacity(10_000);
    observations.push(observe(
        "C:/small",
        ItemKind::Folder,
        now(),
        ObservationSource::Explorer,
    ));
    assert!(engine.send(Command::ObservePaths(observations)).is_err());
    assert!(query(&engine, "small", Scope::All).is_empty());
    shutdown(&engine);
}

#[test]
fn windows_dos_devices_are_not_accepted_as_filesystem_observations() {
    for path in [
        "C:/NUL",
        "C:/folder/CON.txt",
        "C:/COM1.log",
        "C:/LPT9",
        "C:/AUX",
        "C:/PRN",
        "C:/CONIN$",
        "C:/COM¹.txt",
    ] {
        assert!(normalize_path(path).is_err(), "accepted device {path}");
    }
    assert_eq!(normalize_path("C:/COM10.txt").unwrap(), r"c:\com10.txt");
}
