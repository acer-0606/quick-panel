use quick_panel::{engine::Engine, model::*, search};
use rusqlite::Connection;
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn start() -> (tempfile::TempDir, Engine, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("ai-search.sqlite");
    let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&engine, |event| matches!(event, Event::Ready(_)));
    let db = Connection::open(path).unwrap();
    (temp, engine, db)
}

fn wait(engine: &Engine, accept: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match engine.try_recv() {
            Ok(event) if accept(&event) => return event,
            Ok(Event::Error { message, .. }) => panic!("engine error: {message}"),
            Ok(_) => (),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < deadline, "event timeout");
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("engine disconnected: {error}"),
        }
    }
}

fn query(engine: &Engine, plan: AiSearchQuery) -> Vec<ItemSummary> {
    let request_id = plan.request_id;
    engine.send(Command::AiSearch(plan)).unwrap();
    match wait(
        engine,
        |event| matches!(event, Event::Results { request_id: id, .. } if *id == request_id),
    ) {
        Event::Results { items, .. } => items,
        _ => unreachable!(),
    }
}

fn plan(queries: &[&str]) -> AiSearchQuery {
    AiSearchQuery {
        queries: queries.iter().map(|text| (*text).into()).collect(),
        request_id: 41,
        ..AiSearchQuery::default()
    }
}

fn current_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn capture(engine: &Engine, text: &str, at_ms: i64) {
    engine
        .send(Command::Capture(ClipboardSnapshot {
            text: text.into(),
            representations: vec![Representation {
                format: "CF_UNICODETEXT".into(),
                data: quick_panel::clipboard_formats::encode_unicode_text(text),
            }],
            source_app: "editor.exe".into(),
            captured_at_ms: at_ms,
        }))
        .unwrap();
}

fn summary(id: &str, kind: ItemKind, title: &str, created_at_ms: i64) -> ItemSummary {
    ItemSummary {
        id: id.into(),
        kind,
        title: title.into(),
        subtitle: String::new(),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms,
        last_used_ms: created_at_ms,
        use_count: 0,
    }
}

fn insert(db: &Connection, summary: ItemSummary, text: &str, indexed: bool) {
    let kind = match summary.kind {
        ItemKind::Clipboard => 0,
        ItemKind::Folder => 1,
        ItemKind::Document => 2,
    };
    let item = StoredItem {
        summary,
        text: text.into(),
        representations: vec![],
        target: (kind != 0).then(|| "C:/work".into()),
        open_with: None,
    };
    db.execute(
        "INSERT INTO items(id,kind,title,pinned,created_at_ms,last_used_ms,use_count,summary,text,target,open_with) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,NULL)",
        rusqlite::params![
            item.summary.id,
            kind,
            item.summary.title,
            item.summary.pinned,
            item.summary.created_at_ms,
            item.summary.last_used_ms,
            item.summary.use_count as i64,
            serde_json::to_string(&item.summary).unwrap(),
            item.text,
            item.target,
        ],
    )
    .unwrap();
    if indexed {
        db.execute(
            "INSERT INTO item_search(rowid,content,heading,phonetic,phonetic_prefix) SELECT rowid,?2,?3,?4,?5 FROM items WHERE id=?1",
            rusqlite::params![
                item.summary.id,
                search::index_text(&item),
                search::index_heading(&item.summary, false),
                search::index_heading(&item.summary, true),
                search::index_phonetic_prefix(&item.summary),
            ],
        )
        .unwrap();
    }
}

fn data_version(db: &Connection) -> i64 {
    db.query_row("PRAGMA data_version", [], |row| row.get(0))
        .unwrap()
}

fn shutdown(engine: &Engine) {
    engine.send(Command::Shutdown).unwrap();
    wait(engine, |event| matches!(event, Event::ExitReady));
}

#[test]
fn synonym_queries_union_pending_and_saved_items_without_duplicate_ids_or_commits() {
    let (_temp, engine, db) = start();
    let now = current_ms();
    capture(&engine, "连接超时 connection timeout", now);
    capture(&engine, "英文排错\nconnection timeout", now + 1);
    capture(&engine, "无关记录", now + 2);
    let version = data_version(&db);
    let pending = query(&engine, plan(&["连接超时", "connection timeout"]));
    assert_eq!(pending.len(), 2);
    let expected_ids: HashSet<_> = pending.iter().map(|item| item.id.clone()).collect();
    assert_eq!(expected_ids.len(), 2, "one item may match both synonyms");
    for _ in 0..3 {
        let items = query(&engine, plan(&["connection timeout", "连接超时"]));
        assert_eq!(
            items
                .into_iter()
                .map(|item| item.id)
                .collect::<HashSet<_>>(),
            expected_ids
        );
    }
    assert_eq!(data_version(&db), version, "search must not commit");
    let count: i64 = db
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "pending clipboard payloads must stay in memory");
    engine.send(Command::Flush).unwrap();
    wait(&engine, |event| matches!(event, Event::Saved(_)));
    let version = data_version(&db);
    let saved = query(&engine, plan(&["连接超时", "connection timeout"]));
    assert_eq!(
        saved
            .into_iter()
            .map(|item| item.id)
            .collect::<HashSet<_>>(),
        expected_ids
    );
    assert_eq!(data_version(&db), version);
    shutdown(&engine);
}

