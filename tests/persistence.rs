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
    wait_with_budget(engine, accept, Duration::from_secs(5))
}
fn wait_with_budget(engine: &Engine, accept: impl Fn(&Event) -> bool, budget: Duration) -> Event {
    let until = Instant::now() + budget;
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
fn unified_search_keeps_pending_content_visible_without_persisting_functions() {
    use quick_panel::palette::{self, Context, Entry, Function};
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("设置操作说明")))
        .unwrap();
    let pending = palette::results("设置", search(&engine, "设置"), Context::default());
    assert_eq!(pending[0].function(), Some(Function::Settings));
    let content_id = pending.iter().find_map(Entry::saved).unwrap().id.clone();
    assert_eq!(count(&path), 0);
    for _ in 0..5 {
        let entries = palette::results("设置", search(&engine, "设置"), Context::default());
        assert!(entries.iter().any(|entry| entry.id() == content_id));
        assert_eq!(count(&path), 0, "search must not flush pending data");
    }
    engine.send(Command::Flush).unwrap();
    wait(&engine, |event| matches!(event, Event::Saved(_)));
    let persisted = palette::results("设置", search(&engine, "设置"), Context::default());
    assert_eq!(persisted[0].function(), Some(Function::Settings));
    assert!(persisted.iter().any(|entry| entry.id() == content_id));
    assert_eq!(count(&path), 1, "function rows never enter saved storage");
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
        hotkey_modifiers: 6,
        hotkey_key: 80,
        clipboard_hotkey_modifiers: 0,
        clipboard_hotkey_key: 0,
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
    assert_eq!(search(&engine, "pinned").len(), 1);
    assert_eq!(count(&path), 3);
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
    engine.send(Command::Shutdown).unwrap();
    // Drain all 20k accepted queries; this lifecycle wait is separate from the
    // unchanged 150ms assertion on the first dirty disk deadline above.
    wait_with_budget(
        &engine,
        |e| matches!(e, Event::ExitReady),
        Duration::from_secs(30),
    );
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
fn usage_overlay_keeps_only_metadata_and_remains_delayed() {
    let (_temp, engine, path) = start(60_000, 100);
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
    assert!(search(&engine, "").iter().all(|item| item.use_count == 1));
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT sum(use_count) FROM items", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    while let Ok(event) = engine.try_recv() {
        assert!(!matches!(event, Event::Saved(_)));
    }
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    shutdown(&engine);
}

