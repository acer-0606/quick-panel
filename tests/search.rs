use quick_panel::{model::*, search};
use rusqlite::Connection;

fn item(title: &str, alias: &str, text: &str) -> StoredItem {
    StoredItem {
        summary: ItemSummary {
            id: title.into(),
            kind: ItemKind::Document,
            title: title.into(),
            subtitle: "C:/工作/接口说明.docx".into(),
            alias: alias.into(),
            tags: vec![],
            pinned: false,
            created_at_ms: 1,
            last_used_ms: 1,
            use_count: 0,
        },
        text: text.into(),
        representations: vec![],
        target: Some("C:/工作/接口说明.docx".into()),
        open_with: None,
    }
}

#[test]
fn chinese_short_queries_and_pinyin_retrieve_real_fts_rows() {
    let document = item("周报模板", "weekly", "这是今天的接口说明");
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE searchable USING fts5(content)")
        .unwrap();
    db.execute(
        "INSERT INTO searchable(content) VALUES(?1)",
        [search::index_text(&document)],
    )
    .unwrap();
    for query in ["报", "周报", "接口", "zhoubao", "zbmb"] {
        let expression = search::query_expression(query).unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM searchable WHERE searchable MATCH ?1",
                [expression],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "missing FTS candidate for {query}");
        assert!(
            search::matches(&document, query),
            "missing final match for {query}"
        );
    }
}

#[test]
fn literal_user_query_cannot_inject_fts_operators() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE VIRTUAL TABLE searchable USING fts5(content);
        INSERT INTO searchable(content) VALUES('alpha beta')",
    )
    .unwrap();
    for query in ["alpha\" OR beta", "(alpha):beta", "beta*", "alpha - beta"] {
        let expression = search::query_expression(query).unwrap();
        let _: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM searchable WHERE searchable MATCH ?1",
                [expression],
                |row| row.get(0),
            )
            .unwrap();
    }
}

#[test]
fn exact_alias_beats_pinned_but_less_relevant_item() {
    let exact = item("周报模板", "zb", "周报");
    let mut other = item("zb archive", "", "archive");
    other.summary.pinned = true;
    other.summary.use_count = 100_000;
    assert!(search::rank(&exact.summary, "zb") > search::rank(&other.summary, "zb"));
}

#[test]
fn unrelated_query_is_rejected_and_paths_accept_windows_separators() {
    let document = item("接口说明", "api", "方法参数和返回值");
    assert!(!search::matches(&document, "猫粮"));
    assert!(search::matches(&document, "C:\\工作\\接口"));
    assert!(search::query_expression("   ").is_none());
}

#[test]
fn substring_matches_remain_candidates_after_real_fts_persistence() {
    let document = item("周报模板", "weekly", "clipboard");
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE searchable USING fts5(content)")
        .unwrap();
    db.execute(
        "INSERT INTO searchable(content) VALUES(?1)",
        [search::index_text(&document)],
    )
    .unwrap();
    for query in [
        "baomoban", "bmb", "board", "oar", "b", "mb", "oa", "模", "报模",
    ] {
        assert!(
            search::matches(&document, query),
            "missing pending match for {query}"
        );
        let expression = search::query_expression(query).unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM searchable WHERE searchable MATCH ?1",
                [expression],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "pending match disappeared after saving: {query}");
    }
}