#[test]
fn scope_and_half_open_time_filter_include_pending_and_use_first_saved_time() {
    let (_temp, engine, db) = start();
    insert(
        &db,
        summary("old", ItemKind::Clipboard, "needle old", 100),
        "needle old",
        true,
    );
    insert(
        &db,
        summary("start", ItemKind::Clipboard, "needle start", 200),
        "needle start",
        true,
    );
    insert(
        &db,
        summary("end", ItemKind::Clipboard, "needle end", 300),
        "needle end",
        true,
    );
    insert(
        &db,
        summary("folder", ItemKind::Folder, "needle folder", 250),
        "",
        true,
    );
    capture(&engine, "needle pending", 299);
    engine.send(Command::Used("old".into())).unwrap();
    let items = query(
        &engine,
        AiSearchQuery {
            scope: Scope::Clipboard,
            from_ms: Some(200),
            until_ms: Some(300),
            ..plan(&["needle"])
        },
    );
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|item| item.id == "start"));
    assert!(items.iter().any(|item| item.title == "needle pending"));
    assert!(items.iter().all(|item| item.kind == ItemKind::Clipboard));
    assert!(
        items
            .iter()
            .all(|item| (200..300).contains(&item.created_at_ms))
    );
    shutdown(&engine);
}

#[test]
fn time_filter_precedes_fts_candidate_limits_for_title_phonetic_and_body_streams() {
    for (label, needle, title, body) in [
        ("heading", "needle", "needle archive", ""),
        ("phonetic-prefix", "zb", "周报模板", ""),
        ("phonetic-infix", "baomoban", "周报模板", ""),
        ("body", "needle", "Archive", "needle payload"),
    ] {
        let (_temp, engine, db) = start();
        db.execute_batch("BEGIN").unwrap();
        insert(
            &db,
            summary("wanted", ItemKind::Clipboard, title, 100),
            body,
            true,
        );
        for index in 0..600 {
            insert(
                &db,
                summary(&format!("new:{index}"), ItemKind::Clipboard, title, 200),
                body,
                true,
            );
        }
        db.execute_batch("COMMIT").unwrap();
        let items = query(
            &engine,
            AiSearchQuery {
                from_ms: Some(100),
                until_ms: Some(101),
                ..plan(&[needle])
            },
        );
        assert_eq!(items.len(), 1, "{label} candidate quota hid filtered row");
        assert_eq!(items[0].id, "wanted", "{label}");
        shutdown(&engine);
    }
}

#[test]
fn time_filter_precedes_alias_exact_and_prefix_candidate_limits() {
    for (label, needle, alias) in [("exact", "needle", "needle"), ("prefix", "need", "needle")] {
        let (_temp, engine, db) = start();
        db.execute_batch("BEGIN").unwrap();
        // No FTS rows: isolate the indexed alias candidate streams.
        for index in 0..600 {
            let mut item = summary(&format!("new:{index}"), ItemKind::Clipboard, "Archive", 200);
            item.alias = alias.into();
            insert(&db, item, "", false);
        }
        let mut wanted = summary("wanted", ItemKind::Clipboard, "Archive", 100);
        wanted.alias = alias.into();
        insert(&db, wanted, "", false);
        db.execute_batch("COMMIT").unwrap();
        let items = query(
            &engine,
            AiSearchQuery {
                from_ms: Some(100),
                until_ms: Some(101),
                ..plan(&[needle])
            },
        );
        assert_eq!(items.len(), 1, "{label} alias quota hid filtered row");
        assert_eq!(items[0].id, "wanted");
        shutdown(&engine);
    }
}

