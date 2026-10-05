use quick_panel::{
    model::{ItemKind, ItemSummary},
    preview::{PreviewLoads, bounded_text, capture_format, compatibility_bytes, thumbnail_size},
};

fn item(id: &str) -> ItemSummary {
    ItemSummary {
        id: id.into(),
        kind: ItemKind::Clipboard,
        title: "原文".into(),
        subtitle: "内容".into(),
        alias: String::new(),
        tags: vec![],
        pinned: false,
        created_at_ms: 0,
        last_used_ms: 0,
        use_count: 0,
    }
}
#[test]
fn four_k_preview_fits_four_mib_and_preserves_aspect_without_upscaling() {
    assert_eq!(thumbnail_size(3840, 2160).unwrap(), (1024, 576));
    assert_eq!(thumbnail_size(2160, 3840).unwrap(), (576, 1024));
    assert_eq!(thumbnail_size(30, 20).unwrap(), (30, 20));
    assert_eq!(thumbnail_size(1, 100_000).unwrap(), (1, 1024));
    assert!(thumbnail_size(0, 1).is_err());
    assert!(thumbnail_size(100_000, 100_000).is_err());
    assert_eq!(compatibility_bytes(3840, 2160).unwrap(), 33_177_600);
    assert!(compatibility_bytes(8192, 8192).is_err());
    assert!(compatibility_bytes(u32::MAX, u32::MAX).is_err());
}
#[test]
fn png_survives_oversized_redundant_dib_but_other_formats_keep_budget() {
    assert!(!capture_format("CF_DIB", 33_177_640, 200_000, 8 * 1024 * 1024, true).unwrap());
    assert!(capture_format("CF_DIB", 33_177_640, 0, 8 * 1024 * 1024, false).is_err());
    assert!(capture_format("HTML Format", 100, 200, 250, true).is_err());
    assert!(capture_format("CF_DIBV5", 100, 200, 400, true).unwrap());
}
#[test]
fn preview_loads_only_latest_and_retains_inflight_occupancy_after_cancel() {
    let mut gate = PreviewLoads::default();
    assert_eq!(gate.select(Some(item("a")), 1).unwrap().0, 1);
    assert!(gate.select(Some(item("b")), 2).is_none());
    assert!(gate.select(Some(item("c")), 3).is_none());
    gate.cancel();
    assert!(gate.select(Some(item("d")), 4).is_none());
    let (current, next) = gate.complete(1);
    assert!(!current);
    assert_eq!(next.unwrap().1.id, "d");
    assert_eq!(gate.complete(4), (true, None));
    assert!(gate.select(Some(item("d")), 5).is_none());
    let mut changed = item("d");
    changed.title = "编辑后".into();
    assert_eq!(gate.select(Some(changed), 6).unwrap().0, 6);
}
#[test]
fn usage_changes_do_not_reload_same_content_and_text_truncation_is_explicit() {
    let mut gate = PreviewLoads::default();
    gate.select(Some(item("a")), 1);
    let mut used = item("a");
    used.use_count = 3;
    used.last_used_ms = 9;
    assert!(gate.select(Some(used), 2).is_none());
    assert!(gate.complete(1).0);
    assert_eq!(bounded_text("短文本"), "短文本");
    let text = bounded_text(&"🙂".repeat(6001));
    assert_eq!(text.chars().filter(|c| *c == '🙂').count(), 6000);
    assert!(text.contains("截断"));
}

#[test]
fn failed_preview_dispatches_latest_and_same_failed_target_can_retry() {
    let mut gate = PreviewLoads::default();
    gate.select(Some(item("a")), 1);
    assert!(gate.select(Some(item("b")), 2).is_none());
    assert_eq!(gate.fail(99), (false, None));
    let (current, next) = gate.fail(1);
    assert!(!current);
    assert_eq!(next.unwrap().1.id, "b");
    assert_eq!(gate.fail(2), (true, None));
    assert_eq!(gate.select(Some(item("b")), 3).unwrap().0, 3);
    assert!(gate.complete(3).0);
    assert!(gate.select(Some(item("b")), 4).is_none());
}

#[test]
fn failed_inflight_preview_keeps_a_latest_retry_of_the_same_target() {
    let mut gate = PreviewLoads::default();
    gate.select(Some(item("a")), 1);
    gate.select(Some(item("b")), 2);
    gate.select(Some(item("a")), 3);
    let (current, next) = gate.fail(1);
    assert!(current);
    assert_eq!(next.unwrap().0, 3);
    assert_eq!(gate.fail(1), (false, None));
    assert!(gate.select(Some(item("a")), 4).is_none());
    assert!(gate.complete(3).0);
}