#[test]
fn retention_and_usage_preserve_the_used_items_original_representations() {
    let (_temp, engine, path) = start(60_000, 100);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let clip = |ch: char, time: i64, blobs: bool| ClipboardSnapshot {
        text: ch.to_string().repeat(80),
        representations: if blobs {
            vec![Representation {
                format: "PNG".into(),
                data: vec![1, 2, 3],
            }]
        } else {
            vec![]
        },
        source_app: "editor".into(),
        captured_at_ms: time,
    };
    engine
        .send(Command::Capture(clip('b', now - 10_000, true)))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    let id = search(&engine, "")[0].id.clone();
    engine
        .send(Command::SaveSettings(Settings {
            max_history: 1,
            ..Settings::default()
        }))
        .unwrap();
    search(&engine, "");
    engine
        .send(Command::Capture(clip('a', now - 1_000, false)))
        .unwrap();
    search(&engine, "");
    engine.send(Command::Used(id.clone())).unwrap();
    engine.send(Command::Flush).unwrap();
    search(&engine, "");
    engine.send(Command::Load { id, request_id: 22 }).unwrap();
    match wait(&engine, |e| {
        matches!(e, Event::Loaded { request_id: 22, .. })
    }) {
        Event::Loaded {
            item: Some(item), ..
        } => assert_eq!(
            item.representations,
            vec![Representation {
                format: "PNG".into(),
                data: vec![1, 2, 3]
            }]
        ),
        other => panic!("used item disappeared: {other:?}"),
    }
    assert_eq!(count(&path), 1);
    shutdown(&engine);
}
#[test]
fn metadata_updates_and_duplicate_capture_do_not_rewrite_content_index_or_blobs() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("same payload")))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    let id = search(&engine, "")[0].id.clone();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE content_writes(n INTEGER); INSERT INTO content_writes VALUES(0); CREATE TRIGGER body_update AFTER UPDATE OF text ON items BEGIN UPDATE content_writes SET n=n+1; END; CREATE TRIGGER blob_delete AFTER DELETE ON representations BEGIN UPDATE content_writes SET n=n+1; END;").unwrap();
    let original_index: String = db
        .query_row("SELECT content FROM item_search", [], |r| r.get(0))
        .unwrap();
    // A sentinel makes any DELETE/INSERT index rewrite observable without depending on SQL source text.
    db.execute(
        "UPDATE item_search SET content=content || ' sentinelunchanged'",
        [],
    )
    .unwrap();
    engine.send(Command::Used(id.clone())).unwrap();
    engine.send(Command::Pin { id, pinned: true }).unwrap();
    search(&engine, "");
    assert_eq!(
        db.query_row("SELECT pinned FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1,
        "Pin must commit immediately"
    );
    engine
        .send(Command::Capture(snapshot("same payload")))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    search(&engine, "");
    assert_eq!(
        db.query_row("SELECT n FROM content_writes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT content FROM item_search", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        original_index + " sentinelunchanged"
    );
    shutdown(&engine);
}
#[test]
fn repeated_pressure_capture_respects_the_failed_batch_retry_gate() {
    let (_temp, engine, path) = start(60_000, 100);
    let make = |ch: char| ClipboardSnapshot {
        text: ch.to_string().repeat(80),
        representations: vec![],
        source_app: "editor".into(),
        captured_at_ms: 1_791_000_000_000,
    };
    engine.send(Command::Capture(make('a'))).unwrap();
    search(&engine, "");
    let db = Connection::open(path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    let started = Instant::now();
    let mut attempts = 0;
    for ch in ['b', 'c', 'd', 'e', 'f'] {
        engine.send(Command::Capture(make(ch))).unwrap();
        engine
            .send(Command::Search(SearchQuery {
                text: String::new(),
                scope: Scope::All,
                request_id: 99,
            }))
            .unwrap();
        loop {
            match wait(&engine, |_| true) {
                Event::Results { request_id: 99, .. } => break,
                Event::Error { message, .. } if message.contains("locked") => attempts += 1,
                _ => (),
            }
        }
    }
    assert_eq!(
        attempts, 0,
        "pressure inputs retried SQL before the retry gate"
    );
    assert!(started.elapsed() < Duration::from_millis(300));
    db.execute_batch("ROLLBACK").unwrap();
    shutdown(&engine);
}
#[test]
fn persisted_body_search_uses_bounded_sqlite_memory() {
    if std::env::var_os("QP_MEMORY_CHILD").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("persisted_body_search_uses_bounded_sqlite_memory")
            .arg("--nocapture")
            .env("QP_MEMORY_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        );
        return;
    }
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let db = Connection::open(path).unwrap();
    let now = 1_791_000_000_000i64;
    let body = "needle ".repeat(37_449);
    let tx = db.unchecked_transaction().unwrap();
    for i in 0..100 {
        let s = ItemSummary {
            id: format!("memory:{i}"),
            kind: ItemKind::Clipboard,
            title: format!("candidate {i}"),
            subtitle: String::new(),
            alias: String::new(),
            tags: vec![],
            pinned: false,
            created_at_ms: now,
            last_used_ms: now,
            use_count: 0,
        };
        tx.execute(
            "INSERT INTO items VALUES(?1,0,?2,0,?3,?3,0,?4,?5,NULL,NULL)",
            rusqlite::params![s.id, s.title, now, serde_json::to_string(&s).unwrap(), body],
        )
        .unwrap();
        tx.execute("INSERT INTO item_search(rowid,content) SELECT rowid,'n e d l ne ee ed dl le nee eed edl dle' FROM items WHERE id=?1",[s.id]).unwrap();
    }
    tx.commit().unwrap();
    let (mut current, mut high) = (0i64, 0i64);
    unsafe {
        rusqlite::ffi::sqlite3_status64(
            rusqlite::ffi::SQLITE_STATUS_MEMORY_USED,
            &mut current,
            &mut high,
            1,
        );
    }
    let baseline = current;
    engine
        .send(Command::Search(SearchQuery {
            text: "needle".into(),
            scope: Scope::All,
            request_id: 55,
        }))
        .unwrap();
    let results = wait_with_budget(
        &engine,
        |e| matches!(e, Event::Results { request_id: 55, .. }),
        Duration::from_secs(30),
    );
    assert!(matches!(results,Event::Results{items,..} if items.len()==50));
    unsafe {
        rusqlite::ffi::sqlite3_status64(
            rusqlite::ffi::SQLITE_STATUS_MEMORY_USED,
            &mut current,
            &mut high,
            0,
        );
    }
    println!(
        "SQLite search incremental high-water: {} bytes",
        high - baseline
    );
    assert!(
        high - baseline < 16 * 1024 * 1024,
        "SQLite candidate sorting retained bodies: {} bytes",
        high - baseline
    );
    shutdown(&engine);
}

#[test]
fn old_fts_index_is_migrated_once_without_changing_original_representations() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("myreport"))).unwrap();
    shutdown(&engine);
    drop(engine);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO settings(key,value) VALUES('index_version','1') ON CONFLICT(key) DO UPDATE SET value='1'; UPDATE item_search SET content='myreport';").unwrap();
    drop(db);
    let reopened = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&reopened, |e| matches!(e, Event::Ready(_)));
    assert_eq!(search(&reopened, "report").len(), 1);
    shutdown(&reopened);
    drop(reopened);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT value FROM settings WHERE key='index_version'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        quick_panel::search::INDEX_VERSION.to_string()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM representations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    db.execute(
        "UPDATE item_search SET content=content || ' migrationpreserved'",
        [],
    )
    .unwrap();
    drop(db);
    let again = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&again, |e| matches!(e, Event::Ready(_)));
    shutdown(&again);
    let db = Connection::open(path).unwrap();
    assert!(
        db.query_row("SELECT content FROM item_search", [], |r| r
            .get::<_, String>(0))
            .unwrap()
            .ends_with(" migrationpreserved")
    );
}

