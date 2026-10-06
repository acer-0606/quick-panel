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
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
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
    controls: Vec<HWND>,
    colors: theme::Colors,
    appearance: Theme,
    font: HFONT,
    dpi: u32,
    done: bool,
    revision: u64,
}
unsafe fn set(hwnd: HWND, value: &str) {
    if text(hwnd) != value {
        let s = wide(value);
        let _ = SetWindowTextW(hwnd, PCWSTR(s.as_ptr()));
    }
}
fn scope_label(scope: &MemoryScope, snapshot: &MemorySnapshot) -> String {
    match scope {
        MemoryScope::Global => "全局".into(),
        MemoryScope::Directory { path } => format!("目录：{path}"),
        MemoryScope::Item { source } => format!(
            "仅此内容：{}",
            snapshot
                .source_labels
                .iter()
                .find(|(s, _)| s == source)
                .map(|(_, t)| t.as_str())
                .unwrap_or("原内容已移除")
        ),
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
        let evidence = record
            .evidence_ids
            .iter()
            .map(|id| {
                id.strip_prefix("memory:")
                    .and_then(|s| s.parse::<u64>().ok())
                    .and_then(|id| snapshot.records.iter().find(|m| m.id == id))
                    .map(|m| m.text.clone())
                    .unwrap_or_else(|| "依据已移除".into())
            })
            .collect::<Vec<_>>()
            .join("；");
        set(
            controls[2],
            &format!(
                "{}\r\n来源：{}{}\r\n依据：{}",
                scope_label(&record.scope, &snapshot),
                if record.origin == MemoryOrigin::Human {
                    "人工"
                } else {
                    "AI"
                },
                if record.proposed_by_ai {
                    "（最初由 AI 提出）"
                } else {
                    ""
                },
                if evidence.is_empty() {
                    "本地整理与反馈"
                } else {
                    &evidence
                }
            ),
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
    set(
        controls[0],
        snapshot
            .storage_error
            .as_deref()
            .unwrap_or(if message.is_empty() {
                "保存时将换行转为空格；范围与确认状态按您的选择保存。"
            } else {
                &message
            }),
    );
    if !feedback && (*ptr).borrow().records != snapshot.records && !saving && !pending_save {
        let selected_id = (*ptr).borrow().shown.as_ref().map(|m| m.id);
        let top = SendMessageW(controls[1], LB_GETTOPINDEX, None, None);
        SendMessageW(controls[1], WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(controls[1], LB_RESETCONTENT, None, None);
        for record in &snapshot.records {
            let label = wide(&format!(
                "{}  ·  {}  ·  {}  ·  {}",
                scope_label(&record.scope, &snapshot),
                if record.origin == MemoryOrigin::Human {
                    "人工"
                } else {
                    "AI"
                },
                if !record.enabled {
                    "已停用"
                } else if record.confirmed {
                    "已确认"
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
        (*ptr).borrow_mut().records = snapshot.records;
    }
    for c in &controls[3..8] {
        let _ = EnableWindow(*c, !saving && !pending_save);
    }
    let _ = EnableWindow(controls[8], !saving && state.borrow().pending.is_some());
    let _ = EnableWindow(controls[9], !saving && !pending_save && !feedback);
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
        if let Some(command) = state.borrow_mut().retry() {
            submit(command);
        }
        return;
    }
    if state.borrow().saving || state.borrow().pending_save {
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
        let value = crate::organizer_ui::memory_text(&raw);
        state.borrow_mut().raw_input = raw;
        if value.is_empty() || value.chars().count() > 512 {
            state.borrow_mut().message = "请填写 1–512 字符的原因或规则".into();
            state.borrow_mut().revision += 1;
            return;
        }
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
    match state.borrow_mut().submit(command) {
        Ok(command) => submit(command),
        Err(message) => {
            set(controls[0], &message);
        }
    }
}
unsafe fn layout(hwnd: HWND, ptr: *const RefCell<View>) {
    let (controls, dpi, feedback) = {
        let v = (*ptr).borrow();
        (v.controls.clone(), v.dpi, v.target.is_some())
    };
    if controls.len() != 11 {
        return;
    }
    let mut r = RECT::default();
    let _ = GetClientRect(hwnd, &mut r);
    let s = |v| v * dpi as i32 / 96;
    let w = (r.right - s(28)).max(1);
    let footer = r.bottom - s(46);
    let middle = (footer - s(148)).max(s(130));
    let list_h = if feedback {
        0
    } else {
        (middle / 2 - s(24)).max(s(50))
    };
    let detail_y = s(62) + list_h;
    let editor_y = detail_y + s(66);
    let editor_h = (middle + s(48) - editor_y).max(s(54));
    let pos = [
        (14, 10, w, s(44)),
        (14, 62, w, list_h),
        (14, detail_y, w, s(60)),
        (14, editor_y, w, editor_h),
        (14, footer - s(70), s(240), s(160)),
        (270, footer - s(68), s(140), s(26)),
        (270, footer - s(36), s(250), s(26)),
        (14, footer, s(148), s(32)),
        (172, footer, s(104), s(32)),
        (286, footer, s(80), s(32)),
        (0, footer, s(80), s(32)),
    ];
    for (i, (x, y, width, height)) in pos.into_iter().enumerate() {
        let x = if i == 10 { r.right - s(94) } else { s(x) };
        let y = if i < 2 { s(y) } else { y };
        let _ = MoveWindow(controls[i], x, y, width, height.max(1), true);
    }
    if feedback {
        let _ = ShowWindow(controls[1], SW_HIDE);
        let _ = ShowWindow(controls[5], SW_HIDE);
        let _ = ShowWindow(controls[6], SW_HIDE);
        let _ = ShowWindow(controls[9], SW_HIDE);
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
            WM_COMMAND => {
                let id = wp.0 & 0xffff;
                if id == 1 && wp.0 >> 16 == LBN_SELCHANGE as usize {
                    selected(ptr);
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
            WM_SIZE => {
                layout(hwnd, ptr);
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
                theme::apply(hwnd, (*ptr).borrow().colors.dark);
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
) {
    show_form(owner, state, submit, appearance, None);
}
pub unsafe fn feedback(
    owner: HWND,
    state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>,
    appearance: Theme,
    target: (String, FeedbackTarget),
) {
    show_form(owner, state, submit, appearance, Some(target));
}
unsafe fn show_form(
    owner: HWND,
    state: Rc<RefCell<MemoryUiState>>,
    submit: Rc<dyn Fn(Command)>,
    appearance: Theme,
    target: Option<(String, FeedbackTarget)>,
) {
    if target.is_none() {
        let command = state.borrow_mut().request_snapshot();
        if let Some(command) = command {
            submit(command);
        }
    }
    let Ok(instance) = GetModuleHandleW(None) else {
        return;
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
        controls: vec![],
        colors: theme::Colors::new(appearance),
        appearance,
        font: theme::font(dpi),
        dpi,
        done: false,
        revision: u64::MAX,
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
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
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
        return;
    };
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
            Some(hwnd),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )
        .unwrap_or_default()
    };
    let controls = vec![
        create(w!("STATIC"), "", WINDOW_STYLE::default(), 0),
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
        create(
            w!("BUTTON"),
            "启用这条记忆",
            WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            5,
        ),
        create(
            w!("BUTTON"),
            "我确认此规则及所选范围",
            WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
            6,
        ),
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
    SendMessageW(controls[3], EM_SETLIMITTEXT, Some(WPARAM(1024)), None);
    if let Some((title, target)) = &target {
        set(
            controls[2],
            &format!(
                "原名称：{title}\r\n所见建议：{}\r\n请说明拒绝原因；需先启用“记住整理反馈”。",
                target.rejected_alias.as_deref().unwrap_or("暂无建议")
            ),
        );
        if let Some(Command::HarnessFeedback {
            sources,
            reason,
            scope,
            ..
        }) = &state.borrow().pending
            && sources == &vec![target.clone()]
        {
            let raw = state.borrow().raw_input.clone();
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
                    scope_label(&input.scope, &snapshot)
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
    for c in &controls {
        SendMessageW(
            *c,
            WM_SETFONT,
            Some(WPARAM(view.borrow().font.0 as usize)),
            None,
        );
    }
    let focus = if target.is_some() {
        controls[3]
    } else {
        controls[1]
    };
    view.borrow_mut().controls = controls;
    theme::apply(hwnd, view.borrow().colors.dark);
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
        if msg.message == WM_KEYDOWN && msg.wParam.0 == 27 && !composing {
            view.borrow_mut().done = true;
            continue;
        }
        if composing || !IsDialogMessageW(hwnd, &msg).as_bool() {
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
}
