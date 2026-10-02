use quick_panel::{engine::Engine, model::*};
use rusqlite::Connection;
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn snapshot(text: &str) -> ClipboardSnapshot {
    ClipboardSnapshot {
        text: text.into(),
        representations: vec![
            Representation {
                format: "CF_UNICODETEXT".into(),
                data: text.as_bytes().to_vec(),
            },
            Representation {
                format: "PNG".into(),
                data: vec![0, 255, 7, 13],
            },
        ],
        source_app: "editor.exe".into(),
        captured_at_ms: 1_791_000_000_000,
    }
}
fn start(interval: u64, budget: usize) -> (TempDir, Engine, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("panel.sqlite");
    let mut options = EngineOptions::new(path.clone());
    options.save_interval_ms = interval;
    options.max_pending_bytes = budget;
    let engine = Engine::start(options, Arc::new(|| {})).unwrap();
    assert!(matches!(
        wait(&engine, |e| matches!(e, Event::Ready(_))),
        Event::Ready(_)
    ));
    (temp, engine, path)
}
fn wait(engine: &Engine, accept: impl Fn(&Event) -> bool) -> Event {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match engine.try_recv() {
            Ok(e) if accept(&e) => return e,
            Ok(_) => (),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < until, "event timed out");
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("engine disconnected: {e}"),
        }
    }
}
fn search(engine: &Engine, text: &str) -> Vec<ItemSummary> {
    engine
        .send(Command::Search(SearchQuery {
            text: text.into(),
            scope: Scope::All,
            request_id: 1,
        }))
        .unwrap();
    match wait(engine, |e| {
        matches!(e, Event::Results { request_id: 1, .. })
    }) {
        Event::Results { items, .. } => items,
        _ => unreachable!(),
    }
}
fn count(path: &std::path::Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM items", [], |r| r.get(0))
        .unwrap()
}
fn shutdown(engine: &Engine) {
    engine.send(Command::Shutdown).unwrap();
    wait(engine, |e| matches!(e, Event::ExitReady));
}

