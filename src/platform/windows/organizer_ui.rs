//! Native completed-result review. Its callbacks only submit actor commands.
#![allow(unsafe_op_in_unsafe_fn)]
use super::{
    dialogs::{text, wide},
    theme,
};
use crate::{
    model::{Command, Theme},
    organizer_ui::ReviewState,
};
use std::{cell::RefCell, rc::Rc};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};
const LIST: usize = 10;
const ALIAS: usize = 11;
const TAGS: usize = 12;
struct View {
    state: Rc<RefCell<ReviewState>>,
    submit: Rc<dyn Fn(Command)>,
    controls: Vec<HWND>,
    list: HWND,
    status: HWND,
    detail: HWND,
    alias: HWND,
    tags: HWND,
    apply: HWND,
    dismiss: HWND,
    retry: HWND,
    colors: theme::Colors,
    appearance: Theme,
    font: HFONT,
    dpi: u32,
    revision: u64,
    rendering: bool,
    done: bool,
    enabled: bool,
}
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}
unsafe fn set(hwnd: HWND, value: &str) {
    if text(hwnd) == value {
        return;
    }
    let value = wide(value);
    let _ = SetWindowTextW(hwnd, PCWSTR(value.as_ptr()));
}
unsafe fn list_theme(ptr: *const RefCell<View>) {
    let (list, palette) = {
        let v = (*ptr).borrow();
        (v.list, v.colors.palette)
    };
    SendMessageW(
        list,
        LVM_SETBKCOLOR,
        None,
        Some(LPARAM(palette.surface.0 as isize)),
    );
    SendMessageW(
        list,
        LVM_SETTEXTBKCOLOR,
        None,
        Some(LPARAM(palette.surface.0 as isize)),
    );
    SendMessageW(
        list,
        LVM_SETTEXTCOLOR,
        None,
        Some(LPARAM(palette.text.0 as isize)),
    );
}
unsafe fn selected(ptr: *const RefCell<View>) {
    let (state, list, detail, alias, tags) = {
        let v = (*ptr).borrow();
        (v.state.clone(), v.list, v.detail, v.alias, v.tags)
    };
    let index = SendMessageW(
        list,
        LVM_GETNEXTITEM,
        Some(WPARAM(usize::MAX)),
        Some(LPARAM(LVNI_SELECTED as isize)),
    )
    .0;
    state
        .borrow_mut()
        .select((index >= 0).then_some(index as usize));
    let proposal = state.borrow().selected();
    let request = state.borrow_mut().request_detail();
    if let Some((request_id, source, expected_version)) = request {
        let submit = (*ptr).borrow().submit.clone();
        submit(Command::OrganizerDetail {
            request_id,
            source,
            expected_version,
        });
    }
    (*ptr).borrow_mut().rendering = true;
    if let Some(p) = proposal {
        let original = if p.candidate.kind == crate::model::ItemKind::Clipboard {
            String::new()
        } else {
            match state.borrow().detail_target() {
                Some(Ok(target)) => format!("\r\n完整原路径：\r\n{target}\r\n"),
                Some(Err(error)) => format!("\r\n{error}\r\n"),
                None => "\r\n正在读取本地原路径…\r\n".into(),
            }
        };
        set(
            detail,
            &format!(
                "原名称：{}\r\n已有别名：{}\r\n原标签：{}{}\r\n来源摘要（最多 1024 字符）：\r\n{}\r\n\r\n建议依据：{}",
                p.candidate.title,
                p.candidate.alias,
                p.candidate.tags.join("、"),
                original,
                p.candidate.context.replace('\n', "\r\n"),
                p.reason
            ),
        );
        set(alias, p.alias.as_deref().unwrap_or_default());
        set(tags, &p.tags.join(", "));
    } else {
        set(detail, "选择一条建议查看原始名称、完整路径或文本摘要");
        set(alias, "");
        set(tags, "");
    }
    let editable = state.borrow().selected_index().is_some()
        && !state.borrow().pending()
        && !state.borrow().needs_save();
    let _ = EnableWindow(alias, editable);
    let _ = EnableWindow(tags, editable);
    (*ptr).borrow_mut().rendering = false;
}
unsafe fn render(ptr: *const RefCell<View>) {
    let (state, list, status, apply, dismiss, retry, enabled, revision) = {
        let v = (*ptr).borrow();
        (
            v.state.clone(),
            v.list,
            v.status,
            v.apply,
            v.dismiss,
            v.retry,
            v.enabled,
            v.revision,
        )
    };
    let (current, message, rows, selection, pending, needs_save, blocked, recoverable) = {
        let s = state.borrow();
        (
            s.revision,
            s.status(enabled),
            s.snapshot
                .proposals
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    (
                        p.candidate.title.clone(),
                        p.alias.clone().unwrap_or_else(|| "保留原别名".into()),
                        p.tags.join("、"),
                        s.checked(i),
                    )
                })
                .collect::<Vec<_>>(),
            s.selected_index(),
            s.pending(),
            s.needs_save(),
            s.snapshot.storage_error.is_some(),
            s.snapshot.paused || s.snapshot.retry_at_ms.is_some(),
        )
    };
    if revision == current {
        return;
    }
    (*ptr).borrow_mut().rendering = true;
    set(status, &message);
    set(retry, if recoverable { "重试" } else { "刷新" });
    set(
        apply,
        if needs_save {
            "重试保存"
        } else {
            "应用所选"
        },
    );
    SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
    SendMessageW(list, LVM_DELETEALLITEMS, None, None);
    for (i, (title, alias, tags, checked)) in rows.iter().enumerate() {
        let title = wide(title);
        let mut item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: i as i32,
            pszText: windows::core::PWSTR(title.as_ptr() as *mut _),
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_INSERTITEMW,
            None,
            Some(LPARAM((&mut item as *mut LVITEMW) as isize)),
        );
        for (column, value) in [(1, alias), (2, tags)] {
            let value = wide(value);
            let mut item = LVITEMW {
                iItem: i as i32,
                iSubItem: column,
                pszText: windows::core::PWSTR(value.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_SETITEMTEXTW,
                Some(WPARAM(i)),
                Some(LPARAM((&mut item as *mut LVITEMW) as isize)),
            );
        }
        let mut item = LVITEMW {
            stateMask: LVIS_STATEIMAGEMASK,
            state: LIST_VIEW_ITEM_STATE_FLAGS(if *checked { 2 << 12 } else { 1 << 12 }),
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_SETITEMSTATE,
            Some(WPARAM(i)),
            Some(LPARAM((&mut item as *mut LVITEMW) as isize)),
        );
    }
    if let Some(i) = selection {
        let mut item = LVITEMW {
            stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
            state: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_SETITEMSTATE,
            Some(WPARAM(i)),
            Some(LPARAM((&mut item as *mut LVITEMW) as isize)),
        );
    }
    SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
    let _ = InvalidateRect(Some(list), None, true);
    let _ = EnableWindow(apply, !pending && !blocked);
    let _ = EnableWindow(dismiss, !pending && !needs_save && !blocked);
    let _ = EnableWindow(retry, !pending && !needs_save && !blocked);
    {
        let mut v = (*ptr).borrow_mut();
        v.revision = current;
        v.rendering = false;
    }
    selected(ptr);
}
unsafe fn layout(hwnd: HWND, ptr: *const RefCell<View>) {
    let (controls, dpi) = {
        let v = (*ptr).borrow();
        (v.controls.clone(), v.dpi)
    };
    if controls.len() != 11 {
        return;
    }
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    let s = |n: i32| n * dpi as i32 / 96;
    let margin = s(14);
    let width = (r.right - 2 * margin).max(s(120));
    let bottom = r.bottom - margin;
    let field_h = s(26);
    let footer_h = s(32);
    let alias_y = bottom - footer_h - s(84);
    let detail_y = s(66) + ((alias_y - s(66)) / 2).max(s(24));
    let positions = [
        (margin, s(10), width, s(46)),
        (margin, s(62), width, (detail_y - s(70)).max(s(30))),
        (
            margin,
            detail_y,
            width,
            (alias_y - detail_y - s(8)).max(s(30)),
        ),
        (margin, alias_y, s(66), field_h),
        (margin + s(70), alias_y, width - s(70), field_h),
        (margin, alias_y + s(34), s(66), field_h),
        (margin + s(70), alias_y + s(34), width - s(70), field_h),
        (margin, bottom - footer_h, s(112), footer_h),
        (margin + s(122), bottom - footer_h, s(112), footer_h),
        (margin + s(244), bottom - footer_h, s(84), footer_h),
        (r.right - margin - s(84), bottom - footer_h, s(84), footer_h),
    ];
    for (control, (x, y, w, h)) in controls.into_iter().zip(positions) {
        let _ = MoveWindow(control, x, y, w.max(1), h.max(1), true);
    }
    let list = (*ptr).borrow().list;
    for (column, fraction) in [(0, 40), (1, 32), (2, 28)] {
        SendMessageW(
            list,
            LVM_SETCOLUMNWIDTH,
            Some(WPARAM(column)),
            Some(LPARAM((width * fraction / 100) as isize)),
        );
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<View>;
    if !ptr.is_null() {
        match msg {
            WM_TIMER => {
                render(ptr);
                return LRESULT(0);
            }
            WM_SIZE => {
                layout(hwnd, ptr);
                return LRESULT(0);
            }
            WM_COMMAND => {
                let id = wp.0 & 0xffff;
                let notification = (wp.0 >> 16) as u32;
                if (id == ALIAS || id == TAGS)
                    && notification == EN_CHANGE
                    && !(*ptr).borrow().rendering
                {
                    let (state, alias, tags) = {
                        let v = (*ptr).borrow();
                        (v.state.clone(), v.alias, v.tags)
                    };
                    let a = text(alias);
                    let t = text(tags);
                    state.borrow_mut().edit(a, t);
                }
                if [1, 2, 3, 4].contains(&id) {
                    if id == 4 {
                        (*ptr).borrow_mut().done = true;
                    } else {
                        let (state, submit) = {
                            let v = (*ptr).borrow();
                            (v.state.clone(), v.submit.clone())
                        };
                        if id == 3 {
                            let recoverable = {
                                let s = state.borrow();
                                s.snapshot.paused || s.snapshot.retry_at_ms.is_some()
                            };
                            state.borrow_mut().refresh();
                            if recoverable {
                                submit(Command::OrganizerRetry);
                            } else {
                                let request = state.borrow_mut().request_snapshot();
                                if let Some(request_id) = request {
                                    submit(Command::OrganizerSnapshot { request_id });
                                }
                            }
                        } else {
                            let decision = state.borrow_mut().begin_decision(id == 2);
                            match decision {
                                Ok((request_id, decision)) => submit(Command::OrganizerReview {
                                    request_id,
                                    decision,
                                }),
                                Err(error) => {
                                    let mut s = state.borrow_mut();
                                    s.message = error;
                                    s.revision += 1;
                                }
                            }
                        }
                        render(ptr);
                    }
                    return LRESULT(0);
                }
            }
            WM_NOTIFY if !(*ptr).borrow().rendering => {
                let header = &*(lp.0 as *const NMHDR);
                if header.idFrom == LIST && header.code == LVN_ITEMCHANGED {
                    let n = &*(lp.0 as *const NMLISTVIEW);
                    let (state, list) = {
                        let v = (*ptr).borrow();
                        (v.state.clone(), v.list)
                    };
                    if n.iItem >= 0 {
                        let checked = SendMessageW(
                            list,
                            LVM_GETITEMSTATE,
                            Some(WPARAM(n.iItem as usize)),
                            Some(LPARAM(LVIS_STATEIMAGEMASK.0 as isize)),
                        )
                        .0 & (3 << 12)
                            == (2 << 12);
                        state.borrow_mut().check(n.iItem as usize, checked);
                    }
                    selected(ptr);
                }
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                let dpi = (*ptr).borrow().dpi;
                let mut monitor = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                );
                info.ptMinTrackSize = POINT {
                    x: (620 * dpi as i32 / 96).min(monitor.rcWork.right - monitor.rcWork.left - 24),
                    y: (420 * dpi as i32 / 96).min(monitor.rcWork.bottom - monitor.rcWork.top - 24),
                };
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                let (palette, brush) = {
                    let v = (*ptr).borrow();
                    (v.colors.palette, v.colors.background)
                };
                let dc = HDC(wp.0 as *mut _);
                SetTextColor(dc, palette.text);
                SetBkColor(dc, palette.background);
                return LRESULT(brush.0 as isize);
            }
            WM_ERASEBKGND => {
                let brush = (*ptr).borrow().colors.background;
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                FillRect(HDC(wp.0 as *mut _), &r, brush);
                return LRESULT(1);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xffff) as u32;
                let font = theme::font(dpi);
                let controls = (*ptr).borrow().controls.clone();
                for c in controls {
                    SendMessageW(
                        c,
                        WM_SETFONT,
                        Some(WPARAM(font.0 as usize)),
                        Some(LPARAM(1)),
                    );
                }
                let old = {
                    let mut v = (*ptr).borrow_mut();
                    v.dpi = dpi;
                    std::mem::replace(&mut v.font, font)
                };
                let _ = DeleteObject(old.into());
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                layout(hwnd, ptr);
                return LRESULT(0);
            }
            WM_THEMECHANGED | WM_SETTINGCHANGE => {
                let appearance = (*ptr).borrow().appearance;
                (*ptr).borrow_mut().colors = theme::Colors::new(appearance);
                let dark = (*ptr).borrow().colors.dark;
                theme::apply(hwnd, dark);
                list_theme(ptr);
                let _ = InvalidateRect(Some(hwnd), None, true);
                return LRESULT(0);
            }
            WM_CLOSE => {
                (*ptr).borrow_mut().done = true;
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                (*ptr).borrow_mut().done = true;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            _ => {}
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
pub unsafe fn show(
    owner: HWND,
    state: Rc<RefCell<ReviewState>>,
    submit: Rc<dyn Fn(Command)>,
    appearance: Theme,
    enabled: bool,
) {
    let common = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES,
    };
    if !InitCommonControlsEx(&common).as_bool() {
        return;
    }
    let Ok(instance) = GetModuleHandleW(None) else {
        return;
    };
    let class = w!("QuickPanelOrganizerReview");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance.into(),
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let mut monitor = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = GetMonitorInfoW(
        MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    );
    let work = monitor.rcWork;
    let dpi = GetDpiForWindow(owner).max(96);
    let scale = |n: i32| n * dpi as i32 / 96;
    let width = scale(800).min(work.right - work.left - 24);
    let height = scale(640).min(work.bottom - work.top - 24);
    let view = Box::new(RefCell::new(View {
        state: state.clone(),
        submit,
        controls: vec![],
        list: HWND::default(),
        status: HWND::default(),
        detail: HWND::default(),
        alias: HWND::default(),
        tags: HWND::default(),
        apply: HWND::default(),
        dismiss: HWND::default(),
        retry: HWND::default(),
        colors: theme::Colors::new(appearance),
        appearance,
        font: theme::font(dpi),
        dpi,
        revision: u64::MAX,
        rendering: true,
        done: false,
        enabled,
    }));
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class,
        w!("整理建议"),
        WS_OVERLAPPEDWINDOW,
        work.left + (work.right - work.left - width) / 2,
        work.top + (work.bottom - work.top - height) / 2,
        width,
        height,
        Some(owner),
        None,
        Some(instance.into()),
        Some((&*view as *const RefCell<View>).cast()),
    ) else {
        return;
    };
    let font = view.borrow().font;
    let dark = view.borrow().colors.dark;
    theme::apply(hwnd, dark);
    let create = |class: PCWSTR, label: &str, style: WINDOW_STYLE, id: usize| -> HWND {
        let label = wide(label);
        CreateWindowExW(
            WS_EX_CLIENTEDGE,
            class,
            PCWSTR(label.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            1,
            1,
            Some(hwnd),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )
        .unwrap_or_default()
    };
    let status = create(
        w!("STATIC"),
        "正在读取本地建议…",
        WINDOW_STYLE::default(),
        0,
    );
    let list = create(
        w!("SysListView32"),
        "",
        WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS),
        LIST,
    );
    SendMessageW(
        list,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        None,
        Some(LPARAM(
            (LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize,
        )),
    );
    for (i, label) in ["原名称", "建议别名", "建议标签"].iter().enumerate() {
        let label = wide(label);
        let mut column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH,
            cx: scale(200),
            pszText: windows::core::PWSTR(label.as_ptr() as *mut _),
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_INSERTCOLUMNW,
            Some(WPARAM(i)),
            Some(LPARAM((&mut column as *mut LVCOLUMNW) as isize)),
        );
    }
    let detail = create(
        w!("EDIT"),
        "",
        WS_TABSTOP
            | WS_VSCROLL
            | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32),
        0,
    );
    let alabel = create(w!("STATIC"), "别名", WINDOW_STYLE::default(), 0);
    let alias = create(
        w!("EDIT"),
        "",
        WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        ALIAS,
    );
    let tlabel = create(w!("STATIC"), "标签", WINDOW_STYLE::default(), 0);
    let tags = create(
        w!("EDIT"),
        "",
        WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        TAGS,
    );
    SendMessageW(alias, EM_SETLIMITTEXT, Some(WPARAM(96)), None);
    SendMessageW(tags, EM_SETLIMITTEXT, Some(WPARAM(240)), None);
    let apply = create(w!("BUTTON"), "应用所选", WS_TABSTOP, 1);
    let dismiss = create(w!("BUTTON"), "忽略所选", WS_TABSTOP, 2);
    let retry = create(w!("BUTTON"), "重试", WS_TABSTOP, 3);
    let close = create(w!("BUTTON"), "关闭", WS_TABSTOP, 4);
    let controls = vec![
        status, list, detail, alabel, alias, tlabel, tags, apply, dismiss, retry, close,
    ];
    for c in &controls {
        SendMessageW(*c, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
    }
    {
        let mut v = view.borrow_mut();
        v.controls = controls;
        v.list = list;
        v.status = status;
        v.detail = detail;
        v.alias = alias;
        v.tags = tags;
        v.apply = apply;
        v.dismiss = dismiss;
        v.retry = retry;
        v.rendering = false;
    }
    list_theme(&*view);
    layout(hwnd, &*view);
    render(&*view);
    let _ = EnableWindow(owner, false);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetFocus(Some(list));
    SetTimer(Some(hwnd), 1, 250, None);
    let mut msg = MSG::default();
    let mut quit = None;
    while !view.borrow().done {
        let result = GetMessageW(&mut msg, None, 0, 0);
        if result.0 <= 0 {
            quit = Some(msg.wParam.0 as i32);
            break;
        }
        let context = windows::Win32::UI::Input::Ime::ImmGetContext(msg.hwnd);
        let composing = if !context.is_invalid() {
            let composing = windows::Win32::UI::Input::Ime::ImmGetCompositionStringW(
                context,
                windows::Win32::UI::Input::Ime::GCS_COMPSTR,
                None,
                0,
            ) > 0;
            let _ = windows::Win32::UI::Input::Ime::ImmReleaseContext(msg.hwnd, context);
            composing
        } else {
            false
        };
        if msg.message == WM_KEYDOWN && msg.wParam.0 == 27 && !composing {
            view.borrow_mut().done = true;
            continue;
        }
        if composing || !IsDialogMessageW(hwnd, &msg).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    state.borrow_mut().close();
    let _ = KillTimer(Some(hwnd), 1);
    let _ = DestroyWindow(hwnd);
    let _ = EnableWindow(owner, true);
    let _ = SetForegroundWindow(owner);
    if let Some(code) = quit {
        PostQuitMessage(code);
    }
}