#[test]
#[ignore = "真实100k Release性能与特殊候选回归"]
fn hundred_thousand_search_preserves_special_candidates_under_sixty_ms() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let mut db = Connection::open(&path).unwrap();
    let now = 1_791_000_000_000i64;
    let tx = db.transaction().unwrap();
    for i in 0..100_000 {
        let summary = ItemSummary {
            id: format!("perf:{i}"),
            kind: if i == 3 {
                ItemKind::Folder
            } else {
                ItemKind::Clipboard
            },
            title: if i == 1 {
                "needle".into()
            } else {
                format!("needle common {i}")
            },
            subtitle: String::new(),
            alias: if i == 0 {
                "needle".into()
            } else {
                String::new()
            },
            tags: vec![],
            pinned: i == 2,
            created_at_ms: now + i,
            last_used_ms: if i == 4 { now + 2_000_000 } else { now + i },
            use_count: if i == 4 { 50 } else { 0 },
        };
        tx.execute(
            "INSERT INTO items VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?3,NULL,NULL)",
            rusqlite::params![
                summary.id,
                if i == 3 { 1 } else { 0 },
                summary.title,
                summary.pinned,
                summary.created_at_ms,
                summary.last_used_ms,
                summary.use_count as i64,
                serde_json::to_string(&summary).unwrap()
            ],
        )
        .unwrap();
        tx.execute("INSERT INTO item_search(rowid,content) SELECT rowid,'n e d l ne ee ed dl le nee eed edl dle' FROM items WHERE id=?1",[summary.id]).unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(count(&path), 100_000);
    let mut elapsed = vec![];
    for _ in 0..11 {
        let start = Instant::now();
        let items = search(&engine, "needle");
        elapsed.push(start.elapsed().as_secs_f64() * 1000.);
        for id in ["perf:0", "perf:1", "perf:2", "perf:3", "perf:4"] {
            assert!(
                items.iter().any(|item| item.id == id),
                "old exact/pin/favorite/Used candidate disappeared: {id}"
            );
        }
    }
    elapsed.remove(0);
    elapsed.sort_by(f64::total_cmp);
    println!(
        "actual_items=100000 queries=10 p50_ms={:.3} p95_ms={:.3}",
        elapsed[4], elapsed[9]
    );
    assert!(
        elapsed[9] <= 60.,
        "100k query exceeded target: {:.3}ms",
        elapsed[9]
    );
    shutdown(&engine);
}

