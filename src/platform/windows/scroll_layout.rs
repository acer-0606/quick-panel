//! Native scroll/reflow plumbing shared by the panel and management windows.
#![allow(unsafe_op_in_unsafe_fn)]
use crate::ui_layout::{Layout, Rect, clamp_window};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{
        Controls::*,
        HiDpi::AdjustWindowRectExForDpi,
        Input::KeyboardAndMouse::GetFocus,
        Shell::{DefSubclassProc, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};
pub const REVEAL: u32 = WM_APP + 0x187;
#[derive(Default)]
pub struct ScrollLayout {
    pub geometry: Layout,
    pub controls: Vec<HWND>,
    pub position: i32,
    dpi: u32,
}
impl ScrollLayout {
    pub unsafe fn reflow(&mut self, hwnd: HWND, controls: Vec<HWND>, geometry: Layout, dpi: u32) {
        self.position = self.position * dpi as i32 / self.dpi.max(96) as i32;
        self.dpi = dpi;
        self.controls = controls;
        self.geometry = geometry;
        let focus = GetFocus();
        for c in &self.controls {
            attach_focus(*c);
        }
        if let Some(i) = self.controls.iter().position(|c| *c == focus) {
            self.position = self.geometry.reveal(i, self.position);
        }
        self.apply(hwnd);
    }
    pub unsafe fn apply(&mut self, hwnd: HWND) {
        self.position = self.geometry.clamp_scroll(self.position);
        SetScrollInfo(
            hwnd,
            SB_VERT,
            &SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
                nMax: self.geometry.content_height - 1,
                nPage: self.geometry.viewport_height as u32,
                nPos: self.position,
                ..Default::default()
            },
            true,
        );
        for (c, r) in self.controls.iter().zip(&self.geometry.rects) {
            if r.height > 0 {
                let _ = MoveWindow(*c, r.x, r.y - self.position, r.width, r.height, true);
            }
        }
    }
    pub unsafe fn message(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> bool {
        if msg == REVEAL {
            if let Some(i) = self.controls.iter().position(|c| c.0 as isize == lp.0) {
                self.position = self.geometry.reveal(i, self.position);
                self.apply(hwnd);
            }
            return true;
        }
        if msg != WM_VSCROLL && msg != WM_MOUSEWHEEL {
            return false;
        }
        let step = 24 * self.dpi.max(96) as i32 / 96;
        self.position = if msg == WM_MOUSEWHEEL {
            self.position - ((wp.0 >> 16) as u16 as i16 as i32) / 120 * step * 3
        } else {
            match wp.0 & 0xffff {
                0 => self.position - step,
                1 => self.position + step,
                2 => self.position - self.geometry.viewport_height,
                3 => self.position + self.geometry.viewport_height,
                4 | 5 => {
                    let mut info = SCROLLINFO {
                        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                        fMask: SIF_TRACKPOS,
                        ..Default::default()
                    };
                    let _ = GetScrollInfo(hwnd, SB_VERT, &mut info);
                    info.nTrackPos
                }
                6 => 0,
                7 => i32::MAX,
                _ => self.position,
            }
        };
        self.apply(hwnd);
        true
    }
}
pub unsafe fn attach_focus(hwnd: HWND) {
    let _ = SetWindowSubclass(hwnd, Some(focus_proc), 0x187, 0);
}
unsafe extern "system" fn focus_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_SETFOCUS {
        SendMessageW(
            GetParent(hwnd).unwrap_or_default(),
            REVEAL,
            None,
            Some(LPARAM(hwnd.0 as isize)),
        );
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}
pub unsafe fn clamp_rect(rect: RECT) -> RECT {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = GetMonitorInfoW(MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST), &mut info);
    let work = info.rcWork;
    let r = clamp_window(
        Rect {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        },
        Rect {
            x: work.left,
            y: work.top,
            width: work.right - work.left,
            height: work.bottom - work.top,
        },
    );
    RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}