#[test]
fn time_filter_precedes_each_auxiliary_pool_and_empty_browse_limit() {
    for (label, kind, pinned, used, needle) in [
        ("pinned", ItemKind::Clipboard, true, 0, "needle"),
        ("favorite", ItemKind::Folder, false, 0, "needle"),
        ("used", ItemKind::Clipboard, false, 1, "needle"),
        ("recent", ItemKind::Clipboard, false, 0, "needle"),
        ("browse", ItemKind::Clipboard, false, 0, ""),
    ] {
        let (_temp, engine, db) = start();
        db.execute_batch("BEGIN").unwrap();
        for index in 0..601 {
            let wanted = index == 0;
            let id = if wanted {
                "wanted".into()
            } else {
                format!("new:{index}")
            };
            let mut item = summary(&id, kind, "Archive", if wanted { 100 } else { 200 });
            item.tags = vec!["needle".into()];
            item.pinned = pinned;
            item.use_count = used;
            insert(&db, item, "", false);
        }
        db.execute_batch("COMMIT").unwrap();
        let queries = if needle.is_empty() {
            vec![]
        } else {
            vec![needle]
        };
        let items = query(
            &engine,
            AiSearchQuery {
                from_ms: Some(100),
                until_ms: Some(101),
                ..plan(&queries)
            },
        );
        assert_eq!(items.len(), 1, "{label} candidate quota hid filtered row");
        assert_eq!(items[0].id, "wanted");
        shutdown(&engine);
    }
}

#[test]
fn literal_ai_terms_cannot_inject_fts_or_accept_gram_collisions() {
    let (_temp, engine, db) = start();
    insert(
        &db,
        summary("literal", ItemKind::Clipboard, "Actual", current_ms()),
        "board* \" OR beta",
        true,
    );
    insert(
        &db,
        summary("collision", ItemKind::Clipboard, "Unrelated", current_ms()),
        "boa oar ard beta",
        true,
    );
    let items = query(&engine, plan(&["board", "\" OR beta"]));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "literal");
    assert!(query(&engine, plan(&["board OR beta"])).is_empty());
    shutdown(&engine);
}

#[test]
fn pending_deletion_usage_and_immediate_pin_are_reflected_without_search_writes() {
    let (_temp, engine, db) = start();
    let now = current_ms();
    for id in ["deleted", "used", "other"] {
        insert(
            &db,
            summary(id, ItemKind::Clipboard, id, now),
            "needle body",
            true,
        );
    }
    engine.send(Command::Delete("deleted".into())).unwrap();
    engine.send(Command::Used("used".into())).unwrap();
    let version = data_version(&db);
    let items = query(&engine, plan(&["needle", "body"]));
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "used");
    assert_eq!(items[0].use_count, 1);
    assert_eq!(data_version(&db), version);
    engine
        .send(Command::Pin {
            id: "other".into(),
            pinned: true,
        })
        .unwrap();
    wait(&engine, |event| matches!(event, Event::Saved(_)));
    let version = data_version(&db);
    let items = query(&engine, plan(&["needle", "body"]));
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "other");
    assert!(items[0].pinned);
    assert_eq!(data_version(&db), version);
    shutdown(&engine);
}

#[test]
fn empty_ai_queries_browse_only_an_explicit_scope_or_time_range() {
    let (_temp, engine, db) = start();
    insert(
        &db,
        summary("folder", ItemKind::Folder, "Work", 100),
        "",
        true,
    );
    insert(
        &db,
        summary("clipboard", ItemKind::Clipboard, "Copy", 200),
        "Copy",
        true,
    );
    let folders = query(
        &engine,
        AiSearchQuery {
            scope: Scope::Folder,
            ..plan(&[])
        },
    );
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].id, "folder");
    let clips = query(
        &engine,
        AiSearchQuery {
            from_ms: Some(200),
            ..plan(&[])
        },
    );
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].id, "clipboard");
    shutdown(&engine);
}

#[test]
fn invalid_untrusted_ai_query_plans_return_errors_and_do_not_commit() {
    let (_temp, engine, db) = start();
    let too_long = "字".repeat(65);
    let cases = [
        plan(&["one", "two", "three", "four", "five", "six", "seven"]),
        plan(&[&too_long]),
        AiSearchQuery {
            from_ms: Some(200),
            until_ms: Some(200),
            ..plan(&["needle"])
        },
        AiSearchQuery {
            from_ms: Some(300),
            until_ms: Some(200),
            ..plan(&["needle"])
        },
        plan(&[]),
        plan(&["   "]),
    ];
    let version = data_version(&db);
    for invalid in cases {
        engine.send(Command::AiSearch(invalid)).unwrap();
        let event = wait(&engine, |event| {
            matches!(event, Event::Error { .. } | Event::Results { .. })
        });
        assert!(matches!(event, Event::Error { exiting: false, .. }));
    }
    assert_eq!(data_version(&db), version);
    // Rejection must not disable subsequent valid local work.
    assert!(query(&engine, plan(&["needle"])).is_empty());
    shutdown(&engine);
}
