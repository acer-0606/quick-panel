use quick_panel::{engine::Engine, model::*, semantic::*};
use rusqlite::Connection;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn wait(engine: &Engine, accept: impl Fn(&Event) -> bool) -> Event {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match engine.try_recv() {
            Ok(e) if accept(&e) => return e,
            Ok(Event::Error { message, .. }) => panic!("{message}"),
            Ok(_) => (),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("{e}"),
        }
    }
}
fn start(path: &std::path::Path, interval: u64) -> Engine {
    let mut options = EngineOptions::new(path.into());
    options.save_interval_ms = interval;
    let engine = Engine::start(options, Arc::new(|| {})).unwrap();
    wait(&engine, |e| matches!(e, Event::Ready(_)));
    engine
}
fn configure(engine: &Engine, scope: IndexScope) -> Settings {
    let mut settings = Settings::default();
    settings.embedding.enabled = true;
    settings.embedding.model = "fixture".into();
    settings.embedding.index_scope = scope;
    engine
        .send(Command::SaveSettings(settings.clone()))
        .unwrap();
    wait(engine, |e| matches!(e, Event::Saved(_)));
    settings
}
fn capture(engine: &Engine, text: &str) {
    engine
        .send(Command::Capture(ClipboardSnapshot {
            text: text.into(),
            representations: vec![],
            source_app: "fixture".into(),
            captured_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        }))
        .unwrap();
}
fn batch(engine: &Engine, mut cursor: IndexCursor) -> IndexBatch {
    loop {
        engine
            .send(Command::PrepareSemanticIndex {
                request_id: 10,
                cursor,
            })
            .unwrap();
        match wait(engine, |e| matches!(e, Event::SemanticBatch(_))) {
            Event::SemanticBatch(b) if !b.candidates.is_empty() || b.done => return b,
            Event::SemanticBatch(b) => cursor = b.cursor,
            _ => unreachable!(),
        }
    }
}
fn store(engine: &Engine, b: &IndexBatch, vectors: Vec<Vec<f32>>) -> usize {
    let items = b
        .candidates
        .iter()
        .zip(vectors)
        .map(|(c, vector)| IndexedEmbedding {
            id: c.id.clone(),
            text_hash: c.text_hash.clone(),
            vector,
        })
        .collect();
    engine
        .send(Command::StoreEmbeddings {
            request_id: 11,
            profile: b.profile.clone(),
            items,
        })
        .unwrap();
    match wait(engine, |e| matches!(e, Event::EmbeddingsStored { .. })) {
        Event::EmbeddingsStored { accepted, .. } => accepted,
        _ => unreachable!(),
    }
}
fn query(engine: &Engine, profile: &str, scope: Scope, vector: Vec<f32>) -> Vec<ItemSummary> {
    engine
        .send(Command::SemanticSearch(SemanticSearchQuery {
            text: "退款说明".into(),
            scope,
            request_id: 20,
            profile: profile.into(),
            vector,
        }))
        .unwrap();
    match wait(engine, |e| {
        matches!(e, Event::Results { request_id: 20, .. })
    }) {
        Event::Results { items, .. } => items,
        _ => unreachable!(),
    }
}
fn flush(engine: &Engine) {
    engine.send(Command::Flush).unwrap();
    wait(engine, |e| matches!(e, Event::Saved(_)));
}
fn shutdown(engine: &Engine) {
    engine.send(Command::Shutdown).unwrap();
    wait(engine, |e| matches!(e, Event::ExitReady));
}
fn vec_count(db: &Connection) -> i64 {
    if db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='semantic_vectors'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        == 0
    {
        return 0;
    }
    db.query_row("SELECT count(*) FROM semantic_vectors", [], |r| r.get(0))
        .unwrap()
}
// Losing actor vectors or flushing on query breaks this real memory/disk boundary.
#[test]
fn accepted_vector_is_visible_before_flush_and_survives_shutdown() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    capture(&engine, "订单取消后的款项处理");
    let b = batch(&engine, IndexCursor::default());
    assert_eq!(b.candidates.len(), 1);
    let id = b.candidates[0].id.clone();
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]]), 1);
    let db = Connection::open(&path).unwrap();
    let version: i64 = db
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        query(&engine, &b.profile, Scope::Clipboard, vec![1., 0.])[0].id,
        id
    );
    assert_eq!(vec_count(&db), 0);
    assert_eq!(
        db.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        version
    );
    shutdown(&engine);
    drop(engine);
    assert_eq!(vec_count(&db), 1);
    let engine = start(&path, 60_000);
    assert_eq!(
        query(&engine, &b.profile, Scope::Clipboard, vec![1., 0.])[0].id,
        id
    );
    shutdown(&engine);
}
// f32 square overflow/underflow in native cosine must not change independent rankings.
#[test]
fn persisted_knn_normalizes_extreme_finite_vectors() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    capture(&engine, "甲");
    capture(&engine, "乙");
    let b = batch(&engine, IndexCursor::default());
    assert_eq!(
        store(
            &engine,
            &b,
            vec![vec![f32::MAX, 0.], vec![0., f32::from_bits(1)]]
        ),
        2
    );
    let id = b.candidates[0].id.clone();
    flush(&engine);
    assert_eq!(
        query(&engine, &b.profile, Scope::Clipboard, vec![f32::MAX, 0.])[0].id,
        id
    );
    assert_eq!(
        query(
            &engine,
            &b.profile,
            Scope::Clipboard,
            vec![0., f32::from_bits(1)]
        )[0]
        .id,
        b.candidates[1].id
    );
    let db = Connection::open(&path).unwrap();
    let version: String = db
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .unwrap();
    eprintln!("native vec_version={version}");
    assert_eq!(version, "v0.1.9");
    shutdown(&engine);
}
// Metadata overlay must retain real text; obsolete/deleted batches must not revive it.
#[test]
fn candidate_metadata_overlay_and_stale_deleted_admission() {
    let temp = tempfile::tempdir().unwrap();
    let engine = start(&temp.path().join("test.db"), 60_000);
    configure(&engine, IndexScope::AllText);
    capture(&engine, "正文隐藏信息");
    flush(&engine);
    let b = batch(&engine, IndexCursor::default());
    let id = b.candidates[0].id.clone();
    engine.send(Command::Used(id.clone())).unwrap();
    let after = batch(&engine, IndexCursor::default());
    assert_eq!(after.candidates, b.candidates);
    engine.send(Command::Delete(id)).unwrap();
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]]), 0);
    assert!(query(&engine, &b.profile, Scope::All, vec![1., 0.]).is_empty());
    shutdown(&engine);
}
// Missing atomic validation would expose first item from a malformed second item.
#[test]
fn malformed_batch_changes_no_vectors() {
    let temp = tempfile::tempdir().unwrap();
    let engine = start(&temp.path().join("test.db"), 60_000);
    configure(&engine, IndexScope::AllText);
    capture(&engine, "甲");
    capture(&engine, "乙");
    let b = batch(&engine, IndexCursor::default());
    engine
        .send(Command::StoreEmbeddings {
            request_id: 11,
            profile: b.profile.clone(),
            items: b
                .candidates
                .iter()
                .enumerate()
                .map(|(i, c)| IndexedEmbedding {
                    id: c.id.clone(),
                    text_hash: c.text_hash.clone(),
                    vector: if i == 0 {
                        vec![1., 0.]
                    } else {
                        vec![f32::NAN, 0.]
                    },
                })
                .collect(),
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert!(query(&engine, &b.profile, Scope::All, vec![1., 0.]).is_empty());
    shutdown(&engine);
}

fn insert(db: &Connection, id: &str, kind: ItemKind, pinned: bool, text: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let summary = ItemSummary {
        id: id.into(),
        kind,
        title: format!("标题{id}"),
        subtitle: String::new(),
        alias: String::new(),
        tags: vec![],
        pinned,
        created_at_ms: now,
        last_used_ms: now,
        use_count: 0,
    };
    let kind = match kind {
        ItemKind::Clipboard => 0,
        ItemKind::Folder => 1,
        ItemKind::Document => 2,
    };
    db.execute(
        "INSERT INTO items VALUES(?1,?2,?3,?4,?5,?5,0,?6,?7,?8,NULL)",
        rusqlite::params![
            id,
            kind,
            summary.title,
            pinned,
            now,
            serde_json::to_string(&summary).unwrap(),
            text,
            (kind != 0).then_some("C:/fixture")
        ],
    )
    .unwrap();
}
fn one_page(engine: &Engine, cursor: IndexCursor) -> IndexBatch {
    engine
        .send(Command::PrepareSemanticIndex {
            request_id: 10,
            cursor,
        })
        .unwrap();
    match wait(engine, |e| matches!(e, Event::SemanticBatch(_))) {
        Event::SemanticBatch(b) => b,
        _ => unreachable!(),
    }
}
fn index_all(engine: &Engine, vector: impl Fn(&EmbeddingCandidate) -> Vec<f32>) -> String {
    let mut cursor = IndexCursor::default();
    loop {
        let b = one_page(engine, cursor);
        if !b.candidates.is_empty() {
            assert_eq!(
                store(engine, &b, b.candidates.iter().map(&vector).collect()),
                b.candidates.len()
            )
        };
        if b.done {
            return b.profile;
        }
        cursor = b.cursor;
    }
}
// Post-top-k filtering would lose wanted behind 60 closer out-of-scope rows.
#[test]
fn scope_eligibility_and_pending_deletions_precede_native_knn_limit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    let mut settings = configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    for i in 0..60 {
        insert(
            &db,
            &format!("clip:{i}"),
            ItemKind::Clipboard,
            false,
            "原文",
        );
    }
    insert(&db, "wanted", ItemKind::Folder, false, "");
    insert(&db, "pinned", ItemKind::Clipboard, true, "保留");
    let profile = index_all(&engine, |c| {
        if c.id.starts_with("clip:") {
            vec![1., 0.]
        } else {
            vec![0., 1.]
        }
    });
    flush(&engine);
    assert_eq!(
        query(&engine, &profile, Scope::Folder, vec![1., 0.])[0].id,
        "wanted"
    );
    settings.embedding.index_scope = IndexScope::PinnedAndFavorites;
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let items = query(&engine, &profile, Scope::All, vec![1., 0.]);
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|s| s.id == "pinned"));
    configure(&engine, IndexScope::AllText);
    for i in 0..60 {
        engine.send(Command::Delete(format!("clip:{i}"))).unwrap();
    }
    let items = query(&engine, &profile, Scope::All, vec![1., 0.]);
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|s| s.id == "wanted"));
    flush(&engine);
    assert_eq!(vec_count(&db), 2);
    shutdown(&engine);
}
// A changed favorite must invalidate disk coordinates and old in-flight hashes.
#[test]
fn text_changes_and_changed_scope_cannot_admit_old_batches() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    let settings = configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    insert(&db, "favorite", ItemKind::Document, false, "");
    let b = batch(&engine, IndexCursor::default());
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]]), 1);
    flush(&engine);
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: Some("favorite".into()),
            kind: ItemKind::Document,
            title: "新正文".into(),
            target: "C:/new".into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(vec_count(&db), 0);
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]]), 0);
    capture(&engine, "未固定");
    let b = batch(&engine, IndexCursor::default());
    let mut narrowed = settings;
    narrowed.embedding.index_scope = IndexScope::PinnedAndFavorites;
    engine.send(Command::SaveSettings(narrowed)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(
        store(
            &engine,
            &b,
            b.candidates.iter().map(|_| vec![1., 0.]).collect()
        ),
        b.candidates.iter().filter(|c| c.id == "favorite").count()
    );
    shutdown(&engine);
}
// Retention must remove original and derived rows atomically, never resurrecting a cleaned source.
#[test]
fn retention_removes_vectors_and_rejects_originally_cleaned_rows() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    let mut settings = configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    insert(&db, "old", ItemKind::Clipboard, false, "历史");
    insert(&db, "keep", ItemKind::Clipboard, true, "固定");
    let b = batch(&engine, IndexCursor::default());
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]; 2]), 2);
    flush(&engine);
    assert_eq!(vec_count(&db), 2);
    settings.max_history = 0;
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(vec_count(&db), 1);
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]; 2]), 1);
    flush(&engine);
    assert_eq!(vec_count(&db), 1);
    assert_eq!(
        query(&engine, &b.profile, Scope::All, vec![1., 0.])[0].id,
        "keep"
    );
    shutdown(&engine);
}
// Derived state must stay lazy, and profile/dimension errors must have no writes.
#[test]
fn lazy_schema_and_profile_dimension_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    let mut settings = configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    let profile = profile_id(&settings.embedding).unwrap();
    let version: i64 = db
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    assert!(query(&engine, &profile, Scope::All, vec![1., 0.]).is_empty());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name LIKE 'semantic_%'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        version
    );
    capture(&engine, "数据");
    let b = batch(&engine, IndexCursor::default());
    assert_eq!(store(&engine, &b, vec![vec![1., 0.]]), 1);
    flush(&engine);
    engine
        .send(Command::StoreEmbeddings {
            request_id: 12,
            profile: profile.clone(),
            items: vec![IndexedEmbedding {
                id: b.candidates[0].id.clone(),
                text_hash: b.candidates[0].text_hash.clone(),
                vector: vec![1., 0., 0.],
            }],
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert!(query(&engine, &profile, Scope::All, vec![1., 0., 0.]).is_empty());
    settings.embedding.model = "different".into();
    engine
        .send(Command::SaveSettings(settings.clone()))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    engine
        .send(Command::SemanticSearch(SemanticSearchQuery {
            text: "旧请求".into(),
            profile: profile.clone(),
            vector: vec![1., 0.],
            ..SemanticSearchQuery::default()
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    engine
        .send(Command::StoreEmbeddings {
            request_id: 12,
            profile,
            items: vec![IndexedEmbedding {
                id: b.candidates[0].id.clone(),
                text_hash: b.candidates[0].text_hash.clone(),
                vector: vec![1., 0.],
            }],
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    let new_profile = profile_id(&settings.embedding).unwrap();
    assert!(query(&engine, &new_profile, Scope::All, vec![1., 0., 0.]).is_empty());
    let b = batch(&engine, IndexCursor::default());
    assert_eq!(store(&engine, &b, vec![vec![1., 0., 0.]]), 1);
    flush(&engine);
    assert_eq!(vec_count(&db), 1);
    assert_eq!(
        query(&engine, &new_profile, Scope::All, vec![1., 0., 0.]).len(),
        1
    );
    shutdown(&engine);
}
// Metadata writes may update pin/use stats but must leave native coordinates untouched.
#[test]
fn metadata_usage_and_duplicate_capture_do_not_rewrite_vectors() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    capture(&engine, "正文");
    let b = batch(&engine, IndexCursor::default());
    store(&engine, &b, vec![vec![1., 0.]]);
    flush(&engine);
    let db = Connection::open(&path).unwrap();
    let row: (i64, Vec<u8>) = db
        .query_row("SELECT rowid,embedding FROM semantic_vectors", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    engine
        .send(Command::Used(b.candidates[0].id.clone()))
        .unwrap();
    capture(&engine, "正文");
    assert!(batch(&engine, IndexCursor::default()).candidates.is_empty());
    flush(&engine);
    engine
        .send(Command::Pin {
            id: b.candidates[0].id.clone(),
            pinned: true,
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let after: (i64, Vec<u8>) = db
        .query_row("SELECT rowid,embedding FROM semantic_vectors", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(row, after);
    assert_eq!(
        query(&engine, &b.profile, Scope::All, vec![1., 0.])[0].use_count,
        1
    );
    shutdown(&engine);
}
// Candidate quotas must count scanned sources, and cursors must skip completed hashes.
#[test]
fn candidate_pages_are_bounded_skip_unchanged_and_preserve_real_text() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::PinnedAndFavorites);
    let db = Connection::open(&path).unwrap();
    for i in 0..130 {
        insert(
            &db,
            &format!("unselected:{i}"),
            ItemKind::Clipboard,
            false,
            "隐藏文本",
        );
    }
    for i in 0..18 {
        insert(
            &db,
            &format!("selected:{i}"),
            ItemKind::Clipboard,
            true,
            "真正正文",
        );
    }
    let first = one_page(&engine, IndexCursor::default());
    assert!(first.candidates.is_empty());
    assert_eq!(first.cursor.after_rowid, 128);
    assert!(!first.done);
    let second = one_page(&engine, first.cursor);
    assert_eq!(second.candidates.len(), 16);
    assert!(
        second
            .candidates
            .iter()
            .all(|c| c.text.contains("真正正文"))
    );
    store(&engine, &second, vec![vec![1., 0.]; 16]);
    let third = one_page(&engine, second.cursor);
    assert_eq!(third.candidates.len(), 2);
    store(&engine, &third, vec![vec![1., 0.]; 2]);
    assert!(one_page(&engine, third.cursor).done);
    assert!(batch(&engine, IndexCursor::default()).candidates.is_empty());
    shutdown(&engine);
}
// A failed commit/shutdown must keep accepted vectors and retry from the same state.
#[test]
fn failed_shutdown_retains_vectors_and_rejects_new_vectors_until_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    capture(&engine, "待提交");
    let b = batch(&engine, IndexCursor::default());
    store(&engine, &b, vec![vec![1., 0.]]);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine.send(Command::Shutdown).unwrap();
    assert!(matches!(
        wait(&engine, |e| matches!(e, Event::Error { .. })),
        Event::Error { exiting: true, .. }
    ));
    assert!(
        engine
            .send(Command::StoreEmbeddings {
                request_id: 11,
                profile: b.profile.clone(),
                items: vec![]
            })
            .is_err()
    );
    db.execute_batch("ROLLBACK").unwrap();
    engine.send(Command::CancelShutdown).unwrap();
    flush(&engine);
    assert_eq!(vec_count(&db), 1);
    assert_eq!(
        query(&engine, &b.profile, Scope::All, vec![1., 0.]).len(),
        1
    );
    shutdown(&engine);
}
// A vector-only dirty batch must schedule persistence even without source changes.
#[test]
fn vector_only_dirty_state_uses_deadline_and_background_retry() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 150);
    configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    insert(&db, "saved", ItemKind::Clipboard, false, "已保存原文");
    let b = batch(&engine, IndexCursor::default());
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    store(&engine, &b, vec![vec![1., 0.]]);
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert_eq!(vec_count(&db), 0);
    assert_eq!(
        query(&engine, &b.profile, Scope::All, vec![1., 0.])[0].id,
        "saved"
    );
    db.execute_batch("ROLLBACK").unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert_eq!(vec_count(&db), 1);
    shutdown(&engine);
}

// Path matches must lead hybrid results even when semantic distance prefers another ID.
#[test]
fn explicit_path_match_leads_hybrid_results_and_real_ids_are_deduplicated() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            id: Some("path".into()),
            kind: ItemKind::Document,
            title: "归档".into(),
            target: "C:/退款说明.txt".into(),
            alias: String::new(),
            tags: vec![],
            open_with: None,
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    capture(&engine, "售后款项处理");
    let profile = index_all(&engine, |c| {
        if c.id == "path" {
            vec![0., 1.]
        } else {
            vec![1., 0.]
        }
    });
    let items = query(&engine, &profile, Scope::All, vec![1., 0.]);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "path");
    flush(&engine);
    assert_eq!(
        query(&engine, &profile, Scope::All, vec![1., 0.])[0].id,
        "path"
    );
    shutdown(&engine);
}
// Non-input Content mutation must preserve disk vectors while a save is blocked.
#[test]
fn unchanged_hash_content_overlay_keeps_vectors_visible_without_rewriting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    let favorite = FavoriteInput {
        id: Some("favorite".into()),
        kind: ItemKind::Document,
        title: "归档".into(),
        target: "C:/work".into(),
        alias: String::new(),
        tags: vec![],
        open_with: None,
    };
    engine
        .send(Command::SaveFavorite(favorite.clone()))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let b = batch(&engine, IndexCursor::default());
    store(&engine, &b, vec![vec![1., 0.]]);
    flush(&engine);
    let db = Connection::open(&path).unwrap();
    let row: (i64, Vec<u8>) = db
        .query_row("SELECT rowid,embedding FROM semantic_vectors", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine
        .send(Command::SaveFavorite(FavoriteInput {
            open_with: Some("C:/editor.exe".into()),
            ..favorite
        }))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert_eq!(
        query(&engine, &b.profile, Scope::Document, vec![1., 0.])[0].id,
        "favorite"
    );
    assert!(batch(&engine, IndexCursor::default()).candidates.is_empty());
    db.execute_batch("ROLLBACK").unwrap();
    flush(&engine);
    assert_eq!(
        db.query_row("SELECT rowid,embedding FROM semantic_vectors", [], |r| Ok(
            (r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)
        ))
        .unwrap(),
        row
    );
    shutdown(&engine);
}
// Cleaning controls before the Unicode bound must agree across disk/memory/admission.
#[test]
fn control_cleaning_and_unicode_truncation_match_at_storage_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    let text = format!("开头\n{}{}", "\u{1}".repeat(5000), "正文".repeat(5000));
    capture(&engine, &text);
    let memory = batch(&engine, IndexCursor::default());
    assert_eq!(memory.candidates[0].text.chars().count(), 4096);
    assert!(memory.candidates[0].text.contains("正文"));
    flush(&engine);
    let disk = batch(&engine, IndexCursor::default());
    assert_eq!(disk.candidates, memory.candidates);
    assert_eq!(store(&engine, &memory, vec![vec![1., 0.]]), 1);
    flush(&engine);
    assert!(batch(&engine, IndexCursor::default()).candidates.is_empty());
    shutdown(&engine);
}

fn fill_near_vector_budget(engine: &Engine, db: &Connection, folders: bool) -> IndexBatch {
    for i in 0..256 {
        insert(
            db,
            &format!("budget:{i}"),
            if folders && i >= 240 {
                ItemKind::Folder
            } else {
                ItemKind::Clipboard
            },
            false,
            "正文",
        );
    }
    let mut cursor = IndexCursor::default();
    for _ in 0..15 {
        let b = one_page(engine, cursor);
        assert_eq!(b.candidates.len(), 16);
        assert_eq!(store(engine, &b, vec![vec![1.; 4096]; 16]), 16);
        cursor = b.cursor;
    }
    one_page(engine, cursor)
}
// Pressure commits cannot partially admit a malformed batch or lose the old cache on failure.
#[test]
fn vector_budget_pressure_preserves_atomic_admission_and_failed_save_cache() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    let b = fill_near_vector_budget(&engine, &db, true);
    assert_eq!(vec_count(&db), 0);
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut items: Vec<_> = b
        .candidates
        .iter()
        .map(|c| IndexedEmbedding {
            id: c.id.clone(),
            text_hash: c.text_hash.clone(),
            vector: vec![1.; 4096],
        })
        .collect();
    items[15].vector[3] = f32::NAN;
    engine
        .send(Command::StoreEmbeddings {
            request_id: 11,
            profile: b.profile.clone(),
            items,
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert_eq!(vec_count(&db), 0);
    assert!(query(&engine, &b.profile, Scope::Folder, vec![1.; 4096]).is_empty());
    engine
        .send(Command::StoreEmbeddings {
            request_id: 11,
            profile: b.profile.clone(),
            items: b
                .candidates
                .iter()
                .map(|c| IndexedEmbedding {
                    id: c.id.clone(),
                    text_hash: c.text_hash.clone(),
                    vector: vec![1.; 4096],
                })
                .collect(),
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    assert_eq!(vec_count(&db), 0);
    assert_eq!(
        query(&engine, &b.profile, Scope::Clipboard, vec![1.; 4096]).len(),
        50
    );
    assert!(query(&engine, &b.profile, Scope::Folder, vec![1.; 4096]).is_empty());
    db.execute_batch("ROLLBACK").unwrap();
    flush(&engine);
    assert_eq!(vec_count(&db), 240);
    assert_eq!(store(&engine, &b, vec![vec![1.; 4096]; 16]), 16);
    assert_eq!(
        query(&engine, &b.profile, Scope::Folder, vec![1.; 4096]).len(),
        16
    );
    assert_eq!(vec_count(&db), 240);
    shutdown(&engine);
    assert_eq!(vec_count(&db), 256);
}
// Pressure flush runs retention, so batches must validate again before counting accepted rows.
#[test]
fn pressure_flush_revalidates_sources_removed_by_retention() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    let mut settings = configure(&engine, IndexScope::AllText);
    settings.max_history = 0;
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let db = Connection::open(&path).unwrap();
    let b = fill_near_vector_budget(&engine, &db, false);
    assert_eq!(store(&engine, &b, vec![vec![1.; 4096]; 16]), 0);
    assert!(query(&engine, &b.profile, Scope::All, vec![1.; 4096]).is_empty());
    assert_eq!(vec_count(&db), 0);
    shutdown(&engine);
}

// Pin changes still in RAM must filter native candidates before k, not after it.
#[test]
fn pending_pin_overlay_is_eligible_before_knn_limit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    let mut settings = configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    for i in 0..60 {
        insert(
            &db,
            &format!("unselected:{i}"),
            ItemKind::Clipboard,
            false,
            "其他",
        );
    }
    insert(&db, "wanted", ItemKind::Clipboard, false, "需要固定");
    let profile = index_all(&engine, |c| {
        if c.id == "wanted" {
            vec![0., 1.]
        } else {
            vec![1., 0.]
        }
    });
    flush(&engine);
    settings.embedding.index_scope = IndexScope::PinnedAndFavorites;
    engine.send(Command::SaveSettings(settings)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    engine
        .send(Command::Pin {
            id: "wanted".into(),
            pinned: true,
        })
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Error { .. }));
    let items = query(&engine, &profile, Scope::Clipboard, vec![1., 0.]);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "wanted");
    assert!(items[0].pinned);
    db.execute_batch("ROLLBACK").unwrap();
    shutdown(&engine);
}
// Repeated vector admissions must not push back the first dirty vector deadline.
#[test]
fn continuous_vector_batches_do_not_postpone_first_dirty_deadline() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 200);
    configure(&engine, IndexScope::AllText);
    let db = Connection::open(&path).unwrap();
    insert(&db, "saved", ItemKind::Clipboard, false, "正文");
    let b = batch(&engine, IndexCursor::default());
    let first = Instant::now();
    store(&engine, &b, vec![vec![1., 0.]]);
    while vec_count(&db) == 0 && first.elapsed() < Duration::from_millis(600) {
        store(&engine, &b, vec![vec![1., 0.]]);
    }
    assert_eq!(vec_count(&db), 1);
    assert!(
        first.elapsed() < Duration::from_millis(450),
        "later vectors postponed the first dirty deadline"
    );
    shutdown(&engine);
}
// Tail edits past the actual semantic input bound preserve already indexed coordinates.
#[test]
fn content_tail_changes_beyond_input_bound_keep_existing_vector() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("test.db");
    let engine = start(&path, 60_000);
    configure(&engine, IndexScope::AllText);
    let mut favorite = FavoriteInput {
        id: Some("favorite".into()),
        kind: ItemKind::Document,
        title: "归档".into(),
        target: format!("C:/{}old", "界".repeat(5000)),
        alias: String::new(),
        tags: vec![],
        open_with: None,
    };
    engine
        .send(Command::SaveFavorite(favorite.clone()))
        .unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    let b = batch(&engine, IndexCursor::default());
    store(&engine, &b, vec![vec![1., 0.]]);
    flush(&engine);
    let db = Connection::open(&path).unwrap();
    let row: i64 = db
        .query_row("SELECT rowid FROM semantic_vectors", [], |r| r.get(0))
        .unwrap();
    favorite.target = format!("C:/{}new", "界".repeat(5000));
    engine.send(Command::SaveFavorite(favorite)).unwrap();
    wait(&engine, |e| matches!(e, Event::Saved(_)));
    assert!(batch(&engine, IndexCursor::default()).candidates.is_empty());
    assert_eq!(
        db.query_row("SELECT rowid FROM semantic_vectors", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        row
    );
    assert_eq!(
        query(&engine, &b.profile, Scope::Document, vec![1., 0.])[0].id,
        "favorite"
    );
    shutdown(&engine);
}