pub unsafe fn min_track(hwnd: HWND, dpi: u32, width: i32, height: i32) -> POINT {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width * dpi as i32 / 96,
        bottom: height * dpi as i32 / 96,
    };
    let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
    let ex = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
    let _ = AdjustWindowRectExForDpi(&mut rect, style, false, ex, dpi);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info);
    POINT {
        x: (rect.right - rect.left).min(info.rcWork.right - info.rcWork.left),
        y: (rect.bottom - rect.top).min(info.rcWork.bottom - info.rcWork.top),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windows::{
        Win32::UI::{HiDpi::GetDpiForWindow, Input::KeyboardAndMouse::SetFocus},
        core::w,
    };
    unsafe extern "system" fn fixture_proc(
        hwnd: HWND,
        msg: u32,
        wp: WPARAM,
        lp: LPARAM,
        _id: usize,
        data: usize,
    ) -> LRESULT {
        if matches!(msg, WM_VSCROLL | WM_MOUSEWHEEL | REVEAL) {
            (*(data as *const RefCell<ScrollLayout>))
                .borrow_mut()
                .message(hwnd, msg, wp, lp);
            return LRESULT(0);
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
    #[test]
    fn native_short_window_scroll_reflow_and_focus_reaches_all_real_controls() {
        unsafe {
            for dpi in [96, 144, 192, 240] {
                for height in [180, 260] {
                    let root = CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        w!("STATIC"),
                        w!("合成滚动fixture"),
                        WS_OVERLAPPEDWINDOW | WS_VSCROLL,
                        0,
                        0,
                        480,
                        400,
                        None,
                        None,
                        None,
                        None,
                    )
                    .unwrap();
                    let scroll = Box::new(RefCell::new(ScrollLayout::default()));
                    SetWindowSubclass(
                        root,
                        Some(fixture_proc),
                        1,
                        (&*scroll as *const RefCell<ScrollLayout>) as usize,
                    )
                    .unwrap();
                    let _ = ShowWindow(root, SW_SHOW);
                    let controls = (0..14)
                        .map(|_| {
                            CreateWindowExW(
                                WINDOW_EX_STYLE::default(),
                                w!("BUTTON"),
                                w!("合成动作"),
                                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                                0,
                                0,
                                40,
                                30,
                                Some(root),
                                None,
                                None,
                                None,
                            )
                            .unwrap()
                        })
                        .collect::<Vec<_>>();
                    for geometry in [
                        crate::ui_layout::review_layout(240, height, dpi),
                        crate::ui_layout::account_layout(240, height, dpi),
                        crate::ui_layout::panel_layout(240, height, dpi, true, true, true),
                    ] {
                        let mut outer = RECT {
                            left: 0,
                            top: 0,
                            right: 240,
                            bottom: height,
                        };
                        AdjustWindowRectExForDpi(
                            &mut outer,
                            WS_OVERLAPPEDWINDOW | WS_VSCROLL,
                            false,
                            WINDOW_EX_STYLE::default(),
                            GetDpiForWindow(root).max(96),
                        )
                        .unwrap();
                        let _ = MoveWindow(
                            root,
                            0,
                            0,
                            outer.right - outer.left,
                            outer.bottom - outer.top,
                            true,
                        );
                        let n = geometry.rects.len();
                        scroll
                            .borrow_mut()
                            .reflow(root, controls[..n].to_vec(), geometry, dpi);
                        for control in &controls[..n] {
                            let _ = SetFocus(Some(*control));
                            let mut r = RECT::default();
                            GetWindowRect(*control, &mut r).unwrap();
                            let mut p = [
                                POINT {
                                    x: r.left,
                                    y: r.top,
                                },
                                POINT {
                                    x: r.right,
                                    y: r.bottom,
                                },
                            ];
                            MapWindowPoints(None, Some(root), &mut p);
                            assert!(
                                p[0].x >= 0 && p[1].x <= 240 && p[0].y >= 0 && p[1].y <= height,
                                "{dpi}/{height}: {p:?}"
                            );
                        }
                        SendMessageW(root, WM_VSCROLL, Some(WPARAM(SB_TOP.0 as usize)), None);
                        assert_eq!(scroll.borrow().position, 0);
                        SendMessageW(root, WM_VSCROLL, Some(WPARAM(SB_BOTTOM.0 as usize)), None);
                        assert!(scroll.borrow().position > 0);
                    }
                    let _ = DestroyWindow(root);
                }
            }
        }
    }
}