#[test]
fn capture_is_searchable_before_disk_persistence() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("尚未保存"))).unwrap();
    assert_eq!(search(&engine, "尚未")[0].title, "尚未保存");
    assert_eq!(count(&path), 0);
    shutdown(&engine);
}
#[test]
fn shutdown_commits_text_and_binary_atomically_and_releases_database() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("最终内容"))).unwrap();
    shutdown(&engine);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT text FROM items", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "最终内容"
    );
    assert_eq!(
        db.query_row(
            "SELECT data FROM representations WHERE format='PNG'",
            [],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        vec![0, 255, 7, 13]
    );
    db.execute_batch("BEGIN EXCLUSIVE; COMMIT;").unwrap();
}
#[test]
fn later_capture_does_not_move_first_dirty_deadline() {
    let (_temp, engine, path) = start(300, 16 * 1024 * 1024);
    let first_dirty = Instant::now();
    engine.send(Command::Capture(snapshot("first"))).unwrap();
    search(&engine, "first");
    thread::sleep(Duration::from_millis(200));
    engine.send(Command::Capture(snapshot("second"))).unwrap();
    let saved = wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert!(
        first_dirty.elapsed() < Duration::from_millis(450),
        "first dirty deadline was postponed"
    );
    assert!(matches!(saved, Event::Saved(s) if s.commits == 1));
    assert_eq!(count(&path), 2);
    shutdown(&engine);
}
#[test]
fn favorites_and_settings_survive_immediate_reopening() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: None,
            kind: ItemKind::Folder,
            title: "工作目录".into(),
            target: "C:\\work".into(),
            alias: "wk".into(),
            tags: vec!["项目".into()],
            open_with: None,
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&path), 1);
    let settings = Settings {
        theme: Theme::Dark,
        ..Settings::default()
    };
    engine
        .send(Command::SaveSettings(settings.clone()))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let db = Connection::open(&path).unwrap();
    let persisted: String = db
        .query_row("SELECT value FROM settings WHERE key='settings'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Settings>(&persisted).unwrap(),
        settings
    );
    assert_eq!(search(&engine, "wk")[0].title, "工作目录");
    shutdown(&engine);
}
#[test]
fn deletes_hide_pending_and_saved_items() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("pending"))).unwrap();
    let id = search(&engine, "pending")[0].id.clone();
    engine.send(Command::Delete(id)).unwrap();
    assert!(search(&engine, "pending").is_empty());
    engine.send(Command::Capture(snapshot("saved"))).unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let id = search(&engine, "saved")[0].id.clone();
    engine.send(Command::Delete(id)).unwrap();
    assert!(search(&engine, "saved").is_empty());
    shutdown(&engine);
    assert_eq!(count(&path), 0);
}
#[test]
fn failed_shutdown_preserves_pending_cancel_restores_and_retry_is_idempotent() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("保留"))).unwrap();
    let id = search(&engine, "保留")[0].id.clone();
    engine.send(Command::Used(id.clone())).unwrap();
    engine.send(Command::Used(id)).unwrap();
    search(&engine, "保留");
    let lock = Connection::open(&path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine.send(Command::Shutdown).unwrap();
    assert!(matches!(
        wait(&engine, |e| matches!(e, Event::Error { exiting: true, .. })),
        Event::Error { exiting: true, .. }
    ));
    assert_eq!(search(&engine, "保留")[0].use_count, 2);
    assert_eq!(count(&path), 0);
    engine.send(Command::CancelShutdown).unwrap();
    engine.send(Command::Capture(snapshot("恢复"))).unwrap();
    assert_eq!(search(&engine, "恢复").len(), 1);
    lock.execute_batch("ROLLBACK").unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    engine.send(Command::Flush).unwrap();
    assert_eq!(search(&engine, "保留")[0].use_count, 2);
    shutdown(&engine);
    let db = Connection::open(&path).unwrap();
    assert_eq!(count(&path), 2);
    assert_eq!(
        db.query_row(
            "SELECT use_count FROM items WHERE title='保留'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}
#[test]
fn byte_pressure_flushes_and_oversized_item_is_rejected() {
    let (_temp, engine, path) = start(60_000, 40);
    engine
        .send(Command::Capture(snapshot(
            "enough bytes to cross the budget",
        )))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&path), 1);
    shutdown(&engine);
    let temp = tempfile::tempdir().unwrap();
    let mut options = EngineOptions::new(temp.path().join("small.sqlite"));
    options.max_item_bytes = 3;
    let small = Engine::start(options, Arc::new(|| {})).unwrap();
    wait(&small, |e| matches!(e, Event::Ready(_)));
    small.send(Command::Capture(snapshot("too large"))).unwrap();
    wait(&small, |e| matches!(e, Event::Error { .. }));
    assert!(search(&small, "").is_empty());
    shutdown(&small);
}
#[test]
fn idle_flushes_do_not_commit_and_format_distinctions_survive_deduplication() {
    let (_temp, engine, path) = start(30, 16 * 1024 * 1024);
    engine.send(Command::Flush).unwrap();
    search(&engine, "");
    thread::sleep(Duration::from_millis(80));
    while let Ok(event) = engine.try_recv() {
        assert!(!matches!(event, Event::Saved(_)));
    }
    engine.send(Command::Capture(snapshot("same"))).unwrap();
    engine.send(Command::Capture(snapshot("same"))).unwrap();
    let mut rich = snapshot("same");
    rich.representations.push(Representation {
        format: "HTML Format".into(),
        data: b"<b>same</b>".to_vec(),
    });
    engine.send(Command::Capture(rich)).unwrap();
    assert_eq!(search(&engine, "same").len(), 2);
    shutdown(&engine);
    assert_eq!(count(&path), 2);
}
#[test]
fn persisted_search_uses_chinese_index_and_pin_changes_do_not_drop_blobs() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("中文项目资料")))
        .unwrap();
    shutdown(&engine);
    drop(engine);
    let reopened = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&reopened, |e| matches!(e, Event::Ready(_)));
    let items = search(&reopened, "项目");
    assert_eq!(items.len(), 1);
    reopened
        .send(Command::Pin {
            id: items[0].id.clone(),
            pinned: true,
        })
        .unwrap();
    reopened.send(Command::Used(items[0].id.clone())).unwrap();
    shutdown(&reopened);
    let db = Connection::open(path).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM representations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn pure_symbols_do_not_return_unrelated_saved_items() {
    let (_temp, engine, _path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("ordinary text")))
        .unwrap();
    engine
        .send(Command::Capture(snapshot("!!! important")))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(
        search(&engine, "!!!")
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        vec!["!!! important"]
    );
    shutdown(&engine);
}
#[test]
fn retention_bounds_history_but_preserves_pinned_items_and_favorites() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let settings = Settings {
        max_history: 1,
        ..Settings::default()
    };
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    engine.send(Command::Capture(snapshot("pinned"))).unwrap();
    let id = search(&engine, "pinned")[0].id.clone();
    engine.send(Command::Pin { id, pinned: true }).unwrap();
    engine.send(Command::Capture(snapshot("newer"))).unwrap();
    engine.send(Command::Capture(snapshot("newest"))).unwrap();
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: None,
            kind: ItemKind::Document,
            title: "keep document".into(),
            target: "C:\\doc.txt".into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&path), 3);
    assert_eq!(search(&engine, "pinned").len(), 1);
    shutdown(&engine);
}