#[test]
fn scoped_search_keeps_older_favorites_after_many_new_clipboard_hits() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let mut db = Connection::open(path).unwrap();
    let tx = db.transaction().unwrap();
    for index in 0..801 {
        let kind = if index <= 200 {
            ItemKind::Folder
        } else {
            ItemKind::Clipboard
        };
        let summary = ItemSummary {
            id: format!("scope:{index}"),
            kind,
            title: if index == 0 {
                "archive".into()
            } else if kind == ItemKind::Folder {
                format!("distractor {index}")
            } else {
                format!("needle clip {index}")
            },
            subtitle: String::new(),
            alias: String::new(),
            tags: vec![],
            pinned: false,
            created_at_ms: 1_791_000_000_000 + index,
            last_used_ms: 1_791_000_000_000 + index,
            use_count: 0,
        };
        let target = if index == 0 {
            Some("C:/needle/archive".to_string())
        } else {
            None
        };
        tx.execute(
            "INSERT INTO items VALUES(?1,?2,?3,0,?4,?4,0,?5,'',?6,NULL)",
            rusqlite::params![
                summary.id,
                if kind == ItemKind::Folder { 1 } else { 0 },
                summary.title,
                summary.created_at_ms,
                serde_json::to_string(&summary).unwrap(),
                target
            ],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO item_search(rowid,content) SELECT rowid,?2 FROM items WHERE id=?1",
            rusqlite::params![
                summary.id,
                if index == 0 || kind == ItemKind::Clipboard {
                    "nee eed edl dle"
                } else {
                    "zzz"
                }
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    engine
        .send(Command::Search(SearchQuery {
            text: "needle".into(),
            scope: Scope::Folder,
            request_id: 33,
        }))
        .unwrap();
    match wait(&engine, |e| {
        matches!(e, Event::Results { request_id: 33, .. })
    }) {
        Event::Results { items, .. } => assert_eq!(
            items.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["scope:0"]
        ),
        _ => unreachable!(),
    };
    shutdown(&engine);
}

#[test]
#[ignore = "真实bulk WAL空间边界Release回归"]
fn continuous_bulk_saving_keeps_wal_under_emergency_pressure() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::SaveSettings(Settings {
            max_history: 100_000,
            ..Settings::default()
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let mut wal_path = path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let wal_path = std::path::PathBuf::from(wal_path);
    let mut peak = 0;
    let mut previous = 0;
    let mut reuse_cycles = 0;
    for batch in 0..100 {
        for index in batch * 1000..(batch + 1) * 1000 {
            engine
                .send(Command::Capture(ClipboardSnapshot {
                    text: format!(
                        "项目{index} 接口说明 周报第{index}期 C:/Projects/project-{index}/readme.md"
                    ),
                    representations: vec![],
                    source_app: "core-check".into(),
                    captured_at_ms: 1_791_000_000_000,
                }))
                .unwrap();
        }
        engine.send(Command::Flush).unwrap();
        wait(&engine, |e| matches!(e, Event::Saved(_)));
        let size = std::fs::metadata(&wal_path).unwrap().len();
        peak = peak.max(size);
        if size < previous {
            reuse_cycles += 1;
        }
        previous = size;
        println!(
            "bulk_items={} wal_bytes={size} peak_bytes={peak}",
            (batch + 1) * 1000
        );
        assert!(
            size <= 80 * 1024 * 1024,
            "bulk WAL exceeded emergency bound: {size}"
        );
    }
    assert_eq!(count(&path), 100_000);
    println!("actual_items=100000 wal_peak_bytes={peak} wal_reuse_cycles={reuse_cycles}");
    assert!(reuse_cycles > 0);
    shutdown(&engine);
}

#[test]
fn repeated_capture_of_an_old_row_preserves_recent_search_position() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let original = snapshot("archive needle item");
    engine.send(Command::Capture(original.clone())).unwrap();
    engine.send(Command::Flush).unwrap();
    let id = search(&engine, "needle")[0].id.clone();
    let mut db = Connection::open(path).unwrap();
    let tx = db.transaction().unwrap();
    for i in 0..600 {
        let s = ItemSummary {
            id: format!("recent:{i}"),
            kind: ItemKind::Clipboard,
            title: format!("ordinary needle item {i}"),
            subtitle: String::new(),
            alias: String::new(),
            tags: vec![],
            pinned: false,
            created_at_ms: original.captured_at_ms + i + 1,
            last_used_ms: original.captured_at_ms + i + 1,
            use_count: 0,
        };
        tx.execute(
            "INSERT INTO items VALUES(?1,0,?2,0,?3,?3,0,?4,?2,NULL,NULL)",
            rusqlite::params![
                s.id,
                s.title,
                s.created_at_ms,
                serde_json::to_string(&s).unwrap()
            ],
        )
        .unwrap();
        tx.execute("INSERT INTO item_search(rowid,content) SELECT rowid,'nee eed edl dle' FROM items WHERE id=?1",[s.id]).unwrap();
    }
    tx.commit().unwrap();
    let mut repeated = original;
    repeated.captured_at_ms += 2_000_000;
    engine.send(Command::Capture(repeated)).unwrap();
    engine.send(Command::Flush).unwrap();
    assert_eq!(
        search(&engine, "needle")[0].id,
        id,
        "old rowid hid a newly captured duplicate"
    );
    shutdown(&engine);
}

#[test]
fn auxiliary_candidates_keep_body_only_matches_for_old_pin_usage_and_recapture() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let clips = [
        snapshot("pinned preview\nneedle body"),
        snapshot("used preview\nneedle body"),
        snapshot("recent preview\nneedle body"),
    ];
    for clip in &clips {
        engine.send(Command::Capture(clip.clone())).unwrap();
    }
    engine.send(Command::Flush).unwrap();
    let initial = search(&engine, "needle");
    let pinned = initial
        .iter()
        .find(|s| s.title == "pinned preview")
        .unwrap()
        .id
        .clone();
    let used = initial
        .iter()
        .find(|s| s.title == "used preview")
        .unwrap()
        .id
        .clone();
    let recent = initial
        .iter()
        .find(|s| s.title == "recent preview")
        .unwrap()
        .id
        .clone();
    engine
        .send(Command::Pin {
            id: pinned.clone(),
            pinned: true,
        })
        .unwrap();
    engine.send(Command::Used(used.clone())).unwrap();
    engine.send(Command::Flush).unwrap();
    search(&engine, "");
    let mut db = Connection::open(path).unwrap();
    let tx = db.transaction().unwrap();
    for i in 0..600 {
        let s = ItemSummary {
            id: format!("body:{i}"),
            kind: ItemKind::Clipboard,
            title: format!("ordinary {i}"),
            subtitle: String::new(),
            alias: String::new(),
            tags: vec![],
            pinned: false,
            created_at_ms: clips[0].captured_at_ms + i + 1,
            last_used_ms: clips[0].captured_at_ms + i + 1,
            use_count: 0,
        };
        tx.execute(
            "INSERT INTO items VALUES(?1,0,?2,0,?3,?3,0,?4,'needle payload',NULL,NULL)",
            rusqlite::params![
                s.id,
                s.title,
                s.created_at_ms,
                serde_json::to_string(&s).unwrap()
            ],
        )
        .unwrap();
        tx.execute("INSERT INTO item_search(rowid,content) SELECT rowid,'nee eed edl dle' FROM items WHERE id=?1",[s.id]).unwrap();
    }
    tx.commit().unwrap();
    let mut repeated = clips[2].clone();
    repeated.captured_at_ms += 2_000_000;
    engine.send(Command::Capture(repeated)).unwrap();
    engine.send(Command::Flush).unwrap();
    let items = search(&engine, "needle");
    assert_eq!(items[0].id, pinned);
    assert!(items.iter().any(|s| s.id == used));
    assert!(items.iter().any(|s| s.id == recent));
    shutdown(&engine);
}

#[test]
fn final_review_gram_collision_pages_preserve_old_title_body_and_infix() {
    let (_temp, engine, _) = start(60_000, 16 * 1024 * 1024);
    for text in [
        "archive qwerty item",
        "Archived unique\nqwerty",
        "archive zzqwertyzz item",
    ] {
        engine.send(Command::Capture(snapshot(text))).unwrap();
    }
    engine.send(Command::Flush).unwrap();
    assert_eq!(search(&engine, "qwerty").len(), 3);
    for index in 0..1100 {
        let mut clip = snapshot(&format!("new number {index}\nqwe wer ert rty"));
        clip.captured_at_ms += index + 1;
        engine.send(Command::Capture(clip)).unwrap();
    }
    assert_eq!(search(&engine, "qwerty").len(), 3);
    engine.send(Command::Flush).unwrap();
    assert_eq!(search(&engine, "qwerty").len(), 3);
    shutdown(&engine);
}

#[test]
fn final_review_old_interior_title_outranks_recent_body_hits() {
    let (_temp, engine, _) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("archive zzqwertyzz item")))
        .unwrap();
    engine
        .send(Command::Capture(snapshot("我的周报模板")))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    for index in 0..600 {
        let mut clip = snapshot(&format!("new number {index}\nqwerty baomoban"));
        clip.captured_at_ms += index + 1;
        engine.send(Command::Capture(clip)).unwrap();
    }
    engine.send(Command::Flush).unwrap();
    assert_eq!(
        search(&engine, "qwerty")[0].title,
        "archive zzqwertyzz item"
    );
    assert_eq!(search(&engine, "baomoban")[0].title, "我的周报模板");
    shutdown(&engine);
}

