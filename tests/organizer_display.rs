use quick_panel::{
    model::{ItemKind, ItemSummary},
    organizer::Source,
    organizer_display::{MetadataUpdate, Progress, ProgressCache, Stage},
    palette::{self, Entry},
};

fn saved(id: &str) -> ItemSummary {
    ItemSummary {
        id: id.into(),
        kind: ItemKind::Folder,
        title: "logs".into(),
        subtitle: format!("C:\\Projects\\{id}\\logs"),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms: 1,
        last_used_ms: 1,
        use_count: 0,
    }
}
fn source(id: &str) -> Source {
    Source::SavedItem { id: id.into() }
}
fn progress(id: &str, stage: Stage) -> Progress {
    Progress {
        items: vec![(source(id), stage)],
        ..Progress::default()
    }
}

// Badges must describe actual admitted work without replacing usable original labels.
#[test]
fn badges_keep_local_path_disambiguation_and_unqueued_items_unmarked() {
    let entries = vec![
        Entry::Saved(saved("a")),
        Entry::Saved(saved("b")),
        Entry::Saved(saved("old")),
    ];
    let p = Progress {
        items: vec![
            (source("a"), Stage::Waiting),
            (source("b"), Stage::Processing),
        ],
        ..Progress::default()
    };
    let before = palette::row_details(&entries, "");
    let rows = palette::row_details_with_progress(&entries, "", &p);
    assert_eq!(rows[0].organizer_stage, Some(Stage::Waiting));
    assert_eq!(rows[1].organizer_stage, Some(Stage::Processing));
    assert_eq!(rows[2].organizer_stage, None);
    assert!(rows[0].title.contains('a') && rows[1].title.contains('b'));
    for (old, new) in before.iter().zip(&rows) {
        assert_eq!(old.title, new.title);
        assert_eq!(old.text, new.text);
    }
    let ready = palette::row_details_with_progress(&entries, "", &progress("a", Stage::Ready));
    assert_eq!(ready[0].organizer_stage, Some(Stage::Ready));
    assert_eq!(ready[0].title, rows[0].title);
}

// One actual status read, one dirty marker; late replies cannot resurrect cleared badges.
#[test]
fn progress_refresh_coalesces_bursts_and_rejects_pre_disable_responses() {
    let mut cache = ProgressCache::default();
    cache.mark_dirty();
    let first = cache.request().unwrap();
    for _ in 0..100 {
        cache.mark_dirty();
        assert!(cache.request().is_none());
    }
    assert!(!cache.complete(first.wrapping_add(1), progress("wrong", Stage::Ready)));
    assert!(cache.request().is_none());
    assert!(!cache.complete(first, progress("old", Stage::Processing)));
    let second = cache.request().unwrap();
    assert!(cache.complete(second, progress("current", Stage::Waiting)));
    assert_eq!(
        cache.snapshot().items,
        vec![(source("current"), Stage::Waiting)]
    );
    assert!(cache.request().is_none());
    cache.mark_dirty();
    let third = cache.request().unwrap();
    cache.clear();
    assert!(cache.snapshot().items.is_empty());
    assert!(!cache.complete(third, progress("current", Stage::Ready)));
    assert!(cache.snapshot().items.is_empty());
}

// Applying AI metadata must not sort a visible list, change targets, or overwrite newer edits.
#[test]
fn metadata_patch_keeps_order_targets_and_newer_manual_values() {
    let update = MetadataUpdate {
        source: source("b"),
        previous_alias: String::new(),
        previous_tags: vec![],
        alias: "项目 B 日志".into(),
        tags: vec!["项目 B".into()],
    };
    let mut rows = vec![Entry::Saved(saved("a")), Entry::Saved(saved("b"))];
    assert!(palette::apply_organizer_metadata(
        &mut rows,
        std::slice::from_ref(&update)
    ));
    assert_eq!(rows.iter().map(Entry::id).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(rows[1].title(), "项目 B 日志");
    assert_eq!(rows[1].saved().unwrap().subtitle, "C:\\Projects\\b\\logs");
    assert_eq!(rows[0].saved().unwrap().alias, "");
    assert_eq!(rows[1].saved().unwrap().tags, ["项目 B"]);
    let mut manual = saved("b");
    manual.alias = "我的日志".into();
    let mut rows = vec![Entry::Saved(manual)];
    assert!(!palette::apply_organizer_metadata(&mut rows, &[update]));
    assert_eq!(rows[0].title(), "我的日志");
}

// A query already in flight may finish after an AI update; do not drop its late
// saved/path results just to keep the user's current row order.
#[test]
fn late_results_keep_visible_order_and_append_new_matches() {
    let previous = vec![Entry::Saved(saved("b")), Entry::Saved(saved("a"))];
    let next = vec![
        Entry::Saved(saved("new")),
        Entry::Saved(saved("a")),
        Entry::Saved(saved("b")),
    ];
    let rows = palette::preserve_visible_order(&previous, next);
    assert_eq!(
        rows.iter().map(Entry::id).collect::<Vec<_>>(),
        ["b", "a", "new"]
    );
}

#[test]
fn organizer_entry_exposes_global_cooldown_without_per_item_errors() {
    let mut p = progress("a", Stage::Waiting);
    p.cooling = true;
    assert!(p.label().contains("冷却"));
    assert_eq!(p.items[0].1, Stage::Waiting);
    p.cooling = false;
    p.paused = true;
    assert!(p.label().contains("暂停"));
    p.paused = false;
    p.items = vec![(source("a"), Stage::Ready)];
    assert!(p.label().contains("1") && p.label().contains("建议"));
}