#[test]
fn empty_query_ranks_pinned_then_favorites_then_recent_history() {
    let (_temp, engine, _path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: None,
            kind: ItemKind::Folder,
            title: "常用文件夹".into(),
            target: "C:\\favorites".into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    engine.send(Command::Capture(snapshot("固定历史"))).unwrap();
    let pinned = search(&engine, "固定")[0].id.clone();
    engine
        .send(Command::Pin {
            id: pinned,
            pinned: true,
        })
        .unwrap();
    let mut recent = snapshot("最新历史");
    recent.captured_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 2_000;
    engine.send(Command::Capture(recent)).unwrap();
    assert_eq!(
        search(&engine, "")
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        vec!["固定历史", "常用文件夹", "最新历史"]
    );
    shutdown(&engine);
}

#[test]
fn first_dirty_deadline_fires_even_when_command_queue_is_busy() {
    let (_temp, engine, path) = start(10, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("busy queue")))
        .unwrap();
    for request_id in 10..20_000 {
        engine
            .send(Command::Search(SearchQuery {
                text: "busy".into(),
                scope: Scope::All,
                request_id,
            }))
            .unwrap();
    }
    let until = Instant::now() + Duration::from_millis(150);
    while count(&path) == 0 && Instant::now() < until {
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(count(&path), 1, "queued reads postponed the save deadline");
    shutdown(&engine);
}

#[test]
fn queued_capture_payload_has_a_byte_budget_during_a_blocked_writer() {
    let (_temp, engine, path) = start(60_000, 1_024);
    engine.send(Command::Capture(snapshot("initial"))).unwrap();
    search(&engine, "initial");
    let lock = Connection::open(&path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine.send(Command::Flush).unwrap();
    thread::sleep(Duration::from_millis(10));
    let mut refused = 0;
    for _ in 0..100 {
        if engine
            .send(Command::Capture(snapshot(&"x".repeat(400))))
            .is_err()
        {
            refused += 1;
        }
    }
    assert!(
        refused > 0,
        "the blocked writer accepted an unbounded payload queue"
    );
    lock.execute_batch("ROLLBACK").unwrap();
    shutdown(&engine);
}

#[test]
fn failed_save_retries_back_off_instead_of_attempting_every_second() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("retry pacing")))
        .unwrap();
    search(&engine, "retry");
    let lock = Connection::open(&path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    let until = Instant::now() + Duration::from_millis(2_450);
    let mut errors = 0;
    while Instant::now() < until {
        while let Ok(event) = engine.try_recv() {
            if matches!(event, Event::Error { .. }) {
                errors += 1;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(errors <= 1, "save failures retried every second: {errors}");
    lock.execute_batch("ROLLBACK").unwrap();
    shutdown(&engine);
}

#[test]
fn exact_alias_survives_candidate_limit_against_newer_common_prefixes() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("needle archive")))
        .unwrap();
    let archived = search(&engine, "archive")[0].id.clone();
    for index in 0..600 {
        let mut clip = snapshot(&format!("needle recent {index}"));
        clip.captured_at_ms += index + 1;
        engine.send(Command::Capture(clip)).unwrap();
    }
    shutdown(&engine);
    drop(engine);
    let db = Connection::open(&path).unwrap();
    db.execute(
        "UPDATE items SET summary=json_set(summary,'$.alias','needle') WHERE id=?1",
        [&archived],
    )
    .unwrap();
    drop(db);
    let reopened = Engine::start(EngineOptions::new(path), Arc::new(|| {})).unwrap();
    wait(&reopened, |e| matches!(e, Event::Ready(_)));
    assert_eq!(search(&reopened, "needle")[0].id, archived);
    shutdown(&reopened);
}

#[test]
fn usage_overlay_flushes_under_the_same_pending_byte_pressure() {
    let (_temp, engine, _path) = start(60_000, 100);
    for index in 0..4 {
        engine
            .send(Command::Capture(snapshot(&format!(
                "item {index} {}",
                "x".repeat(30)
            ))))
            .unwrap();
        search(&engine, "");
    }
    engine.send(Command::Flush).unwrap();
    let items = search(&engine, "");
    while engine.try_recv().is_ok() {}
    for item in items {
        engine.send(Command::Used(item.id)).unwrap();
    }
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    shutdown(&engine);
}
