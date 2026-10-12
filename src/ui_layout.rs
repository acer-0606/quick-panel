//! Client geometry consumed directly by native windows. Short workareas scroll
//! content; each control remains a real, independently reachable rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}
impl Rect {
    pub fn right(self) -> i32 {
        self.x + self.width
    }
    pub fn bottom(self) -> i32 {
        self.y + self.height
    }
}
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub rects: Vec<Rect>,
    pub content_height: i32,
    pub viewport_height: i32,
}
impl Layout {
    pub fn clamp_scroll(&self, position: i32) -> i32 {
        position.clamp(0, (self.content_height - self.viewport_height).max(0))
    }
    pub fn reveal(&self, index: usize, position: i32) -> i32 {
        let Some(r) = self.rects.get(index).filter(|r| r.height > 0) else {
            return self.clamp_scroll(position);
        };
        self.clamp_scroll(if r.y < position {
            r.y
        } else if r.bottom() > position + self.viewport_height {
            r.bottom() - self.viewport_height
        } else {
            position
        })
    }
}
struct Flow {
    layout: Layout,
    width: i32,
    pad: i32,
    gap: i32,
    y: i32,
    dpi: u32,
}
impl Flow {
    fn new(width: i32, height: i32, dpi: u32, count: usize) -> Self {
        let pad = (12 * dpi as i32 / 96).min(width / 12).max(0);
        Self {
            layout: Layout {
                rects: vec![Rect::default(); count],
                content_height: height,
                viewport_height: height,
            },
            width,
            pad,
            gap: (8 * dpi as i32 / 96).max(1),
            y: pad,
            dpi,
        }
    }
    fn scale(&self, n: i32) -> i32 {
        n * self.dpi as i32 / 96
    }
    fn row(&mut self, items: &[(usize, i32)], logical_height: i32) {
        let available = self.width - 2 * self.pad;
        let height = self
            .scale(logical_height)
            .min(self.layout.viewport_height - 2 * self.pad)
            .max(1);
        let mut x = self.pad;
        for &(index, logical_width) in items {
            let width = if logical_width == 0 {
                available
            } else {
                self.scale(logical_width).min(available)
            };
            if x != self.pad && x + width > self.width - self.pad {
                self.y += height + self.gap;
                x = self.pad;
            }
            self.layout.rects[index] = Rect {
                x,
                y: self.y,
                width,
                height,
            };
            x += width + self.gap;
        }
        self.y += height + self.gap;
    }
    fn fit_list(&mut self, index: usize, following: &[usize], side: Option<usize>) {
        let overflow = (self.y + self.pad - self.layout.viewport_height).max(0);
        let shrink = overflow.min((self.layout.rects[index].height - self.scale(96)).max(0));
        self.layout.rects[index].height -= shrink;
        if let Some(side) = side {
            self.layout.rects[side].height -= shrink;
        }
        for &i in following {
            self.layout.rects[i].y -= shrink;
        }
        self.y -= shrink;
    }
    fn finish(mut self) -> Layout {
        self.layout.content_height = (self.y + self.pad).max(self.layout.viewport_height);
        self.layout
    }
}
pub fn review_layout(width: i32, height: i32, dpi: u32) -> Layout {
    let mut f = Flow::new(width, height, dpi, 13);
    f.row(&[(0, 0)], 46);
    f.row(&[(1, 0)], 128);
    f.row(&[(2, 0)], 80);
    for (label, edit) in [(3, 4), (5, 6)] {
        if width >= f.scale(360) {
            let w = width - 2 * f.pad - f.scale(66) - f.gap;
            let y = f.y;
            f.row(&[(label, 66)], 26);
            f.layout.rects[edit] = Rect {
                x: f.pad + f.scale(66) + f.gap,
                y,
                width: w,
                height: f.layout.rects[label].height,
            };
        } else {
            f.row(&[(label, 0)], 24);
            f.row(&[(edit, 0)], 28);
        }
    }
    f.row(&[(11, 148), (12, 148)], 32);
    f.row(&[(7, 112), (8, 112), (9, 84), (10, 84)], 32);
    f.finish()
}
pub fn account_layout(width: i32, height: i32, dpi: u32) -> Layout {
    let mut f = Flow::new(width, height, dpi, 9);
    f.row(&[(0, 0)], 28);
    f.row(&[(1, 0)], 66);
    f.row(&[(2, 0)], 32);
    let list = (height * 96 / dpi as i32 - 250).max(96);
    f.row(&[(3, 0)], list);
    f.row(&[(4, 180), (5, 160), (6, 100)], 32);
    f.row(&[(7, 180), (8, 180)], 32);
    f.fit_list(3, &[4, 5, 6, 7, 8], None);
    f.finish()
}
pub fn panel_layout(
    width: i32,
    height: i32,
    dpi: u32,
    index: bool,
    organizer: bool,
    preview: bool,
) -> Layout {
    let mut f = Flow::new(width, height, dpi, 14);
    let available = width - 2 * f.pad;
    if available >= f.scale(440) {
        let edit_width = available - f.scale(106 + 80) - 2 * f.gap;
        let y = f.y;
        f.row(&[(0, 0)], 36);
        let h = f.layout.rects[0].height;
        f.layout.rects[0].width = edit_width;
        f.layout.rects[1] = Rect {
            x: f.pad + edit_width + f.gap,
            y,
            width: f.scale(106),
            height: h,
        };
        f.layout.rects[2] = Rect {
            x: f.pad + edit_width + 2 * f.gap + f.scale(106),
            y,
            width: f.scale(80),
            height: h,
        };
    } else {
        f.row(&[(0, 0)], 36);
        f.row(&[(1, 106), (2, 80)], 36);
    }
    f.row(&[(3, 89), (4, 89), (5, 89), (6, 89), (7, 62)], 30);
    if index {
        f.row(&[(8, 0)], 26);
    }
    if organizer {
        f.row(&[(9, 0)], 26);
    }
    let remaining = (height - f.y - f.scale(54)) * 96 / dpi as i32;
    f.row(&[(10, 0)], remaining.max(96));
    if preview {
        if available >= f.scale(640) {
            let r = f.layout.rects[10];
            let preview_width = f.scale(232);
            f.layout.rects[10].width -= preview_width + f.gap;
            f.layout.rects[13] = Rect {
                x: r.right() - preview_width,
                y: r.y,
                width: preview_width,
                height: r.height,
            };
        } else {
            f.row(&[(13, 0)], 128);
        }
    }
    let footer_width = available - f.scale(76) - f.gap;
    if footer_width >= f.scale(120) {
        let y = f.y;
        f.row(&[(11, 0)], 30);
        f.layout.rects[11].width = footer_width;
        f.layout.rects[12] = Rect {
            x: f.pad + footer_width + f.gap,
            y,
            width: f.scale(76),
            height: f.layout.rects[11].height,
        };
    } else {
        f.row(&[(11, 0)], 30);
        f.row(&[(12, 76)], 30);
    }
    if preview && f.layout.rects[13].y == f.layout.rects[10].y {
        f.fit_list(10, &[11, 12], Some(13));
    } else if preview {
        f.fit_list(10, &[11, 12, 13], None);
    } else {
        f.fit_list(10, &[11, 12], None);
    }
    f.finish()
}
pub fn clamp_window(rect: Rect, work: Rect) -> Rect {
    let width = rect.width.clamp(1, work.width.max(1));
    let height = rect.height.clamp(1, work.height.max(1));
    Rect {
        x: rect.x.clamp(work.x, work.right() - width),
        y: rect.y.clamp(work.y, work.bottom() - height),
        width,
        height,
    }
}
/// Logical outer size deliberately lives only in RAM; clipping does not change it.
#[derive(Default, Debug)]
pub struct PanelSize {
    logical: Option<(i32, i32)>,
}
impl PanelSize {
    pub fn remember(&mut self, width: i32, height: i32, dpi: u32) {
        self.logical = Some((
            width * 96 / dpi.max(96) as i32,
            height * 96 / dpi.max(96) as i32,
        ));
    }
    pub fn physical(
        &self,
        dpi: u32,
        work_width: i32,
        work_height: i32,
        preview: bool,
    ) -> (i32, i32) {
        let (w, h) = self
            .logical
            .unwrap_or((if preview { 930 } else { 680 }, 480));
        (
            (w * dpi as i32 / 96).min(work_width),
            (h * dpi as i32 / 96).min(work_height),
        )
    }
}