#[test]
fn final_review_favorite_delete_is_durable_but_history_is_batched() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    for kind in [ItemKind::Document, ItemKind::Folder] {
        engine
            .send(Command::SaveFavorite(FavoriteInput {
                id: Some("favorite:durable".into()),
                kind,
                title: "favorite".into(),
                target: "C:/demo".into(),
                alias: String::new(),
                tags: vec![],
                open_with: None,
            }))
            .unwrap();
        search(&engine, "");
        assert_eq!(count(&path), 1);
        engine
            .send(Command::Delete("favorite:durable".into()))
            .unwrap();
        search(&engine, "");
        assert_eq!(count(&path), 0);
    }
    engine.send(Command::Capture(snapshot("history"))).unwrap();
    engine.send(Command::Flush).unwrap();
    let id = search(&engine, "history")[0].id.clone();
    engine.send(Command::Delete(id)).unwrap();
    search(&engine, "");
    assert_eq!(count(&path), 1);
    shutdown(&engine);
}

#[test]
fn final_review_exit_failure_waits_for_explicit_retry_or_cancel() {
    let (_temp, engine, path) = start(200, 16 * 1024 * 1024);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine
        .send(Command::Capture(snapshot("unsaved exit")))
        .unwrap();
    engine.send(Command::Shutdown).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { exiting: true, .. }));
    thread::sleep(Duration::from_millis(1400));
    assert!(
        !engine
            .try_recv()
            .into_iter()
            .any(|e| matches!(e, Event::Error { exiting: true, .. })),
        "must not retry behind a modal prompt"
    );
    engine.send(Command::Shutdown).unwrap();
    wait(&engine, |e| matches!(e, Event::Error { exiting: true, .. }));
    db.execute_batch("ROLLBACK").unwrap();
    thread::sleep(Duration::from_millis(2200));
    assert_eq!(
        count(&path),
        0,
        "unlocking must not save or exit until a user decision"
    );
    engine.send(Command::CancelShutdown).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&path), 1);
    shutdown(&engine);
}

#[test]
fn final_review_save_window_reserves_commit_time() {
    let mut options = EngineOptions::new("unused.db".into());
    assert_eq!(options.save_start_delay_ms(), 55_000);
    options.save_interval_ms = 1_200;
    assert_eq!(options.save_start_delay_ms(), 1_100);
    options.save_interval_ms = 12;
    assert_eq!(options.save_start_delay_ms(), 11);
    let (_temp, engine, path) = start(1_200, 16 * 1024 * 1024);
    let accepted = Instant::now();
    engine.send(Command::Capture(snapshot("window"))).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert!(accepted.elapsed() < Duration::from_millis(1_200));
    assert_eq!(count(&path), 1);
    shutdown(&engine);
}

#[test]
fn final_review_pinned_history_delete_is_immediately_durable() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("pinned clip")))
        .unwrap();
    let id = search(&engine, "pinned")[0].id.clone();
    engine
        .send(Command::Pin {
            id: id.clone(),
            pinned: true,
        })
        .unwrap();
    search(&engine, "");
    assert_eq!(count(&path), 1);
    engine.send(Command::Delete(id)).unwrap();
    search(&engine, "");
    assert_eq!(count(&path), 0);
    engine
        .send(Command::Delete("already missing".into()))
        .unwrap();
    search(&engine, "");
    shutdown(&engine);
}