#[test]
fn real_fts_candidates_still_require_literal_substring_filtering() {
    let documents = [
        item("Unrelated", "", "boa oar ard"),
        item("Literal", "", "board* \" OR beta"),
    ];
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE searchable USING fts5(content)")
        .unwrap();
    for (index, document) in documents.iter().enumerate() {
        db.execute(
            "INSERT INTO searchable(rowid,content) VALUES(?1,?2)",
            rusqlite::params![index as i64 + 1, search::index_text(document)],
        )
        .unwrap();
    }
    let expression = search::query_expression("board").unwrap();
    let candidates: i64 = db
        .query_row(
            "SELECT count(*) FROM searchable WHERE searchable MATCH ?1",
            [expression],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        candidates, 2,
        "unordered grams intentionally form a candidate superset"
    );
    for (query, expected) in [
        ("board", vec![2]),
        ("board*", vec![2]),
        ("\" OR ", vec![2]),
        ("board OR beta", vec![]),
    ] {
        let expression = search::query_expression(query).unwrap();
        let mut statement = db
            .prepare("SELECT rowid FROM searchable WHERE searchable MATCH ?1 ORDER BY rowid")
            .unwrap();
        let candidates = statement
            .query_map([expression], |row| row.get::<_, i64>(0))
            .unwrap();
        let matched: Vec<_> = candidates
            .map(Result::unwrap)
            .filter(|rowid| search::matches(&documents[*rowid as usize - 1], query))
            .collect();
        assert_eq!(
            matched, expected,
            "literal query was interpreted as syntax: {query}"
        );
    }
}

#[test]
fn repeated_words_do_not_duplicate_substring_index_storage() {
    let once = item("周报模板", "weekly", "clipboard");
    let repeated = item("周报模板", "weekly", &"clipboard ".repeat(10_000));
    assert_eq!(
        search::index_text(&once).len(),
        search::index_text(&repeated).len()
    );
}

#[test]
fn engine_pending_and_saved_searches_return_the_same_substring_items() {
    use quick_panel::engine::Engine;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    fn wait(engine: &Engine, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match engine.try_recv() {
                Ok(event) if predicate(&event) => return event,
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
    fn search_ids(engine: &Engine, query: &str, request_id: u64) -> Vec<String> {
        engine
            .send(Command::Search(SearchQuery {
                text: query.into(),
                scope: Scope::All,
                request_id,
            }))
            .unwrap();
        match wait(
            engine,
            |event| matches!(event, Event::Results {request_id: id, ..} if *id == request_id),
        ) {
            Event::Results { items, .. } => items.into_iter().map(|item| item.id).collect(),
            _ => unreachable!(),
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("substring.sqlite");
    let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    wait(&engine, |event| matches!(event, Event::Ready(_)));
    engine
        .send(Command::Capture(ClipboardSnapshot {
            text: "周报模板 clipboard".into(),
            representations: vec![Representation {
                format: "CF_UNICODETEXT".into(),
                data: quick_panel::clipboard_formats::encode_unicode_text("周报模板 clipboard"),
            }],
            source_app: "editor.exe".into(),
            captured_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        }))
        .unwrap();
    let queries = ["baomoban", "bmb", "board", "b", "mb", "oa"];
    let pending: Vec<_> = queries
        .iter()
        .enumerate()
        .map(|(index, query)| search_ids(&engine, query, index as u64))
        .collect();
    assert!(
        pending.iter().all(|ids| ids.len() == 1),
        "all substrings must match the pending clipboard item: {pending:?}"
    );
    assert_eq!(
        Connection::open(&path)
            .unwrap()
            .query_row("SELECT count(*) FROM items", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    engine.send(Command::Flush).unwrap();
    wait(&engine, |event| matches!(event, Event::Saved(_)));
    for (index, query) in queries.iter().enumerate() {
        assert_eq!(
            search_ids(&engine, query, 100 + index as u64),
            pending[index],
            "saved search differs for {query}"
        );
    }
    engine.send(Command::Shutdown).unwrap();
    wait(&engine, |event| matches!(event, Event::ExitReady));
}

#[test]
fn legacy_settings_default_smart_mode_and_roundtrip_explicit_modes() {
    assert_eq!(
        serde_json::from_str::<Settings>("{}").unwrap().smart_search,
        SmartSearchMode::Auto
    );
    for smart_search in [
        SmartSearchMode::Auto,
        SmartSearchMode::Claude,
        SmartSearchMode::Semantic,
    ] {
        let settings = Settings {
            smart_search,
            ..Default::default()
        };
        assert_eq!(
            serde_json::from_str::<Settings>(&serde_json::to_string(&settings).unwrap())
                .unwrap()
                .smart_search,
            smart_search
        );
    }
}
