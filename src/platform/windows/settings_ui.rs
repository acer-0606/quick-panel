//! Native settings hub. The message pump never holds Hub/App borrows across callbacks.
use super::{
    dialogs::{text, wide},
    theme,
};
use crate::{
    model::Theme,
    settings_ui::{Action, Group, layout},
};
use std::cell::RefCell;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};
pub struct Snapshot {
    pub appearance: Theme,
    pub detail: String,
    pub alive: bool,
    pub close_label: &'static str,
}
struct Hub {
    colors: theme::Colors,
    font: HFONT,
    done: bool,
    changed: bool,
    activate: bool,
    resize: bool,
    dpi: u32,
    suggested: Option<RECT>,
}
impl Drop for Hub {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Hub>;
        if !ptr.is_null() {
            match msg {
                WM_CLOSE => {
                    (*ptr).borrow_mut().done = true;
                    return LRESULT(0);
                }
                WM_SIZE => {
                    (*ptr).borrow_mut().resize = true;
                    return LRESULT(0);
                }
                WM_DPICHANGED => {
                    let mut s = (*ptr).borrow_mut();
                    s.dpi = (wp.0 & 0xffff) as u32;
                    s.suggested = Some(*(lp.0 as *const RECT));
                    s.resize = true;
                    return LRESULT(0);
                }
                WM_COMMAND => {
                    let id = wp.0 & 0xffff;
                    let code = (wp.0 >> 16) as u32;
                    if id == 2 {
                        (*ptr).borrow_mut().done = true;
                    } else if id == 1 {
                        (*ptr).borrow_mut().activate = true;
                    } else if id == 10 && (code == CBN_SELCHANGE || code == LBN_SELCHANGE) {
                        (*ptr).borrow_mut().changed = true;
                    }
                    return LRESULT(0);
                }
                WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                    let state = (*ptr).borrow();
                    let dc = HDC(wp.0 as *mut _);
                    SetTextColor(dc, state.colors.palette.text);
                    SetBkColor(dc, state.colors.palette.background);
                    return LRESULT(state.colors.background.0 as isize);
                }
                WM_ERASEBKGND => {
                    let state = (*ptr).borrow();
                    let mut r = RECT::default();
                    let _ = GetClientRect(hwnd, &mut r);
                    FillRect(HDC(wp.0 as *mut _), &r, state.colors.background);
                    return LRESULT(1);
                }
                _ => (),
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
/// None action reads RAM only; Some opens an editor using this hub as its modal owner.
pub unsafe fn show(
    owner: HWND,
    initial: Group,
    mut update: impl FnMut(HWND, Group, Option<Action>) -> Snapshot,
) -> Option<Action> {
    unsafe {
        let initial_snapshot = update(owner, initial, None);
        let instance = GetModuleHandleW(None).ok()?;
        let class = w!("QuickPanelSettingsHub");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            ..Default::default()
        });
        let dpi = GetDpiForWindow(owner).max(96);
        let scale = |v: i32| v * dpi as i32 / 96;
        let mut monitor = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        );
        let work = monitor.rcWork;
        let width = scale(820).min(work.right - work.left);
        let height = scale(600).min(work.bottom - work.top);
        let state = Box::new(RefCell::new(Hub {
            colors: theme::Colors::new(initial_snapshot.appearance),
            font: theme::font(dpi),
            done: false,
            changed: true,
            activate: false,
            resize: true,
            dpi,
            suggested: None,
        }));
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            class,
            w!("设置中心"),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            Some((&*state as *const RefCell<Hub>).cast()),
        )
        .ok()?;
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let _window = Window(hwnd);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let geometry = layout(client.right, client.bottom, dpi);
        let create = |class: PCWSTR,
                      label: &str,
                      style: WINDOW_STYLE,
                      r: crate::settings_ui::Rect,
                      id: usize,
                      combo: bool|
         -> Option<HWND> {
            let label = wide(label);
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                PCWSTR(label.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
                r.x,
                r.y,
                r.width,
                if combo {
                    r.height + scale(210)
                } else {
                    r.height
                },
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )
            .ok()
        };
        let nav_list = create(
            w!("LISTBOX"),
            "",
            WINDOW_STYLE(LBS_NOTIFY as u32 | WS_VSCROLL.0),
            geometry.navigation,
            10,
            false,
        )?;
        let nav_combo = create(
            w!("COMBOBOX"),
            "",
            WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            geometry.navigation,
            10,
            true,
        )?;
        let mut navigation = if geometry.top_navigation {
            nav_combo
        } else {
            nav_list
        };
        let mut get = if geometry.top_navigation {
            CB_GETCURSEL
        } else {
            LB_GETCURSEL
        };
        let detail = create(
            w!("EDIT"),
            "",
            WINDOW_STYLE(
                ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32 | WS_VSCROLL.0,
            ),
            geometry.detail,
            11,
            false,
        )?;
        let action = create(
            w!("COMBOBOX"),
            "",
            WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            geometry.action,
            12,
            true,
        )?;
        let open = create(
            w!("BUTTON"),
            "打开",
            WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
            geometry.open,
            1,
            false,
        )?;
        let close = create(
            w!("BUTTON"),
            initial_snapshot.close_label,
            WINDOW_STYLE::default(),
            geometry.close,
            2,
            false,
        )?;
        for (nav, add, select) in [
            (nav_list, LB_ADDSTRING, LB_SETCURSEL),
            (nav_combo, CB_ADDSTRING, CB_SETCURSEL),
        ] {
            for group in Group::ALL {
                let name = wide(group.title());
                SendMessageW(nav, add, None, Some(LPARAM(name.as_ptr() as isize)));
            }
            SendMessageW(
                nav,
                select,
                Some(WPARAM(
                    Group::ALL.iter().position(|g| *g == initial).unwrap_or(0),
                )),
                None,
            );
        }
        let font = state.borrow().font;
        for control in [nav_list, nav_combo, detail, action, open, close] {
            SendMessageW(control, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
        }
        let dark = state.borrow().colors.dark;
        theme::apply(hwnd, dark);
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(navigation));
        SetTimer(Some(hwnd), 1, 250, None);
        let mut group = initial;
        let mut msg = MSG::default();
        let mut result = None;
        let mut quit = None;
        loop {
            let (done, changed, activate, resize, dpi, suggested) = {
                let mut s = state.borrow_mut();
                let v = (
                    s.done,
                    s.changed,
                    s.activate,
                    s.resize,
                    s.dpi,
                    s.suggested.take(),
                );
                s.resize = false;
                s.changed = false;
                s.activate = false;
                v
            };
            if done {
                break;
            }
            if resize {
                if let Some(rect) = suggested {
                    let mut monitor = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let _ = GetMonitorInfoW(
                        MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                        &mut monitor,
                    );
                    let work = monitor.rcWork;
                    let width = (rect.right - rect.left).min(work.right - work.left);
                    let height = (rect.bottom - rect.top).min(work.bottom - work.top);
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        rect.left.clamp(work.left, work.right - width),
                        rect.top.clamp(work.top, work.bottom - height),
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                let mut client = RECT::default();
                let _ = GetClientRect(hwnd, &mut client);
                let g = layout(client.right, client.bottom, dpi);
                let next = if g.top_navigation {
                    nav_combo
                } else {
                    nav_list
                };
                if next != navigation {
                    let selected = SendMessageW(navigation, get, None, None);
                    navigation = next;
                    get = if g.top_navigation {
                        CB_GETCURSEL
                    } else {
                        LB_GETCURSEL
                    };
                    SendMessageW(
                        navigation,
                        if g.top_navigation {
                            CB_SETCURSEL
                        } else {
                            LB_SETCURSEL
                        },
                        Some(WPARAM(selected.0.max(0) as usize)),
                        None,
                    );
                }
                let _ = ShowWindow(nav_list, if g.top_navigation { SW_HIDE } else { SW_SHOW });
                let _ = ShowWindow(nav_combo, if g.top_navigation { SW_SHOW } else { SW_HIDE });
                let font = theme::font(dpi);
                for (control, r, combo) in [
                    (navigation, g.navigation, g.top_navigation),
                    (detail, g.detail, false),
                    (action, g.action, true),
                    (open, g.open, false),
                    (close, g.close, false),
                ] {
                    let _ = MoveWindow(
                        control,
                        r.x,
                        r.y,
                        r.width,
                        r.height + if combo { 210 * dpi as i32 / 96 } else { 0 },
                        true,
                    );
                }
                for control in [nav_list, nav_combo, detail, action, open, close] {
                    SendMessageW(
                        control,
                        WM_SETFONT,
                        Some(WPARAM(font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                let old = {
                    let mut s = state.borrow_mut();
                    std::mem::replace(&mut s.font, font)
                };
                let _ = DeleteObject(old.into());
            }
            if changed {
                group = Group::ALL
                    .get(SendMessageW(navigation, get, None, None).0 as usize)
                    .copied()
                    .unwrap_or(initial);
                SendMessageW(action, CB_RESETCONTENT, None, None);
                for a in group.actions() {
                    let label = wide(a.title());
                    SendMessageW(
                        action,
                        CB_ADDSTRING,
                        None,
                        Some(LPARAM(label.as_ptr() as isize)),
                    );
                }
                SendMessageW(action, CB_SETCURSEL, Some(WPARAM(0)), None);
            }
            let selected = activate
                .then(|| {
                    group
                        .actions()
                        .get(SendMessageW(action, CB_GETCURSEL, None, None).0 as usize)
                        .copied()
                })
                .flatten();
            if selected.is_some_and(|a| matches!(a, Action::Review | Action::Memory)) {
                // Commit the current group to the RAM navigation frame before suspension.
                let _ = update(hwnd, group, None);
                result = selected;
                break;
            }
            let snapshot = update(hwnd, group, selected);
            if !snapshot.alive {
                break;
            }
            let contents = format!("{}\r\n\r\n{}", group.title(), snapshot.detail);
            if text(detail) != contents {
                let contents = wide(&contents);
                let _ = SetWindowTextW(detail, PCWSTR(contents.as_ptr()));
            }
            let new_colors = theme::Colors::new(snapshot.appearance);
            if new_colors.dark != state.borrow().colors.dark {
                let dark = new_colors.dark;
                state.borrow_mut().colors = new_colors;
                theme::apply(hwnd, dark);
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            let status = GetMessageW(&mut msg, None, 0, 0);
            if status.0 <= 0 {
                quit = Some(msg.wParam.0 as i32);
                break;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == 27 {
                break;
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = KillTimer(Some(hwnd), 1);
        let _ = DestroyWindow(hwnd);
        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        if let Some(code) = quit {
            PostQuitMessage(code);
        }
        result
    }
}
