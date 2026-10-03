//! Small native dialogs keep every editable field in a real Win32 Edit control.
use super::theme;
use std::cell::RefCell;
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, LibraryLoader::GetModuleHandleW},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub unsafe fn text(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut b = vec![0u16; n + 1];
        let n = GetWindowTextW(hwnd, &mut b);
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }
}
pub unsafe fn pick(owner: HWND, folder: bool) -> Result<Option<String>, String> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
        let mut options = FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_FILEMUSTEXIST;
        if folder {
            options |= FOS_PICKFOLDERS;
        }
        dialog.SetOptions(options).map_err(|e| e.to_string())?;
        if dialog.Show(Some(owner)).is_err() {
            return Ok(None);
        }
        let item = dialog.GetResult().map_err(|e| e.to_string())?;
        let name = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|e| e.to_string())?;
        let result = name.to_string().map_err(|e| e.to_string());
        CoTaskMemFree(Some(name.0.cast()));
        result.map(Some)
    }
}
enum Field {
    Text(String),
    Choice(usize),
    Toggle(bool),
}
struct FormWindow(HWND);
impl Drop for FormWindow {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.0)).as_bool() {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
struct Form {
    edits: Vec<HWND>,
    done: bool,
    accepted: bool,
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Form>;
        if !ptr.is_null() {
            match msg {
                WM_COMMAND if (wp.0 & 0xffff) == 1 || (wp.0 & 0xffff) == 2 => {
                    let mut f = (*ptr).borrow_mut();
                    f.accepted = (wp.0 & 0xffff) == 1;
                    f.done = true;
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    (*ptr).borrow_mut().done = true;
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
/// A bounded modal editor. It dispatches the application's messages while waiting.
pub unsafe fn form(owner: HWND, title: &str, fields: &[(&str, String)]) -> Option<Vec<String>> {
    let fields: Vec<_> = fields
        .iter()
        .map(|(label, value)| (*label, Field::Text(value.clone())))
        .collect();
    unsafe { form_fields(owner, title, &fields) }
}
pub unsafe fn settings_form(
    owner: HWND,
    theme: usize,
    hotkey: String,
    clipboard_hotkey: String,
    paused: bool,
    excluded: String,
) -> Option<Vec<String>> {
    unsafe {
        form_fields(
            owner,
            "设置",
            &[
                ("外观主题", Field::Choice(theme)),
                ("呼出快捷键（留空关闭）", Field::Text(hotkey)),
                (
                    "剪贴板专用快捷键（留空关闭）",
                    Field::Text(clipboard_hotkey),
                ),
                ("记录状态", Field::Toggle(paused)),
                ("排除应用（exe 名称，逗号分隔）", Field::Text(excluded)),
            ],
        )
    }
}
unsafe fn form_fields(owner: HWND, title: &str, fields: &[(&str, Field)]) -> Option<Vec<String>> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = w!("QuickPanelForm");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut _),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let state = Box::new(RefCell::new(Form {
            edits: Vec::new(),
            done: false,
            accepted: false,
        }));
        let title = wide(title);
        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(owner).max(96);
        let scale = |v: i32| v * (dpi as i32) / 96;
        let mut rect = RECT::default();
        let _ = GetWindowRect(owner, &mut rect);
        let height = scale(104 + fields.len() as i32 * 62);
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class,
            PCWSTR(title.as_ptr()),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            rect.left + scale(72),
            rect.top + scale(30),
            scale(520),
            height,
            Some(owner),
            None,
            Some(instance.into()),
            Some((&*state as *const RefCell<Form>).cast()),
        )
        .ok()?;
        let _window_guard = FormWindow(hwnd);
        let font = theme::font(dpi);
        for (i, (label, value)) in fields.iter().enumerate() {
            let label = wide(label);
            let value = wide(match value {
                Field::Text(t) => t,
                Field::Choice(_) => "",
                Field::Toggle(_) => "暂停剪贴板记录",
            });
            let y = scale(12 + i as i32 * 62);
            let l = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                PCWSTR(label.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                scale(18),
                y,
                scale(470),
                scale(22),
                Some(hwnd),
                None,
                None,
                None,
            )
            .ok()?;
            let e = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                match fields[i].1 {
                    Field::Text(_) => w!("EDIT"),
                    Field::Choice(_) => w!("COMBOBOX"),
                    Field::Toggle(_) => w!("BUTTON"),
                },
                PCWSTR(value.as_ptr()),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WINDOW_STYLE(match fields[i].1 {
                        Field::Text(_) => ES_AUTOHSCROLL as u32,
                        Field::Choice(_) => CBS_DROPDOWNLIST as u32,
                        Field::Toggle(_) => BS_AUTOCHECKBOX as u32,
                    }),
                scale(18),
                y + scale(24),
                scale(470),
                scale(if matches!(fields[i].1, Field::Choice(_)) {
                    150
                } else {
                    28
                }),
                Some(hwnd),
                None,
                None,
                None,
            )
            .ok()?;
            SendMessageW(l, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            SendMessageW(e, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            match fields[i].1 {
                Field::Choice(selected) => {
                    for choice in ["跟随系统", "浅色", "深色"] {
                        let choice = wide(choice);
                        SendMessageW(
                            e,
                            CB_ADDSTRING,
                            None,
                            Some(LPARAM(choice.as_ptr() as isize)),
                        );
                    }
                    SendMessageW(e, CB_SETCURSEL, Some(WPARAM(selected)), None);
                }
                Field::Toggle(checked) => {
                    SendMessageW(e, BM_SETCHECK, Some(WPARAM(usize::from(checked))), None);
                }
                Field::Text(_) => {}
            }
            state.borrow_mut().edits.push(e);
        }
        for (id, label, x) in [(1, "保存", 300), (2, "取消", 398)] {
            let s = wide(label);
            let b = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                PCWSTR(s.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                scale(x),
                height - scale(80),
                scale(90),
                scale(30),
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )
            .ok()?;
            SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
        }
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let first_edit = state.borrow().edits.first().copied();
        if let Some(e) = first_edit {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(e));
        }
        let mut msg = MSG::default();
        let mut quit = None;
        loop {
            if state.borrow().done {
                break;
            }
            let status = GetMessageW(&mut msg, None, 0, 0);
            if status.0 <= 0 {
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
                state.borrow_mut().done = true;
                continue;
            }
            if composing || !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let (accepted, edits) = {
            let state = state.borrow();
            (state.accepted, state.edits.clone())
        };
        let result = accepted.then(|| {
            edits
                .iter()
                .enumerate()
                .map(|(i, e)| match fields[i].1 {
                    Field::Text(_) => text(*e),
                    Field::Choice(_) => SendMessageW(*e, CB_GETCURSEL, None, None).0.to_string(),
                    Field::Toggle(_) => SendMessageW(*e, BM_GETCHECK, None, None).0.to_string(),
                })
                .collect()
        });
        let _ = DestroyWindow(hwnd);
        let _ = DeleteObject(font.into());
        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        if let Some(code) = quit {
            PostQuitMessage(code);
        }
        result
    }
}
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, DeleteObject, HBRUSH};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
