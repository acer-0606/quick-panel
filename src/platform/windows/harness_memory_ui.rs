//! Native memory manager and seen-snapshot feedback editor. No database/network work.
#![allow(unsafe_op_in_unsafe_fn)]
use super::{
    dialogs::{text, wide},
    theme,
};
use crate::{
    harness_tools::*,
    model::{Command, Theme},
    organizer_ui::{MemoryUiState, memory_request_id},
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
            Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};
struct View {
    state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>,
    target: Option<(String, FeedbackTarget)>,
    shown: Option<MemoryRecord>,
    records: Vec<MemoryRecord>,
    availability: Option<(bool, bool, bool)>,
    controls: Vec<HWND>,
    colors: theme::Colors,
    appearance: Theme,
    font: HFONT,
    dpi: u32,
    done: bool,
    open_settings: bool,
    revision: u64,
    viewport: HWND,
    scroll_y: i32,
    geometry: Option<crate::organizer_ui::MemoryLayout>,
    input_policy: crate::organizer_ui::MemoryInputPolicy,
}
unsafe fn set(hwnd: HWND, value: &str) {
    if text(hwnd) != value {
        let s = wide(value);
        let _ = SetWindowTextW(hwnd, PCWSTR(s.as_ptr()));
    }
}
unsafe fn selected(ptr: *const RefCell<View>) {
    let (controls, state, target) = {
        let v = (*ptr).borrow();
        (v.controls.clone(), v.state.clone(), v.target.is_some())
    };
    if target || state.borrow().saving || state.borrow().pending_save {
        return;
    }
    let index = SendMessageW(controls[1], LB_GETCURSEL, None, None).0;
    let record = (index >= 0)
        .then(|| (*ptr).borrow().records.get(index as usize).cloned())
        .flatten();
    if let Some(record) = record {
        let snapshot = state.borrow().snapshot.clone();
        set(
            controls[2],
            &crate::organizer_ui::memory_detail(&record, &snapshot),
        );
        set(controls[3], &record.text);
        let scope = match record.scope {
            MemoryScope::Item { .. } => 0,
            MemoryScope::Directory { .. } => 1,
            MemoryScope::Global => 2,
        };
        SendMessageW(controls[4], CB_SETCURSEL, Some(WPARAM(scope)), None);
        SendMessageW(
            controls[5],
            BM_SETCHECK,
            Some(WPARAM(usize::from(record.enabled))),
            None,
        );
        SendMessageW(
            controls[6],
            BM_SETCHECK,
            Some(WPARAM(usize::from(record.confirmed))),
            None,
        );
        state.borrow_mut().navigation_draft = None;
        (*ptr).borrow_mut().shown = Some(record);
    }
}
unsafe fn render(ptr: *const RefCell<View>) {
    let (state, controls, old, feedback) = {
        let v = (*ptr).borrow();
        (
            v.state.clone(),
            v.controls.clone(),
            v.revision,
            v.target.is_some(),
        )
    };
    if controls.len() != 11 {
        return;
    }
    let (revision, message, snapshot, saving, pending_save) = {
        let s = state.borrow();
        (
            s.revision,
            s.message.clone(),
            s.snapshot.clone(),
            s.saving,
            s.pending_save,
        )
    };
    if old == revision {
        return;
    }
    let note = snapshot.availability();
    set(
        controls[0],
        &if message.is_empty() {
            note
        } else {
            format!("{message}\r\n{note}")
        },
    );
    let availability = (
        snapshot.organizer_enabled,
        snapshot.memory_enabled,
        snapshot.storage_error.is_some(),
    );
    let rebuild = {
        let v = (*ptr).borrow();
        v.records != snapshot.records || v.availability != Some(availability)
    };
    if !feedback && rebuild && !saving && !pending_save {
        let selected_id = (*ptr).borrow().shown.as_ref().map(|m| m.id);
        let top = SendMessageW(controls[1], LB_GETTOPINDEX, None, None);
        SendMessageW(controls[1], WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(controls[1], LB_RESETCONTENT, None, None);
        for record in &snapshot.records {
            let label = wide(&format!(
                "{} · {} · {} · {} · {}",
                record.kind.label(),
                crate::organizer_ui::memory_scope_label(&record.scope, &snapshot),
                if record.origin == MemoryOrigin::Human {
                    "人工"
                } else {
                    "AI"
                },
                if !record.enabled {
                    "单条已停用"
                } else if snapshot.record_effective(record) {
                    "当前生效"
                } else if record.confirmed {
                    "单条已启用/已确认，当前不生效"
                } else {
                    "待确认"
                },
                record.text
            ));
            SendMessageW(
                controls[1],
                LB_ADDSTRING,
                None,
                Some(LPARAM(label.as_ptr() as isize)),
            );
        }
        if let Some(index) = snapshot
            .records
            .iter()
            .position(|m| Some(m.id) == selected_id)
        {
            SendMessageW(controls[1], LB_SETCURSEL, Some(WPARAM(index)), None);
        }
        SendMessageW(
            controls[1],
            LB_SETTOPINDEX,
            Some(WPARAM(top.0.max(0) as usize)),
            None,
        );
        SendMessageW(controls[1], WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = InvalidateRect(Some(controls[1]), None, true);
        (*ptr).borrow_mut().records = snapshot.records.clone();
    }
    let available = !feedback || snapshot.feedback_available();
    for c in &controls[3..8] {
        let _ = EnableWindow(*c, !saving && !pending_save && available);
    }
    let has_pending = state.borrow().pending.is_some();
    set(
        controls[8],
        if has_pending {
            "重试保存"
        } else {
            "管家设置"
        },
    );
    let _ = EnableWindow(controls[8], !saving);
    let _ = EnableWindow(controls[9], !saving && !pending_save && !feedback);
    scope_preview(ptr);
    (*ptr).borrow_mut().availability = Some(availability);
    (*ptr).borrow_mut().revision = revision;
}
unsafe fn action(ptr: *const RefCell<View>, id: usize) {
    let (state, submit, controls, target, shown) = {
        let v = (*ptr).borrow();
        (
            v.state.clone(),
            v.submit.clone(),
            v.controls.clone(),
            v.target.clone(),
            v.shown.clone(),
        )
    };
    if id == 10 {
        (*ptr).borrow_mut().done = true;
        return;
    }
    if id == 8 {
        let has_pending = state.borrow().pending.is_some();
        if has_pending {
            let command = state.borrow_mut().retry();
            if let Some(command) = command {
                submit(command);
            }
        } else {
            let draft = crate::organizer_ui::MemoryNavigationDraft {
                target: target.as_ref().map(|(_, target)| target.clone()),
                record: shown,
                raw_input: text(controls[3]),
                scope_choice: SendMessageW(controls[4], CB_GETCURSEL, None, None).0.max(0) as usize,
                enabled: SendMessageW(controls[5], BM_GETCHECK, None, None).0 == 1,
                confirmed: SendMessageW(controls[6], BM_GETCHECK, None, None).0 == 1,
            };
            state.borrow_mut().navigation_draft = Some(draft);
            let mut v = (*ptr).borrow_mut();
            v.open_settings = true;
            v.done = true;
        }
        return;
    }
    if state.borrow().saving || state.borrow().pending_save {
        return;
    }
    if target.is_some() && !state.borrow().snapshot.feedback_available() {
        return;
    }
    let command = if id == 9 {
        let Some(record) = shown else {
            return;
        };
        if MessageBoxW(
            Some(controls[9]),
            w!("删除这条整理记忆？"),
            w!("删除记忆"),
            MB_YESNO | MB_ICONQUESTION,
        ) != IDYES
        {
            return;
        }
        Command::HarnessMemoryDelete {
            request_id: memory_request_id(),
            ids: vec![record.id],
        }
    } else {
        let raw = text(controls[3]);
        let value = crate::organizer_ui::memory_text_input(&raw, target.is_some());
        state.borrow_mut().raw_input = raw;
        let value = match value {
            Ok(value) => value,
            Err(message) => {
                state.borrow_mut().message = message;
                state.borrow_mut().revision += 1;
                return;
            }
        };
        let choice = SendMessageW(controls[4], CB_GETCURSEL, None, None).0;
        if let Some((_, target)) = target {
            Command::HarnessFeedback {
                request_id: memory_request_id(),
                sources: vec![target],
                reason: value,
                scope: match choice {
                    1 => FeedbackScope::Directory,
                    2 => FeedbackScope::Global,
                    _ => FeedbackScope::Item,
                },
            }
        } else {
            let Some(record) = shown else {
                return;
            };
            let scope = if choice == 2 {
                MemoryScope::Global
            } else if (choice == 1 && matches!(record.scope, MemoryScope::Directory { .. }))
                || (choice == 0 && matches!(record.scope, MemoryScope::Item { .. }))
            {
                record.scope.clone()
            } else {
                state.borrow_mut().message =
                    "请保留原范围，或明确选择全局；目录反馈请从原内容发起".into();
                state.borrow_mut().revision += 1;
                return;
            };
            Command::HarnessMemoryEdit {
                request_id: memory_request_id(),
                input: MemoryEdit {
                    id: Some(record.id),
                    scope,
                    text: value,
                    enabled: SendMessageW(controls[5], BM_GETCHECK, None, None).0 == 1,
                    confirmed: SendMessageW(controls[6], BM_GETCHECK, None, None).0 == 1,
                },
            }
        }
    };
    let result = state.borrow_mut().submit(command);
    match result {
        Ok(command) => submit(command),
        Err(message) => {
            set(controls[0], &message);
        }
    }
}
unsafe fn scope_preview(ptr: *const RefCell<View>) {
    let (controls, state, target, shown) = {
        let v = (*ptr).borrow();
        (
            v.controls.clone(),
            v.state.clone(),
            v.target.clone(),
            v.shown.clone(),
        )
    };
    if controls.len() != 11 {
        return;
    }
    let snapshot = state.borrow().snapshot.clone();
    let choice = SendMessageW(controls[4], CB_GETCURSEL, None, None).0;
    if let Some((title, target)) = target {
        let scope = match choice {
            1 => "同目录：此目录内的整理来源",
            2 => "全局：所有整理来源",
            _ => "仅当前内容",
        };
        set(
            controls[2],
            &format!(
                "{}\r\n当前所选范围：{scope}",
                crate::organizer_ui::feedback_intro(&title, &target)
            ),
        );
    } else if let Some(record) = shown {
        let valid = choice == 2
            || (choice == 1 && matches!(record.scope, MemoryScope::Directory { .. }))
            || (choice == 0 && matches!(record.scope, MemoryScope::Item { .. }));
        let scope = if choice == 2 {
            MemoryScope::Global
        } else {
            record.scope.clone()
        };
        if !valid {
            set(
                controls[2],
                &format!(
                    "{}\r\n所选范围无效：请保留原范围或明确选择全局；目录反馈请从原内容发起。",
                    crate::organizer_ui::memory_detail(&record, &snapshot)
                ),
            );
            return;
        }
        let mut preview = record.clone();
        preview.text = text(controls[3]);
        set(
            controls[2],
            &format!(
                "{}\r\n\r\n{}",
                crate::organizer_ui::memory_detail(&record, &snapshot),
                crate::organizer_ui::memory_scope_impact(&preview, &scope, &snapshot)
            ),
        );
    }
}
unsafe fn scroll_content(ptr: *const RefCell<View>, position: i32) {
    let (viewport, controls, dpi, geometry) = {
        let v = (*ptr).borrow();
        (v.viewport, v.controls.clone(), v.dpi, v.geometry.clone())
    };
    let Some(geometry) = geometry else {
        return;
    };
    let position = crate::organizer_ui::clamp_scroll(
        position,
        geometry.content_height,
        geometry.viewport_height,
    );
    (*ptr).borrow_mut().scroll_y = position;
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
        nMin: 0,
        nMax: geometry.content_height - 1,
        nPage: geometry.viewport_height as u32,
        nPos: position,
        ..Default::default()
    };
    SetScrollInfo(viewport, SB_VERT, &info, true);
    let fields = crate::organizer_ui::memory_scrolled_fields(&geometry, position);
    // Explicitly position every child. ScrollWindowEx cannot reliably move/repaint
    // a child that is only partly inside the source viewport.
    let flags = SWP_NOZORDER | SWP_NOACTIVATE;
    let heights = fields.map(|r| r.height.max(1));
    let height = |index: usize| {
        if index == 4 {
            160 * dpi as i32 / 96
        } else {
            heights[index]
        }
    };
    let batch = BeginDeferWindowPos(7).and_then(|mut batch| {
        for (index, r) in fields.iter().enumerate() {
            batch = DeferWindowPos(
                batch,
                controls[index],
                None,
                r.x,
                r.y,
                r.width.max(1),
                height(index),
                flags,
            )?;
        }
        EndDeferWindowPos(batch)
    });
    if batch.is_err() {
        // If allocation for the batch fails, retain correct absolute positions.
        for (index, r) in fields.iter().enumerate() {
            let _ = SetWindowPos(
                controls[index],
                None,
                r.x,
                r.y,
                r.width.max(1),
                height(index),
                flags,
            );
        }
    }
    let _ = RedrawWindow(
        Some(viewport),
        None,
        None,
        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
    );
}
unsafe fn reveal_control(ptr: *const RefCell<View>, hwnd: HWND) {
    let (index, position, geometry) = {
        let v = (*ptr).borrow();
        (
            v.controls.iter().take(7).position(|c| *c == hwnd),
            v.scroll_y,
            v.geometry.clone(),
        )
    };
    if let (Some(index), Some(geometry)) = (index, geometry) {
        scroll_content(
            ptr,
            crate::organizer_ui::memory_focus_scroll(&geometry, index, position),
        );
    }
}
unsafe extern "system" fn focus_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let ptr = data as *const RefCell<View>;
    if msg == WM_SETFOCUS {
        reveal_control(ptr, hwnd);
    }
    if matches!(msg, WM_IME_STARTCOMPOSITION | WM_IME_ENDCOMPOSITION) {
        // Only the editable text field participates; keep START active even before
        // the IME has supplied a nonempty composition string.
        let editor = (*ptr).borrow().controls.get(3).copied();
        if editor == Some(hwnd) {
            (*ptr)
                .borrow_mut()
                .input_policy
                .composition(msg == WM_IME_STARTCOMPOSITION);
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}
unsafe extern "system" fn viewport_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let ptr = data as *const RefCell<View>;
    if msg == WM_VSCROLL || msg == WM_MOUSEWHEEL {
        let (position, dpi, height) = {
            let v = (*ptr).borrow();
            (
                v.scroll_y,
                v.dpi,
                v.geometry.as_ref().map_or(1, |g| g.viewport_height),
            )
        };
        let step = 24 * dpi as i32 / 96;
        let next = if msg == WM_MOUSEWHEEL {
            position - ((wp.0 >> 16) as u16 as i16 as i32) / 120 * step * 3
        } else {
            match wp.0 & 0xffff {
                0 => position - step,
                1 => position + step,
                2 => position - height,
                3 => position + height,
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
                _ => position,
            }
        };
        scroll_content(ptr, next);
        return LRESULT(0);
    }
    if [
        WM_COMMAND,
        WM_CTLCOLOREDIT,
        WM_CTLCOLORSTATIC,
        WM_CTLCOLORLISTBOX,
    ]
    .contains(&msg)
    {
        return SendMessageW(GetParent(hwnd).unwrap_or_default(), msg, Some(wp), Some(lp));
    }
    if msg == WM_ERASEBKGND {
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        FillRect(
            HDC(wp.0 as *mut _),
            &rect,
            (*ptr).borrow().colors.background,
        );
        return LRESULT(1);
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}
unsafe fn layout(hwnd: HWND, ptr: *const RefCell<View>) {
    let (controls, dpi, feedback, viewport, scroll_y) = {
        let v = (*ptr).borrow();
        (
            v.controls.clone(),
            v.dpi,
            v.target.is_some(),
            v.viewport,
            v.scroll_y,
        )
    };
    if controls.len() != 11 {
        return;
    }
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let geometry = crate::organizer_ui::memory_layout(rect.right, rect.bottom, dpi, feedback);
    let _ = MoveWindow(viewport, 0, 0, rect.right, geometry.viewport_height, true);
    let scroll_y = crate::organizer_ui::clamp_scroll(
        scroll_y,
        geometry.content_height,
        geometry.viewport_height,
    );
    for (index, r) in geometry.fields.iter().enumerate() {
        let _ = ShowWindow(
            controls[index],
            if r.height == 0 { SW_HIDE } else { SW_SHOW },
        );
    }
    for (index, r) in geometry.buttons.iter().enumerate() {
        let _ = MoveWindow(controls[index + 7], r.x, r.y, r.width, r.height, true);
    }
    if feedback {
        let _ = ShowWindow(controls[9], SW_HIDE);
    }
    {
        let mut v = (*ptr).borrow_mut();
        v.geometry = Some(geometry);
        v.scroll_y = scroll_y;
    }
    scroll_content(ptr, scroll_y);
    reveal_control(ptr, GetFocus());
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<View>;
    if !ptr.is_null() {
        match msg {
            WM_COMMAND => {
                let id = wp.0 & 0xffff;
                if id == 1 && wp.0 >> 16 == LBN_SELCHANGE as usize {
                    selected(ptr);
                } else if (id == 4 && wp.0 >> 16 == CBN_SELCHANGE as usize)
                    || (id == 3 && wp.0 >> 16 == EN_CHANGE as usize)
                {
                    scope_preview(ptr);
                } else if (7..=10).contains(&id) {
                    action(ptr, id);
                }
                render(ptr);
                return LRESULT(0);
            }
            WM_TIMER => {
                render(ptr);
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                let viewport = (*ptr).borrow().viewport;
                return SendMessageW(viewport, msg, Some(wp), Some(lp));
            }
            WM_SIZE => {
                layout(hwnd, ptr);
                return LRESULT(0);
            }
            WM_GETMINMAXINFO => {
                let dpi = (*ptr).borrow().dpi as i32;
                let mut monitor = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(
                    MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                    &mut monitor,
                );
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT {
                    x: (320 * dpi / 96)
                        .min((monitor.rcWork.right - monitor.rcWork.left - 24).max(1)),
                    y: (280 * dpi / 96)
                        .min((monitor.rcWork.bottom - monitor.rcWork.top - 24).max(1)),
                };
                return LRESULT(0);
            }
            WM_CLOSE => {
                (*ptr).borrow_mut().done = true;
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
                let v = (*ptr).borrow();
                let dc = HDC(wp.0 as *mut _);
                SetTextColor(dc, v.colors.palette.text);
                SetBkColor(dc, v.colors.palette.background);
                return LRESULT(v.colors.background.0 as isize);
            }
            WM_ERASEBKGND => {
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                FillRect(HDC(wp.0 as *mut _), &r, (*ptr).borrow().colors.background);
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
                    v.scroll_y = v.scroll_y * dpi as i32 / v.dpi as i32;
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
                let _ = InvalidateRect(Some(hwnd), None, true);
                return LRESULT(0);
            }
            _ => (),
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}
pub unsafe fn show(
    owner: HWND,
    state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>,
    appearance: Theme,
) -> bool {
    show_form(owner, state, submit, appearance, None)
}
pub unsafe fn feedback(
    owner: HWND,
    state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>,
    appearance: Theme,
    target: (String, FeedbackTarget),
) -> bool {
    show_form(owner, state, submit, appearance, Some(target))
}
unsafe fn show_form(
    owner: HWND,
    state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>,
    appearance: Theme,
    target: Option<(String, FeedbackTarget)>,
) -> bool {
    {
        let command = state.borrow_mut().request_snapshot();
        if let Some(command) = command {
            submit(command);
        }
    }
    let Ok(instance) = GetModuleHandleW(None) else {
        return false;
    };
    let class = w!("QuickPanelHarnessMemory");
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance.into(),
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    });
    let dpi = GetDpiForWindow(owner).max(96);
    let s = |v| v * dpi as i32 / 96;
    let mut monitor = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let _ = GetMonitorInfoW(
        MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
        &mut monitor,
    );
    let width = s(700).min(monitor.rcWork.right - monitor.rcWork.left - 24);
    let height = s(640).min(monitor.rcWork.bottom - monitor.rcWork.top - 24);
    let view = Box::new(RefCell::new(View {
        state: state.clone(),
        submit,
        target: target.clone(),
        shown: None,
        records: vec![],
        availability: None,
        controls: vec![],
        colors: theme::Colors::new(appearance),
        appearance,
        font: theme::font(dpi),
        dpi,
        done: false,
        open_settings: false,
        revision: u64::MAX,
        viewport: HWND::default(),
        scroll_y: 0,
        geometry: None,
        input_policy: Default::default(),
    }));
    let title = if target.is_some() {
        w!("反馈并重做")
    } else {
        w!("AI 整理记忆")
    };
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_DLGMODALFRAME,
        class,
        title,
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MAXIMIZEBOX,
        monitor.rcWork.left + (monitor.rcWork.right - monitor.rcWork.left - width) / 2,
        monitor.rcWork.top + (monitor.rcWork.bottom - monitor.rcWork.top - height) / 2,
        width,
        height,
        Some(owner),
        None,
        Some(instance.into()),
        Some((&*view as *const RefCell<View>).cast()),
    ) else {
        let _ = DeleteObject(view.borrow().font.into());
        return false;
    };
    let Ok(viewport) = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        w!("STATIC"),
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_CLIPCHILDREN,
        0,
        0,
        1,
        1,
        Some(hwnd),
        None,
        None,
        None,
    ) else {
        let _ = DestroyWindow(hwnd);
        let _ = DeleteObject(view.borrow().font.into());
        return false;
    };
    view.borrow_mut().viewport = viewport;
    if !SetWindowSubclass(
        viewport,
        Some(viewport_proc),
        1,
        (&*view as *const RefCell<View>) as usize,
    )
    .as_bool()
    {
        let _ = DestroyWindow(hwnd);
        let _ = DeleteObject(view.borrow().font.into());
        return false;
    }
    let create = |class: PCWSTR, label: &str, style: WINDOW_STYLE, id: usize| {
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
            Some(if id < 7 { viewport } else { hwnd }),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )
        .unwrap_or_default()
    };
    let controls = vec![
        create(
            w!("EDIT"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_READONLY) as u32),
            0,
        ),
        create(
            w!("LISTBOX"),
            "",
            WS_TABSTOP | WS_VSCROLL | WS_HSCROLL | WINDOW_STYLE(LBS_NOTIFY as u32),
            1,
        ),
        create(
            w!("EDIT"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_READONLY) as u32),
            2,
        ),
        create(
            w!("EDIT"),
            "",
            WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL) as u32),
            3,
        ),
        create(
            w!("COMBOBOX"),
            "",
            WS_TABSTOP | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            4,
        ),
        if target.is_none() {
            create(
                w!("BUTTON"),
                "启用这条记忆",
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                5,
            )
        } else {
            create(w!("STATIC"), "", WINDOW_STYLE::default(), 5)
        },
        if target.is_none() {
            create(
                w!("BUTTON"),
                "我确认此规则及所选范围",
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                6,
            )
        } else {
            create(w!("STATIC"), "", WINDOW_STYLE::default(), 6)
        },
        create(
            w!("BUTTON"),
            if target.is_some() {
                "保存反馈并重做"
            } else {
                "保存所选规则"
            },
            WS_TABSTOP,
            7,
        ),
        create(w!("BUTTON"), "重试保存", WS_TABSTOP, 8),
        create(w!("BUTTON"), "删除", WS_TABSTOP, 9),
        create(w!("BUTTON"), "关闭", WS_TABSTOP, 10),
    ];
    for label in ["仅当前内容", "同目录", "全局（明确选择）"] {
        let label = wide(label);
        SendMessageW(
            controls[4],
            CB_ADDSTRING,
            None,
            Some(LPARAM(label.as_ptr() as isize)),
        );
    }
    SendMessageW(controls[4], CB_SETCURSEL, Some(WPARAM(0)), None);
    // Win32 counts UTF-16 units, while validation counts Unicode scalars.
    let input_limit = if target.is_some() {
        FEEDBACK_REASON_MAX_CHARS
    } else {
        512
    };
    SendMessageW(
        controls[3],
        EM_SETLIMITTEXT,
        Some(WPARAM(input_limit * 2)),
        None,
    );
    if let Some((title, target)) = &target {
        set(
            controls[2],
            &crate::organizer_ui::feedback_intro(title, target),
        );
        let (pending, raw) = {
            let s = state.borrow();
            (s.pending.clone(), s.raw_input.clone())
        };
        if let Some(Command::HarnessFeedback {
            sources,
            reason,
            scope,
            ..
        }) = &pending
            && sources == &vec![target.clone()]
        {
            set(controls[3], if raw.is_empty() { reason } else { &raw });
            SendMessageW(
                controls[4],
                CB_SETCURSEL,
                Some(WPARAM(match scope {
                    FeedbackScope::Item => 0,
                    FeedbackScope::Directory => 1,
                    FeedbackScope::Global => 2,
                })),
                None,
            );
        }
    }
    if target.is_none() {
        let (pending, raw, snapshot) = {
            let memory = state.borrow();
            (
                memory.pending.clone(),
                memory.raw_input.clone(),
                memory.snapshot.clone(),
            )
        };
        if let Some(Command::HarnessMemoryEdit { input, .. }) = pending {
            view.borrow_mut().shown = snapshot
                .records
                .iter()
                .find(|m| Some(m.id) == input.id)
                .cloned();
            set(
                controls[2],
                &format!(
                    "{}\r\n尚未确认落盘的编辑已保留",
                    crate::organizer_ui::memory_scope_label(&input.scope, &snapshot)
                ),
            );
            set(controls[3], if raw.is_empty() { &input.text } else { &raw });
            let choice = match input.scope {
                MemoryScope::Item { .. } => 0,
                MemoryScope::Directory { .. } => 1,
                MemoryScope::Global => 2,
            };
            SendMessageW(controls[4], CB_SETCURSEL, Some(WPARAM(choice)), None);
            SendMessageW(
                controls[5],
                BM_SETCHECK,
                Some(WPARAM(usize::from(input.enabled))),
                None,
            );
            SendMessageW(
                controls[6],
                BM_SETCHECK,
                Some(WPARAM(usize::from(input.confirmed))),
                None,
            );
        }
    }
    let draft = state
        .borrow()
        .navigation_draft_for(target.as_ref().map(|(_, target)| target));
    if let Some(draft) = draft {
        view.borrow_mut().shown = draft.record;
        set(controls[3], &draft.raw_input);
        SendMessageW(
            controls[4],
            CB_SETCURSEL,
            Some(WPARAM(draft.scope_choice)),
            None,
        );
        if target.is_none() {
            SendMessageW(
                controls[5],
                BM_SETCHECK,
                Some(WPARAM(usize::from(draft.enabled))),
                None,
            );
            SendMessageW(
                controls[6],
                BM_SETCHECK,
                Some(WPARAM(usize::from(draft.confirmed))),
                None,
            );
        }
    }
    let font = view.borrow().font;
    for c in &controls {
        SendMessageW(*c, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
    }
    let feedback_available = state.borrow().snapshot.feedback_available();
    let focus = if target.is_some() && !feedback_available {
        controls[8]
    } else if target.is_some() {
        controls[3]
    } else {
        controls[1]
    };
    for control in controls.iter().take(7) {
        let _ = SetWindowSubclass(
            *control,
            Some(focus_proc),
            2,
            (&*view as *const RefCell<View>) as usize,
        );
    }
    view.borrow_mut().controls = controls;
    let dark = view.borrow().colors.dark;
    theme::apply(hwnd, dark);
    layout(hwnd, &*view);
    render(&*view);
    let _ = EnableWindow(owner, false);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetFocus(Some(focus));
    SetTimer(Some(hwnd), 1, 100, None);
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
            let active = windows::Win32::UI::Input::Ime::ImmGetCompositionStringW(
                context,
                windows::Win32::UI::Input::Ime::GCS_COMPSTR,
                None,
                0,
            ) > 0;
            let _ = windows::Win32::UI::Input::Ime::ImmReleaseContext(msg.hwnd, context);
            active
        } else {
            false
        };
        let dialog_keys = view.borrow().input_policy.dialog_keys(composing);
        if msg.message == WM_KEYDOWN && msg.wParam.0 == 27 && dialog_keys {
            view.borrow_mut().done = true;
            continue;
        }
        if !dialog_keys || !IsDialogMessageW(hwnd, &msg).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let _ = KillTimer(Some(hwnd), 1);
    let _ = DestroyWindow(hwnd);
    let _ = DeleteObject(view.borrow().font.into());
    let _ = EnableWindow(owner, true);
    let _ = SetForegroundWindow(owner);
    if let Some(code) = quit {
        PostQuitMessage(code);
    }
    view.borrow().open_settings
}