#[test]
fn final_review_favorite_edit_updates_heading_and_phonetic_postings() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    for (title, alias) in [("archive qwerty", "oldalias"), ("我的周报模板", "newalias")] {
        engine
            .send(Command::SaveFavorite(FavoriteInput {
                id: Some("favorite:edited".into()),
                kind: ItemKind::Document,
                title: title.into(),
                target: "C:/demo.txt".into(),
                alias: alias.into(),
                tags: vec![],
                open_with: None,
            }))
            .unwrap();
        assert_eq!(search(&engine, alias).len(), 1);
        assert_eq!(count(&path), 1);
    }
    assert!(search(&engine, "qwerty").is_empty());
    assert!(search(&engine, "oldalias").is_empty());
    assert_eq!(search(&engine, "zhoubaomoban").len(), 1);
    assert_eq!(search(&engine, "周报").len(), 1);
    let db = Connection::open(path).unwrap();
    for column in ["heading", "phonetic"] {
        let count: i64 = db
            .query_row(
                &format!("SELECT COUNT(*) FROM item_search WHERE {column}<>''"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    shutdown(&engine);
}

fn phonetic_prefix_survives_newer_infix_titles(queries: &[&str]) {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("周报模板"))).unwrap();
    engine.send(Command::Flush).unwrap();
    search(&engine, "zhoubao");
    for index in 0..200 {
        let mut clip = snapshot(&format!("我的周报模板{index}"));
        clip.captured_at_ms += index + 1;
        engine.send(Command::Capture(clip)).unwrap();
    }
    for query in queries {
        assert_eq!(
            search(&engine, query)[0].title,
            "周报模板",
            "before saving {query}"
        );
    }
    engine.send(Command::Flush).unwrap();
    let mut missing = vec![];
    for query in queries {
        let items = search(&engine, query);
        let expression = format!(
            "phonetic_prefix: {}",
            quick_panel::search::phonetic_prefix_expression(query)
        );
        let prefix_rows: i64 = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT count(*) FROM item_search WHERE item_search MATCH ?1",
                [expression],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            prefix_rows, 1,
            "infix rows leaked into prefix posting for {query}"
        );
        if items[0].title != "周报模板" {
            missing.push(format!("{query}: {}", items[0].title));
        }
    }
    assert!(missing.is_empty(), "prefix lost after saving: {missing:?}");
    shutdown(&engine);
}

#[test]
fn final_rereview_phonetic_prefix_survives_two_hundred_newer_infix_titles() {
    phonetic_prefix_survives_newer_infix_titles(&["zhoubao"]);
}

#[test]
fn final_rereview_initials_prefix_survives_two_hundred_newer_infix_titles() {
    phonetic_prefix_survives_newer_infix_titles(&["zbmb"]);
}

#[test]
fn final_rereview_short_prefixes_have_independent_postings_and_quota() {
    phonetic_prefix_survives_newer_infix_titles(&["z", "zh", "zho", "zb", "zbm"]);
}

#[test]
fn final_rereview_v3_upgrade_adds_prefix_postings_without_touching_blobs() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("周报模板"))).unwrap();
    shutdown(&engine);
    drop(engine);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("UPDATE settings SET value='3' WHERE key='index_version'; DROP TABLE item_search; CREATE VIRTUAL TABLE item_search USING fts5(content,heading,phonetic); INSERT INTO item_search(rowid,content) SELECT rowid,'old index' FROM items;").unwrap();
    let original: Vec<u8> = db
        .query_row(
            "SELECT data FROM representations WHERE format='PNG'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    let reopened = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&reopened, |e| matches!(e, Event::Ready(_)));
    assert_eq!(search(&reopened, "zhoubao")[0].title, "周报模板");
    shutdown(&reopened);
    drop(reopened);
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT value FROM settings WHERE key='index_version'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "4"
    );
    assert_eq!(
        db.query_row(
            "SELECT data FROM representations WHERE format='PNG'",
            [],
            |r| r.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        original
    );
    assert!(
        !db.query_row("SELECT phonetic_prefix FROM item_search", [], |r| r
            .get::<_, String>(0))
            .unwrap()
            .is_empty()
    );
    db.execute_batch("UPDATE item_search SET phonetic_prefix=phonetic_prefix || ' migrationkept'")
        .unwrap();
    drop(db);
    let reopened = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&reopened, |e| matches!(e, Event::Ready(_)));
    shutdown(&reopened);
    drop(reopened);
    let db = Connection::open(path).unwrap();
    assert!(
        db.query_row("SELECT phonetic_prefix FROM item_search", [], |r| r
            .get::<_, String>(0))
            .unwrap()
            .ends_with(" migrationkept")
    );
}

