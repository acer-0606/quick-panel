use quick_panel::ui_layout::*;
fn nonoverlap(a: Rect, b: Rect) -> bool {
    a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y
}
#[test]
fn actual_layouts_reflow_and_scroll_on_short_narrow_workareas() {
    for dpi in [96, 144, 192, 240] {
        for width in [240, 464, 640, 1000] {
            for height in [180, 260, 600] {
                for layout in [
                    review_layout(width, height, dpi),
                    account_layout(width, height, dpi),
                    panel_layout(width, height, dpi, true, true, true),
                ] {
                    assert!(layout.content_height >= height);
                    for (i, a) in layout
                        .rects
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.height > 0)
                    {
                        assert!(
                            a.x >= 0 && a.y >= 0 && a.width > 0 && a.height > 0,
                            "{dpi}/{width}/{height}: {a:?}"
                        );
                        assert!(a.right() <= width && a.bottom() <= layout.content_height);
                        let scroll = layout.reveal(i, 0);
                        assert!(
                            a.y - scroll >= 0 && a.bottom() - scroll <= height,
                            "action unreachable: {a:?}"
                        );
                        for b in layout.rects[i + 1..].iter().filter(|r| r.height > 0) {
                            assert!(nonoverlap(*a, *b), "{a:?} overlaps {b:?}");
                        }
                    }
                    if height == 180 {
                        assert!(layout.content_height > height);
                    }
                }
            }
        }
    }
}
#[test]
fn manual_panel_size_is_ram_only_and_workarea_clipping_does_not_overwrite_preference() {
    let mut size = PanelSize::default();
    size.remember(900, 700, 144);
    assert_eq!(size.physical(96, 2000, 1200, false), (600, 466));
    assert_eq!(size.physical(240, 500, 350, false), (500, 350));
    assert_eq!(size.physical(96, 2000, 1200, false), (600, 466));
    let r = clamp_window(
        Rect {
            x: -400,
            y: 100,
            width: 2000,
            height: 1200,
        },
        Rect {
            x: 20,
            y: 30,
            width: 500,
            height: 350,
        },
    );
    assert_eq!(
        r,
        Rect {
            x: 20,
            y: 30,
            width: 500,
            height: 350
        }
    );
}

#[test]
fn ordinary_windows_allocate_list_space_without_unnecessary_outer_scrolling() {
    assert_eq!(account_layout(640, 600, 96).content_height, 600);
    assert_eq!(
        panel_layout(1000, 600, 96, true, true, true).content_height,
        600
    );
}

#[test]
fn moving_a_clipped_panel_does_not_replace_manual_size_preference() {
    let mut size = PanelSize::default();
    size.remember(900, 700, 96);
    let clipped = size.physical(96, 600, 500, false);
    assert_eq!(clipped, (600, 500));
    size.begin_move_or_size();
    size.finish_move_or_size(clipped.0, clipped.1, 96);
    assert_eq!(size.physical(96, 2000, 1200, false), (900, 700));
    size.begin_move_or_size();
    size.user_sizing();
    size.finish_move_or_size(750, 550, 96);
    assert_eq!(size.physical(96, 2000, 1200, false), (750, 550));
    // A subsequent pure move cannot inherit the previous sizing intent.
    size.begin_move_or_size();
    size.finish_move_or_size(600, 500, 96);
    assert_eq!(size.physical(96, 2000, 1200, false), (750, 550));
}