#[test]
fn blocked_writer_coalesces_queries_across_modes_without_losing_save() {
    use std::sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("coalesce.sqlite");
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let entered = Arc::new(AtomicBool::new(false));
    let armed = Arc::new(AtomicBool::new(false));
    let engine = Engine::start(EngineOptions::new(path.clone()), {
        let gate = gate.clone();
        let entered = entered.clone();
        let armed = armed.clone();
        Arc::new(move || {
            if armed.swap(false, Ordering::SeqCst) {
                entered.store(true, Ordering::SeqCst);
                let (lock, ready) = &*gate;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = ready.wait(open).unwrap();
                }
            }
        })
    })
    .unwrap();
    wait(&engine, |e| matches!(e, Event::Ready(_)));
    armed.store(true, Ordering::SeqCst);
    engine.send(Command::Capture(snapshot("最新内容"))).unwrap();
    while !entered.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    for request_id in 1..=30 {
        if request_id == 15 {
            engine
                .send(Command::SaveSettings(Settings::default()))
                .unwrap();
        }
        if request_id % 3 == 0 {
            engine
                .send(Command::Search(SearchQuery {
                    text: "最新".into(),
                    request_id,
                    ..Default::default()
                }))
                .unwrap();
        } else if request_id % 3 == 2 {
            engine
                .send(Command::SemanticSearch(
                    quick_panel::semantic::SemanticSearchQuery {
                        text: "最新".into(),
                        scope: Scope::All,
                        request_id,
                        profile: "superseded".into(),
                        vector: vec![1., 0.],
                    },
                ))
                .unwrap();
        } else {
            engine
                .send(Command::AiSearch(AiSearchQuery {
                    queries: vec!["最新".into()],
                    request_id,
                    ..Default::default()
                }))
                .unwrap();
        }
    }
    *gate.0.lock().unwrap() = true;
    gate.1.notify_one();
    let mut results = Vec::new();
    let mut saved = false;
    wait(&engine, |event| matches!(event, Event::Changed));
    let until = Instant::now() + Duration::from_secs(5);
    while !results.contains(&30) {
        match engine.try_recv() {
            Ok(Event::Results { request_id, .. }) => results.push(request_id),
            Ok(Event::Saved(_)) => saved = true,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            _ => {
                assert!(Instant::now() < until);
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert_eq!(results, vec![30]);
    assert!(saved);
    assert_eq!(count(&path), 1);
    // A second blocked batch proves a pending query cannot cross Shutdown.
    *gate.0.lock().unwrap() = false;
    entered.store(false, Ordering::SeqCst);
    armed.store(true, Ordering::SeqCst);
    engine
        .send(Command::Capture(snapshot("退出前内容")))
        .unwrap();
    while !entered.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    engine
        .send(Command::Search(SearchQuery {
            request_id: 77,
            ..Default::default()
        }))
        .unwrap();
    engine.send(Command::Shutdown).unwrap();
    *gate.0.lock().unwrap() = true;
    gate.1.notify_one();
    wait(&engine, |event| {
        assert!(!matches!(event, Event::Results { .. }));
        assert!(!matches!(event, Event::Error { .. }));
        matches!(event, Event::ExitReady)
    });
    assert_eq!(count(&path), 2);
}

#[test]
fn cjk_body_snippet_is_bounded_and_never_persisted() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let text = format!("标题\n{}目标片段\n后面的内容", "前文".repeat(100));
    engine.send(Command::Capture(snapshot(&text))).unwrap();
    let pending = search(&engine, "目标片段");
    assert!(pending[0].subtitle.contains("目标片段"));
    assert!(pending[0].subtitle.contains("editor.exe"));
    assert!(!pending[0].subtitle.contains('\n'));
    assert!(pending[0].subtitle.chars().count() <= 220);
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(search(&engine, "目标片段")[0].subtitle, pending[0].subtitle);
    let db = Connection::open(path).unwrap();
    let raw: (String, String) = db
        .query_row("SELECT summary,text FROM items", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<ItemSummary>(&raw.0)
            .unwrap()
            .subtitle,
        "editor.exe"
    );
    assert_eq!(raw.1, text);
    shutdown(&engine);
}

#[test]
fn pinned_clipboard_metadata_keeps_identity_formats_on_restart_and_recapture() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    let original = snapshot("原始正文\n完整格式");
    engine.send(Command::Capture(original.clone())).unwrap();
    let id = search(&engine, "原始")[0].id.clone();
    engine
        .send(Command::Pin {
            id: id.clone(),
            pinned: true,
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER no_blob_update BEFORE DELETE ON representations BEGIN SELECT RAISE(ABORT,'metadata rewrote blob'); END;").unwrap();
    engine
        .send(Command::EditClipboardMetadata(ClipboardMetadataInput {
            id: id.clone(),
            title: "我的模板".into(),
            alias: "custom".into(),
            tags: vec!["重要".into()],
        }))
        .unwrap();
    wait(&engine, |e| {
        matches!(e, Event::Saved(_) | Event::Error { .. })
    });
    assert_eq!(search(&engine, "custom")[0].title, "我的模板");
    assert_eq!(search(&engine, "重要")[0].id, id);
    shutdown(&engine);
    drop(engine);
    let engine = Engine::start(EngineOptions::new(path), Arc::new(|| {})).unwrap();
    wait(&engine, |e| matches!(e, Event::Ready(_)));
    engine.send(Command::Capture(original.clone())).unwrap();
    assert_eq!(search(&engine, "custom")[0].title, "我的模板");
    engine
        .send(Command::Load {
            id: id.clone(),
            request_id: 99,
        })
        .unwrap();
    match wait(&engine, |e| {
        matches!(e, Event::Loaded { request_id: 99, .. })
    }) {
        Event::Loaded {
            item: Some(item), ..
        } => {
            assert_eq!(item.summary.id, id);
            assert_eq!(item.text, original.text);
            assert_eq!(item.representations, original.representations);
        }
        _ => panic!("missing original"),
    }
    shutdown(&engine);
}

#[test]
fn small_settings_commit_skips_retention_until_policy_changes() {
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine.send(Command::Capture(snapshot("维护探针"))).unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE items SET last_used_ms=1", []).unwrap();
    let mut settings = Settings {
        theme: Theme::Dark,
        ..Default::default()
    };
    engine
        .send(Command::SaveSettings(settings.clone()))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(
        count(&path),
        1,
        "theme-only save must not run full retention"
    );
    settings.max_history = 0;
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(count(&path), 0);
    shutdown(&engine);
}

#[test]
fn load_sqlite_failure_returns_terminal_id_and_recovery_admits_new_action_load() {
    use quick_panel::background_job::ActionLoads;
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("读取恢复探针")))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let summary = search(&engine, "读取恢复").remove(0);
    let db = Connection::open(&path).unwrap();
    db.execute(
        "ALTER TABLE representations RENAME TO load_failure_formats",
        [],
    )
    .unwrap();
    let mut loads = ActionLoads::default();
    assert!(loads.begin(301));
    engine
        .send(Command::Load {
            id: summary.id.clone(),
            request_id: 301,
        })
        .unwrap();
    let failure = wait(&engine, |e| {
        matches!(
            e,
            Event::LoadFailed {
                request_id: 301,
                ..
            } | Event::Loaded {
                request_id: 301,
                ..
            } | Event::Error { .. }
        )
    });
    db.execute(
        "ALTER TABLE load_failure_formats RENAME TO representations",
        [],
    )
    .unwrap();
    match failure {
        Event::LoadFailed {
            request_id,
            message,
        } => {
            assert!(message.contains("representations"));
            loads.complete(request_id);
        }
        event => panic!("读取失败缺少带301代次的终态：{event:?}"),
    }
    assert!(loads.begin(302));
    engine
        .send(Command::Load {
            id: summary.id,
            request_id: 302,
        })
        .unwrap();
    match wait(&engine, |e| {
        matches!(
            e,
            Event::Loaded {
                request_id: 302,
                ..
            } | Event::LoadFailed {
                request_id: 302,
                ..
            }
        )
    }) {
        Event::Loaded {
            request_id,
            item: Some(item),
        } => {
            assert_eq!(item.text, "读取恢复探针");
            loads.complete(request_id);
        }
        event => panic!("修复数据库后读取未恢复：{event:?}"),
    }
    assert!(loads.begin(303));
    engine
        .send(Command::Load {
            id: "not-found".into(),
            request_id: 303,
        })
        .unwrap();
    match wait(&engine, |e| {
        matches!(
            e,
            Event::Loaded {
                request_id: 303,
                ..
            } | Event::LoadFailed {
                request_id: 303,
                ..
            }
        )
    }) {
        Event::Loaded {
            request_id,
            item: None,
        } => loads.complete(request_id),
        event => panic!("不存在记录缺少终态：{event:?}"),
    }
    assert!(!loads.busy());
    shutdown(&engine);
}

#[test]
fn data_editor_retention_save_cleans_unused_history_only() {
    use quick_panel::settings_ui::{Action, edit};
    let (_temp, engine, path) = start(60_000, 16 * 1024 * 1024);
    engine
        .send(Command::Capture(snapshot("fixed keeper")))
        .unwrap();
    let id = search(&engine, "fixed keeper")[0].id.clone();
    engine.send(Command::Pin { id, pinned: true }).unwrap();
    engine
        .send(Command::Capture(snapshot("unused history")))
        .unwrap();
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: None,
            kind: ItemKind::Document,
            title: "favorite keeper".into(),
            target: "C:\\keep.txt".into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
    engine.send(Command::Flush).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let _ = search(&engine, ""); // actor barrier before direct synthetic fixture aging
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE items SET last_used_ms=1", []).unwrap();
    let original = Settings::default();
    let before = count(&path);
    let invalid = edit(
        &original,
        Action::Data,
        &["0", "", "0", "1"].map(str::to_owned),
    );
    assert!(invalid.is_err());
    assert_eq!(
        count(&path),
        before,
        "validation/browsing does not clean SQLite"
    );
    let next = edit(
        &original,
        Action::Data,
        &["0", "", "1", "1"].map(str::to_owned),
    )
    .unwrap();
    engine.send(Command::SaveSettings(next)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert!(search(&engine, "unused history").is_empty());
    assert_eq!(search(&engine, "fixed keeper").len(), 1);
    assert_eq!(search(&engine, "favorite keeper").len(), 1);
    shutdown(&engine);
}

#[test]
fn path_settings_actor_notifications_use_hub_gate_until_return_to_panel() {
    use quick_panel::settings_ui::{self, Group, Navigation, Page};
    let (_temp, engine, _path) = start(60_000, 16 * 1024 * 1024);
    let mut navigation = Navigation::default();
    navigation.open(Page::Hub(Group::Paths));
    let submission = settings_ui::submit_paths(
        &Settings::default(),
        &["1", "1", "", "0"].map(str::to_owned),
    )
    .unwrap();
    for command in submission.commands {
        engine.send(command).unwrap();
    }
    let mut paths = false;
    let mut changed = false;
    let mut saved = false;
    while !(paths && changed && saved) {
        let event = wait(&engine, |_| true);
        match event {
            Event::PathActivityChanged => paths = true,
            Event::Changed => changed = true,
            Event::Saved(_) => saved = true,
            Event::Error { message, .. } => panic!("synthetic save failed: {message}"),
            _ => continue,
        }
        // Same production gate consumed by native Changed/PathActivityChanged/
        // Saved and Search: actual actor notifications may dirty RAM but cannot
        // cause a panel query while the hub/management chain owns navigation.
        assert!(!navigation.allows_panel_query());
    }
    navigation.open(Page::Review);
    navigation.open(Page::Hub(Group::Services));
    navigation.close();
    navigation.close();
    assert!(!navigation.allows_panel_query());
    navigation.close();
    assert!(navigation.allows_panel_query());
    assert!(search(&engine, "").is_empty());
    shutdown(&engine);
}
