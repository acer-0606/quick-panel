//! Small native dialogs keep every editable field in a real Win32 Edit control.
use super::{ai_client, theme};
use crate::connection_test::{ConnectionTestGate, ConnectionTestWorker};
use crate::form_validation::{FormError, FormStatus, Submission, Validation};
use crate::model::{ItemSummary, SmartSearchMode, Theme};
use std::{cell::RefCell, sync::Arc};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        System::{Com::*, LibraryLoader::GetModuleHandleW},
        UI::{Controls::EM_SETLIMITTEXT, Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
/// Called before teardown, with no Form borrow held. Errors retain the native draft.
/// Successful callback callers must not apply the returned values again; password slots
/// in that acknowledgement are blank, while a callback-free form retains the legacy result.
type Submit<'a> = Option<&'a mut dyn FnMut(&[String]) -> Result<(), FormError>>;
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
pub unsafe fn path_settings_form(
    owner: HWND,
    settings: &crate::path_activity::PathActivitySettings,
    appearance: Theme,
) -> Option<Vec<String>> {
    unsafe {
        form_fields_with_test_theme(
            owner,
            "常用推荐设置",
            &[
                (
                    "外部资源管理器的常用路径（本机排名）",
                    Field::Switch(settings.enabled, "启用常用推荐（默认关闭）"),
                ),
                (
                    "Recent 仅补充变更目标，不回放已有链接",
                    Field::Switch(settings.recent_documents, "同时观察最近文档"),
                ),
                (
                    "排除目录（每行一条绝对路径，最多 128 条）",
                    Field::Multiline(settings.excluded_paths.join("\r\n")),
                ),
                (
                    "清空后，后续的新访问可重新记录并推荐",
                    Field::Switch(false, "清空访问统计及不再推荐名单"),
                ),
            ],
            None,
            appearance,
            Validation::Paths,
            None,
        )
    }
}

pub unsafe fn screenshot_settings_form(
    owner: HWND,
    hotkey: String,
    appearance: Theme,
    submit: Submit<'_>,
) -> Option<Vec<String>> {
    unsafe {
        form_fields_with_test_theme(
            owner,
            "截图快捷键设置",
            &[("区域截图快捷键（默认关闭；留空关闭）", Field::Text(hotkey))],
            None,
            appearance,
            Validation::Screenshot,
            submit,
        )
    }
}
enum Field {
    Model(String),
    EffortChoice(usize),
    OrganizerModeChoice(usize),
    OrganizerScopeChoice(usize),
    Text(String),
    Multiline(String),
    Password(String),
    Choice(usize),
    AuthChoice(usize),
    IndexChoice(usize),
    SmartChoice(usize),
    Toggle(bool),
    Switch(bool, &'static str),
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
    controls: Vec<HWND>,
    colors: theme::Colors,
    appearance: Theme,
    font: HFONT,
    dpi: u32,
    done: bool,
    accepted: bool,
    test: Option<TestForm>,
    validation: Validation,
    status: FormStatus,
    labels: Vec<String>,
    health_requested: bool,
    test_gate: ConnectionTestGate,
    test_button: Option<HWND>,
    test_result: Option<HWND>,
    error_result: Option<HWND>,
    save_button: Option<HWND>,
    model_button: Option<HWND>,
    models: Vec<crate::ai_models::ModelInfo>,
    choices: Vec<usize>,
    switches: Vec<usize>,
    viewport: HWND,
    scroll_y: i32,
    content_height: i32,
}

#[derive(Clone)]
enum TestForm {
    Ai(crate::ai::AiSettings),
    Embedding(crate::semantic::EmbeddingSettings),
    Organizer {
        ai: crate::ai::AiSettings,
        path_enabled: bool,
        owner: usize,
        health: Arc<std::sync::Mutex<String>>,
    },
}
struct TestRequest {
    config: TestForm,
    key: ai_client::SecretString,
    clear_key: bool,
    list_models: bool,
}
enum TestSuccess {
    Ai,
    Embedding(usize),
    Models(Vec<crate::ai_models::ModelInfo>),
}
impl Drop for Form {
    fn drop(&mut self) {
        if !self.font.is_invalid() {
            unsafe {
                let _ = DeleteObject(self.font.into());
            }
        }
    }
}
const TEST_BUTTON: usize = 3;
const TEST_TIMER: usize = 1;
const MODEL_BUTTON: usize = 4;
unsafe fn scroll_form(ptr: *const RefCell<Form>, position: i32) {
    unsafe {
        let (viewport, old, content) = {
            let form = (*ptr).borrow();
            (form.viewport, form.scroll_y, form.content_height)
        };
        let mut rect = RECT::default();
        let _ = GetClientRect(viewport, &mut rect);
        let position = crate::organizer_ui::clamp_scroll(position, content, rect.bottom);
        (*ptr).borrow_mut().scroll_y = position;
        let info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: content - 1,
            nPage: rect.bottom.max(1) as u32,
            nPos: position,
            ..Default::default()
        };
        windows::Win32::UI::Controls::SetScrollInfo(viewport, SB_VERT, &info, true);
        ScrollWindowEx(
            viewport,
            0,
            old - position,
            None,
            None,
            None,
            None,
            SW_SCROLLCHILDREN | SW_INVALIDATE,
        );
    }
}
unsafe extern "system" fn viewport_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        let ptr = data as *const RefCell<Form>;
        if msg == WM_COMMAND {
            let owner = GetParent(hwnd).unwrap_or_default();
            let notification = (wp.0 >> 16) as u32;
            if [EN_SETFOCUS, CBN_SETFOCUS, BN_SETFOCUS].contains(&notification) {
                let edit = HWND(lp.0 as *mut _);
                let mut rect = RECT::default();
                let _ = GetWindowRect(edit, &mut rect);
                let mut points = [
                    POINT {
                        x: rect.left,
                        y: rect.top,
                    },
                    POINT {
                        x: rect.right,
                        y: rect.bottom,
                    },
                ];
                windows::Win32::Graphics::Gdi::MapWindowPoints(None, Some(hwnd), &mut points);
                let mut viewport = RECT::default();
                let _ = GetClientRect(hwnd, &mut viewport);
                let old = (*ptr).borrow().scroll_y;
                if points[0].y < 0 {
                    scroll_form(ptr, old + points[0].y);
                } else if points[1].y > viewport.bottom {
                    scroll_form(ptr, old + points[1].y - viewport.bottom);
                }
            }
            return SendMessageW(owner, msg, Some(wp), Some(lp));
        }
        if msg == WM_VSCROLL || msg == WM_MOUSEWHEEL {
            let (position, dpi) = {
                let f = (*ptr).borrow();
                (f.scroll_y, f.dpi)
            };
            let step = 24 * dpi as i32 / 96;
            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let command = (wp.0 & 0xffff) as u32;
            let next = if msg == WM_MOUSEWHEEL {
                position - ((wp.0 >> 16) as u16 as i16 as i32) / 120 * step * 3
            } else {
                match command {
                    0 => position - step,
                    1 => position + step,
                    2 => position - rect.bottom,
                    3 => position + rect.bottom,
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
            scroll_form(ptr, next);
            return LRESULT(0);
        }
        if [WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_CTLCOLORLISTBOX].contains(&msg) {
            let owner = GetParent(hwnd).unwrap_or_default();
            return SendMessageW(owner, msg, Some(wp), Some(lp));
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}
thread_local! {
    // Shared across forms so closing one does not allow a second concurrent paid call.
    // The worker creates its first thread only after an explicit click.
    static TEST_WORKER: ConnectionTestWorker<TestRequest, TestSuccess> = crate::organizer_ui::request_worker(
        |request: TestRequest| match request.config {
            TestForm::Ai(settings) if request.list_models => ai_client::list_models_with_key(&settings, request.key, request.clear_key).map(TestSuccess::Models),
            TestForm::Ai(settings) => ai_client::test_ai_connection_with_key(
                &settings, request.key, request.clear_key,
            ).map(|()| TestSuccess::Ai),
            TestForm::Embedding(settings) => ai_client::test_embedding_connection_with_key(
                &settings, request.key, request.clear_key,
            ).map(TestSuccess::Embedding),
            TestForm::Organizer { .. } => Err("管家设置无需测试请求".into()),
        },
        Arc::new(|| {}),
    );
}

unsafe fn render_status(ptr: *const RefCell<Form>) {
    let (output, message, error_output, error) = unsafe {
        let form = (*ptr).borrow();
        (
            form.test_result,
            form.status.text(),
            form.error_result,
            form.status
                .error
                .as_ref()
                .map(|e| format!("未保存：{}", e.message))
                .unwrap_or_default(),
        )
    };
    if let Some(output) = error_output {
        let error = wide(&error);
        unsafe {
            let _ = SetWindowTextW(output, PCWSTR(error.as_ptr()));
        }
    }
    if let Some(output) = output {
        let message = wide(&message);
        unsafe {
            let _ = SetWindowTextW(output, PCWSTR(message.as_ptr()));
        }
    }
}
unsafe fn test_status(ptr: *const RefCell<Form>, message: &str) {
    unsafe {
        (*ptr)
            .borrow_mut()
            .status
            .runtime_health(message.to_owned());
        render_status(ptr);
    }
}
unsafe fn form_error(ptr: *const RefCell<Form>, mut error: FormError) {
    let edit = unsafe {
        let mut form = (*ptr).borrow_mut();
        if let Some(label) = form.labels.get(error.field) {
            error.message = format!("{label}：{}", error.message);
        }
        let edit = form.edits.get(error.field).copied();
        form.status.error = Some(error);
        edit
    };
    unsafe {
        render_status(ptr);
        if let Some(edit) = edit {
            // Checkboxes need not send BN_SETFOCUS; reveal the field explicitly too.
            let (viewport, old) = {
                let form = (*ptr).borrow();
                (form.viewport, form.scroll_y)
            };
            let mut rect = RECT::default();
            let _ = GetWindowRect(edit, &mut rect);
            let mut top = [POINT {
                x: rect.left,
                y: rect.top,
            }];
            windows::Win32::Graphics::Gdi::MapWindowPoints(None, Some(viewport), &mut top);
            let label_height = 24 * (*ptr).borrow().dpi as i32 / 96;
            scroll_form(ptr, old + top[0].y - label_height);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
        }
    }
}
struct FormValues {
    values: Vec<String>,
    passwords: Vec<usize>,
}
impl Drop for FormValues {
    fn drop(&mut self) {
        for &index in &self.passwords {
            if let Some(value) = self.values.get_mut(index) {
                drop(ai_client::SecretString(std::mem::take(value)));
            }
        }
    }
}
unsafe fn read_values(ptr: *const RefCell<Form>) -> FormValues {
    let (edits, choices, switches, password) = unsafe {
        let form = (*ptr).borrow();
        (
            form.edits.clone(),
            form.choices.clone(),
            form.switches.clone(),
            match form.test {
                Some(TestForm::Ai(_)) => Some(4),
                Some(TestForm::Embedding(_)) => Some(3),
                _ => None,
            },
        )
    };
    FormValues {
        values: edits
            .iter()
            .enumerate()
            .map(|(i, edit)| unsafe {
                if choices.contains(&i) {
                    SendMessageW(*edit, CB_GETCURSEL, None, None).0.to_string()
                } else if switches.contains(&i) {
                    SendMessageW(*edit, BM_GETCHECK, None, None).0.to_string()
                } else {
                    text(*edit)
                }
            })
            .collect(),
        passwords: password.into_iter().collect(),
    }
}

unsafe fn test_controls(ptr: *const RefCell<Form>, running: bool) {
    // EnableWindow can dispatch messages synchronously. Never keep a Form borrow across it.
    let (edits, save, test, model) = unsafe {
        let form = (*ptr).borrow();
        (
            form.edits.clone(),
            form.save_button,
            form.test_button,
            form.model_button,
        )
    };
    unsafe {
        for edit in edits {
            let _ = EnableWindow(edit, !running);
        }
        if let Some(save) = save {
            let _ = EnableWindow(save, !running);
        }
        if let Some(test) = test {
            let available = !running && !TEST_WORKER.with(|worker| worker.busy());
            let _ = EnableWindow(test, available);
        }
        if let Some(model) = model {
            let _ = EnableWindow(model, !running && !TEST_WORKER.with(|worker| worker.busy()));
        }
    }
}

unsafe fn cancel_test(ptr: *const RefCell<Form>) {
    let id = unsafe { (*ptr).borrow_mut().test_gate.cancel() };
    if let Some(id) = id {
        TEST_WORKER.with(|worker| worker.cancel(id));
    }
}

unsafe fn begin_test(ptr: *const RefCell<Form>, list_models: bool) {
    let (config, edits) = unsafe {
        let form = (*ptr).borrow();
        if form.done || form.test_gate.busy() {
            return;
        }
        (form.test.clone(), form.edits.clone())
    };
    let Some(mut config) = config else { return };
    let (password_index, clear_index) = match config {
        TestForm::Ai(_) => (4, 5),
        TestForm::Embedding(_) => (3, 4),
        TestForm::Organizer {
            owner, ref health, ..
        } => {
            unsafe {
                (*ptr).borrow_mut().health_requested = true;
            }
            *health.lock().unwrap() = "正在启动 Pi（不调用模型）…".into();
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(owner as *mut _)),
                    super::ui::PI_HEALTH_WAKE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            return;
        }
    };
    // Password text enters an erasing owner immediately; no plain-text result vector is made.
    let key = ai_client::SecretString(unsafe { text(edits[password_index]) });
    let clear_key = unsafe { SendMessageW(edits[clear_index], BM_GETCHECK, None, None).0 == 1 };
    unsafe {
        match &mut config {
            TestForm::Ai(settings) => {
                settings.endpoint = text(edits[1]).trim().into();
                settings.model = text(edits[2]).trim().into();
                settings.auth = if SendMessageW(edits[3], CB_GETCURSEL, None, None).0 == 1 {
                    crate::ai::AuthMethod::Bearer
                } else {
                    crate::ai::AuthMethod::ApiKey
                };
                settings.effort = crate::organizer_ui::EFFORTS
                    .get(SendMessageW(edits[6], CB_GETCURSEL, None, None).0.max(0) as usize)
                    .copied()
                    .unwrap_or_default();
                if !list_models
                    && let Some(message) = crate::organizer_ui::effort_warning(
                        &(*ptr).borrow().models,
                        &settings.model,
                        settings.effort,
                    )
                {
                    test_status(ptr, &message);
                    return;
                }
            }
            TestForm::Embedding(settings) => {
                settings.endpoint = text(edits[1]).trim().into();
                settings.model = text(edits[2]).trim().into();
            }
            TestForm::Organizer { .. } => return,
        }
    }
    let request = TestRequest {
        config,
        key,
        clear_key,
        list_models,
    };
    match TEST_WORKER.with(|worker| worker.submit(request)) {
        Ok(id) => unsafe {
            (*ptr).borrow_mut().test_gate.begin(id);
            test_status(
                ptr,
                if list_models {
                    "正在获取模型… 不发送推理请求"
                } else {
                    "正在测试连接… 仅发送内置测试文本"
                },
            );
            test_controls(ptr, true);
        },
        Err(message) => unsafe { test_status(ptr, &format!("测试失败：{message}")) },
    }
}

unsafe fn poll_test(ptr: *const RefCell<Form>) {
    let health = unsafe {
        let form = (*ptr).borrow();
        match &form.test {
            Some(TestForm::Organizer { health, .. }) => {
                Some((health.clone(), form.health_requested))
            }
            _ => None,
        }
    };
    if let Some((health, requested)) = health {
        if requested {
            unsafe {
                test_status(ptr, &health.lock().unwrap());
            }
        }
        return;
    }

    let done = unsafe { (*ptr).borrow().done };
    if done {
        return;
    }
    if let Some(completion) = TEST_WORKER.with(|worker| worker.try_result()) {
        let adopted = unsafe { (*ptr).borrow_mut().test_gate.finish(completion.id) };
        if adopted {
            let elapsed = completion.elapsed.as_millis();
            let message = match completion.result {
                Ok(TestSuccess::Ai) => format!("连接成功 · 检索接口可用 · {elapsed} ms"),
                Ok(TestSuccess::Embedding(dimensions)) => {
                    format!("连接成功 · 向量维度 {dimensions} · {elapsed} ms")
                }
                Ok(TestSuccess::Models(models)) => unsafe {
                    let edit = (*ptr).borrow().edits[2];
                    let manual = text(edit);
                    SendMessageW(edit, CB_RESETCONTENT, None, None);
                    for model in &models {
                        let id = wide(&model.id);
                        SendMessageW(edit, CB_ADDSTRING, None, Some(LPARAM(id.as_ptr() as isize)));
                    }
                    let manual = wide(&manual);
                    let _ = SetWindowTextW(edit, PCWSTR(manual.as_ptr()));
                    let count = models.len();
                    (*ptr).borrow_mut().models = models;
                    format!("已获取 {count} 个模型；可选择 ID 或手动输入")
                },
                Err(error) => format!("测试失败：{error} · {elapsed} ms"),
            };
            unsafe {
                test_status(ptr, &message);
                test_controls(ptr, false);
            }
        }
    }
    let (running, button, model) = unsafe {
        let form = (*ptr).borrow();
        (form.test_gate.busy(), form.test_button, form.model_button)
    };
    if let Some(button) = button {
        let available = !running && !TEST_WORKER.with(|worker| worker.busy());
        unsafe {
            let _ = EnableWindow(button, available);
        }
    }
    if let Some(button) = model {
        unsafe {
            let _ = EnableWindow(
                button,
                !running && !TEST_WORKER.with(|worker| worker.busy()),
            );
        }
    }
}
unsafe fn validate_form(ptr: *const RefCell<Form>, values: &[String]) -> Result<(), FormError> {
    let (validation, config, models) = unsafe {
        let form = (*ptr).borrow();
        (
            form.validation.clone(),
            form.test.clone(),
            form.models.clone(),
        )
    };
    crate::form_validation::validate(&validation, values, &models)?;
    if let Some(TestForm::Organizer { mut ai, .. }) = config
        && values[0] == "1"
    {
        ai.enabled = true;
        ai_client::validate_settings(&ai).map_err(|e| FormError::new(0, e))?;
    }
    Ok(())
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
                    let accept = (wp.0 & 0xffff) == 1;
                    if accept && (*ptr).borrow().test_gate.busy() {
                        return LRESULT(0);
                    }
                    {
                        let mut f = (*ptr).borrow_mut();
                        f.accepted = accept;
                        f.done = true;
                    }
                    cancel_test(ptr);
                    return LRESULT(0);
                }
                WM_COMMAND if (wp.0 & 0xffff) == TEST_BUTTON => {
                    begin_test(ptr, false);
                    return LRESULT(0);
                }
                WM_COMMAND if (wp.0 & 0xffff) == MODEL_BUTTON => {
                    begin_test(ptr, true);
                    return LRESULT(0);
                }
                WM_COMMAND => {
                    let changed = {
                        let form = (*ptr).borrow();
                        form.test.is_some()
                            && !form.test_gate.busy()
                            && form.edits.iter().any(|edit| edit.0 as isize == lp.0)
                    };
                    let notification = (wp.0 >> 16) as u32;
                    if changed
                        && [EN_CHANGE, CBN_SELCHANGE, CBN_EDITCHANGE, BN_CLICKED]
                            .contains(&notification)
                    {
                        let credential_change = {
                            let form = (*ptr).borrow();
                            matches!(form.test, Some(TestForm::Ai(_)))
                                && [1usize, 3, 4, 5]
                                    .iter()
                                    .any(|i| form.edits[*i].0 as isize == lp.0)
                        };
                        if credential_change {
                            (*ptr).borrow_mut().models.clear();
                        }
                        test_status(ptr, "配置已修改，请重新测试连接");
                    }
                }
                WM_TIMER if wp.0 == TEST_TIMER => {
                    poll_test(ptr);
                    return LRESULT(0);
                }
                WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX => {
                    let (palette, brush) = {
                        let form = (*ptr).borrow();
                        (form.colors.palette, form.colors.background)
                    };
                    let dc = windows::Win32::Graphics::Gdi::HDC(wp.0 as *mut _);
                    windows::Win32::Graphics::Gdi::SetTextColor(dc, palette.text);
                    windows::Win32::Graphics::Gdi::SetBkColor(dc, palette.background);
                    return LRESULT(brush.0 as isize);
                }
                WM_ERASEBKGND => {
                    let brush = (*ptr).borrow().colors.background;
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    windows::Win32::Graphics::Gdi::FillRect(
                        windows::Win32::Graphics::Gdi::HDC(wp.0 as *mut _),
                        &rect,
                        brush,
                    );
                    return LRESULT(1);
                }
                WM_SETTINGCHANGE | WM_THEMECHANGED => {
                    let appearance = (*ptr).borrow().appearance;
                    (*ptr).borrow_mut().colors = theme::Colors::new(appearance);
                    let dark = (*ptr).borrow().colors.dark;
                    theme::apply(hwnd, dark);
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, true);
                    return LRESULT(0);
                }
                WM_DPICHANGED => {
                    let dpi = (wp.0 & 0xffff) as u32;
                    let (old_dpi, controls) = {
                        let form = (*ptr).borrow();
                        (form.dpi, form.controls.clone())
                    };
                    let new_font = theme::font(dpi);
                    for control in controls {
                        let mut rect = RECT::default();
                        let _ = GetWindowRect(control, &mut rect);
                        let mut points = [
                            POINT {
                                x: rect.left,
                                y: rect.top,
                            },
                            POINT {
                                x: rect.right,
                                y: rect.bottom,
                            },
                        ];
                        windows::Win32::Graphics::Gdi::MapWindowPoints(
                            None,
                            GetParent(control).ok(),
                            &mut points,
                        );
                        let scale = |n: i32| n * dpi as i32 / old_dpi.max(1) as i32;
                        let _ = MoveWindow(
                            control,
                            scale(points[0].x),
                            scale(points[0].y),
                            scale(points[1].x - points[0].x),
                            scale(points[1].y - points[0].y),
                            true,
                        );
                        SendMessageW(
                            control,
                            WM_SETFONT,
                            Some(WPARAM(new_font.0 as usize)),
                            Some(LPARAM(1)),
                        );
                    }
                    let old_font = {
                        let mut form = (*ptr).borrow_mut();
                        form.dpi = dpi;
                        form.content_height =
                            form.content_height * dpi as i32 / old_dpi.max(1) as i32;
                        form.scroll_y = form.scroll_y * dpi as i32 / old_dpi.max(1) as i32;
                        std::mem::replace(&mut form.font, new_font)
                    };
                    if !old_font.is_invalid() {
                        let _ = DeleteObject(old_font.into());
                    }
                    let rect = &*(lp.0 as *const RECT);
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
                    let (viewport, scroll, edits, all, save) = {
                        let form = (*ptr).borrow();
                        (
                            form.viewport,
                            form.scroll_y,
                            form.edits.len(),
                            form.controls.clone(),
                            form.save_button,
                        )
                    };
                    let scale = |n: i32| n * dpi as i32 / 96;
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let _ = MoveWindow(
                        viewport,
                        0,
                        0,
                        client.right,
                        (client.bottom - scale(110)).max(1),
                        true,
                    );
                    for control in all.iter().take(edits * 2) {
                        let mut r = RECT::default();
                        let _ = GetWindowRect(*control, &mut r);
                        let mut points = [POINT {
                            x: r.left,
                            y: r.top,
                        }];
                        windows::Win32::Graphics::Gdi::MapWindowPoints(
                            None,
                            Some(viewport),
                            &mut points,
                        );
                        let _ = MoveWindow(
                            *control,
                            points[0].x,
                            points[0].y,
                            (client.right - scale(48)).max(1),
                            r.bottom - r.top,
                            true,
                        );
                    }
                    let error_result = (*ptr).borrow().error_result;
                    if let Some(output) = error_result {
                        let _ = MoveWindow(
                            output,
                            scale(18),
                            client.bottom - scale(102),
                            (client.right - scale(36)).max(1),
                            scale(50),
                            true,
                        );
                    }
                    if let Some(save) = save {
                        let _ = MoveWindow(
                            save,
                            client.right - scale(218),
                            client.bottom - scale(46),
                            scale(90),
                            scale(30),
                            true,
                        );
                    }
                    if let Some(cancel) = all.last() {
                        let _ = MoveWindow(
                            *cancel,
                            client.right - scale(120),
                            client.bottom - scale(46),
                            scale(90),
                            scale(30),
                            true,
                        );
                    }
                    scroll_form(ptr, scroll);
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    (*ptr).borrow_mut().done = true;
                    cancel_test(ptr);
                    return LRESULT(0);
                }
                WM_NCDESTROY => {
                    (*ptr).borrow_mut().done = true;
                    cancel_test(ptr);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                _ => {}
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
/// A bounded modal editor. It dispatches the application's messages while waiting.
pub unsafe fn form(
    owner: HWND,
    title: &str,
    fields: &[(&str, String)],
    appearance: Theme,
) -> Option<Vec<String>> {
    let fields: Vec<_> = fields
        .iter()
        .map(|(label, value)| (*label, Field::Text(value.clone())))
        .collect();
    unsafe {
        form_fields_with_test_theme(
            owner,
            title,
            &fields,
            None,
            appearance,
            Validation::None,
            None,
        )
    }
}
pub unsafe fn favorite_form(
    owner: HWND,
    fields: &[(&str, String)],
    appearance: Theme,
) -> Option<Vec<String>> {
    let fields: Vec<_> = fields
        .iter()
        .map(|(label, value)| (*label, Field::Text(value.clone())))
        .collect();
    unsafe {
        form_fields_with_test_theme(
            owner,
            "编辑收藏",
            &fields,
            None,
            appearance,
            Validation::Favorite,
            None,
        )
    }
}
#[allow(clippy::too_many_arguments)]
pub unsafe fn settings_form(
    owner: HWND,
    theme: usize,
    hotkey: String,
    clipboard_hotkey: String,
    paused: bool,
    excluded: String,
    preferred: SmartSearchMode,
    submit: Submit<'_>,
) -> Option<Vec<String>> {
    unsafe {
        form_fields_with_test_theme(
            owner,
            "设置",
            &[
                ("外观主题", Field::Choice(theme)),
                ("呼出快捷键（留空关闭）", Field::Text(hotkey)),
                (
                    "剪贴板专用快捷键（留空关闭）",
                    Field::Text(clipboard_hotkey),
                ),
                ("剪贴板记录状态", Field::Toggle(paused)),
                ("排除应用（exe 名称，逗号分隔）", Field::Text(excluded)),
                (
                    "智能查找首选能力",
                    Field::SmartChoice(match preferred {
                        SmartSearchMode::Auto => 0,
                        SmartSearchMode::Claude => 1,
                        SmartSearchMode::Semantic => 2,
                    }),
                ),
            ],
            None,
            match theme {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::System,
            },
            Validation::General,
            submit,
        )
    }
}

pub unsafe fn clipboard_metadata_form(
    owner: HWND,
    summary: &ItemSummary,
    appearance: Theme,
) -> Option<Vec<String>> {
    unsafe {
        form_fields_with_test_theme(
            owner,
            "编辑固定内容的名称、别名和标签",
            &[
                ("名称（1–160 字符）", Field::Text(summary.title.clone())),
                ("别名（最多 128 字符）", Field::Text(summary.alias.clone())),
                (
                    "标签（逗号分隔；最多 32 个）",
                    Field::Text(summary.tags.join(", ")),
                ),
            ],
            None,
            appearance,
            Validation::Metadata(summary.clone()),
            None,
        )
    }
}
pub unsafe fn ai_settings_form(
    owner: HWND,
    settings: &crate::ai::AiSettings,
    appearance: Theme,
    submit: Submit<'_>,
) -> Option<Vec<String>> {
    unsafe {
        form_fields_with_test_theme(
            owner,
            "AI 设置 · 共用于检索与管家",
            &[
                (
                    "可选功能",
                    Field::Switch(settings.enabled, "启用 AI 按意思查找（默认关闭）"),
                ),
                (
                    "Claude 格式 API 地址（根地址、/v1 或完整接口）",
                    Field::Text(settings.endpoint.clone()),
                ),
                (
                    "模型 ID（由服务商提供）",
                    Field::Model(settings.model.clone()),
                ),
                (
                    "认证方式",
                    Field::AuthChoice(usize::from(settings.auth == crate::ai::AuthMethod::Bearer)),
                ),
                (
                    "API Key（留空保留已保存密钥）",
                    Field::Password(String::new()),
                ),
                (
                    "密钥管理",
                    Field::Switch(false, "清除已保存密钥（同时关闭 AI）"),
                ),
                (
                    "推理强度（默认不发送参数，兼容性最佳）",
                    Field::EffortChoice(
                        crate::organizer_ui::EFFORTS
                            .iter()
                            .position(|e| *e == settings.effort)
                            .unwrap_or(0),
                    ),
                ),
            ],
            Some(TestForm::Ai(settings.clone())),
            appearance,
            Validation::Ai(settings.clone()),
            submit,
        )
    }
}
pub unsafe fn organizer_settings_form(
    owner: HWND,
    settings: &crate::organizer::OrganizerSettings,
    ai: &crate::ai::AiSettings,
    path_enabled: bool,
    appearance: Theme,
    health: Arc<std::sync::Mutex<String>>,
) -> Option<Vec<String>> {
    use crate::organizer::{OrganizerMode, OrganizerScope};
    unsafe {
        form_fields_with_test_theme(
            owner,
            "AI 管家设置",
            &[
                (
                    "发送摘要、路径与元数据；不读取文件正文或图片",
                    Field::Switch(settings.enabled, "启用 AI 管家（默认关闭）"),
                ),
                (
                    "整理模式",
                    Field::OrganizerModeChoice(usize::from(
                        settings.mode == OrganizerMode::Automatic,
                    )),
                ),
                (
                    "发送范围（只处理启用后的新内容和观察）",
                    Field::OrganizerScopeChoice(match settings.scope {
                        OrganizerScope::SavedAndPaths => 0,
                        OrganizerScope::SavedOnly => 1,
                        OrganizerScope::PathsOnly => 2,
                        OrganizerScope::AllTextAndPaths => 3,
                    }),
                ),
                (
                    "整理间隔（30–3600 秒）",
                    Field::Text(settings.interval_seconds.to_string()),
                ),
                (
                    "每批数量（1–8 条）",
                    Field::Text(settings.batch_size.to_string()),
                ),
                (
                    "整理说明（最多 512 字符）",
                    Field::Multiline(settings.instructions.clone()),
                ),
                (
                    "整理记忆",
                    Field::Switch(settings.memory_enabled, "记住整理反馈"),
                ),
                (
                    "Pi 最大模型轮次（2–8）",
                    Field::Text(settings.max_rounds.to_string()),
                ),
            ],
            Some(TestForm::Organizer {
                ai: ai.clone(),
                path_enabled,
                owner: owner.0 as usize,
                health,
            }),
            appearance,
            Validation::Organizer(ai.clone()),
            None,
        )
    }
}
pub unsafe fn embedding_settings_form(
    owner: HWND,
    settings: &crate::semantic::EmbeddingSettings,
    appearance: Theme,
    submit: Submit<'_>,
) -> Option<Vec<String>> {
    unsafe {
        form_fields_with_test_theme(
            owner,
            "语义检索设置 · 独立 Embedding 服务",
            &[
                (
                    "查询与选定保存文本会发往独立服务",
                    Field::Switch(settings.enabled, "启用语义检索（默认关闭）"),
                ),
                (
                    "OpenAI 兼容 API 地址（根地址、/v1 或完整接口）",
                    Field::Text(settings.endpoint.clone()),
                ),
                (
                    "模型 ID（由服务商提供）",
                    Field::Text(settings.model.clone()),
                ),
                (
                    "API Key（留空保留已保存密钥）",
                    Field::Password(String::new()),
                ),
                (
                    "密钥管理",
                    Field::Switch(false, "清除已保存密钥（同时关闭语义检索）"),
                ),
                (
                    "文档/目录仅发名称、路径、别名、标签，不解析正文",
                    Field::IndexChoice(usize::from(
                        settings.index_scope == crate::semantic::IndexScope::AllText,
                    )),
                ),
            ],
            Some(TestForm::Embedding(settings.clone())),
            appearance,
            Validation::Embedding(settings.clone()),
            submit,
        )
    }
}
unsafe fn form_fields_with_test_theme(
    owner: HWND,
    title: &str,
    fields: &[(&str, Field)],
    test: Option<TestForm>,
    appearance: Theme,
    validation: Validation,
    mut submit: Submit<'_>,
) -> Option<Vec<String>> {
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
        let testing = test.is_some();
        let validating = test.is_some();
        let model_listing = matches!(test, Some(TestForm::Ai(_)));
        let initial_test_note = if model_listing {
            "获取模型不发推理请求；测试只发送内置文本"
        } else if matches!(test, Some(TestForm::Embedding(_))) {
            "测试只发送内置文本；保存不调用服务"
        } else {
            ""
        };
        let state = Box::new(RefCell::new(Form {
            edits: Vec::new(),
            controls: Vec::new(),
            colors: theme::Colors::new(appearance),
            appearance,
            font: HFONT::default(),
            dpi: 96,
            done: false,
            accepted: false,
            test,
            validation,
            status: FormStatus {
                test: initial_test_note.into(),
                ..Default::default()
            },
            labels: fields
                .iter()
                .map(|(label, _)| (*label).to_owned())
                .collect(),
            health_requested: false,
            test_gate: ConnectionTestGate::default(),
            test_button: None,
            test_result: None,
            error_result: None,
            save_button: None,
            model_button: None,
            models: Vec::new(),
            choices: fields
                .iter()
                .enumerate()
                .filter_map(|(i, (_, f))| {
                    matches!(
                        f,
                        Field::Choice(_)
                            | Field::AuthChoice(_)
                            | Field::IndexChoice(_)
                            | Field::SmartChoice(_)
                            | Field::EffortChoice(_)
                            | Field::OrganizerModeChoice(_)
                            | Field::OrganizerScopeChoice(_)
                    )
                    .then_some(i)
                })
                .collect(),
            switches: fields
                .iter()
                .enumerate()
                .filter_map(|(i, (_, f))| {
                    matches!(f, Field::Toggle(_) | Field::Switch(_, _)).then_some(i)
                })
                .collect(),
            viewport: HWND::default(),
            scroll_y: 0,
            content_height: 0,
        }));
        let title = wide(title);
        let owner_dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(owner).max(96);
        let mut monitor = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        );
        let work = monitor.rcWork;
        let multiline_extra = fields
            .iter()
            .filter(|(_, field)| matches!(field, Field::Multiline(_)))
            .count() as i32
            * 92;
        let logical_height =
            156 + fields.len() as i32 * 62 + multiline_extra + if validating { 180 } else { 0 };
        let dpi = owner_dpi;
        let fit = crate::organizer_ui::fit_form(
            work.right - work.left,
            work.bottom - work.top,
            dpi,
            logical_height,
        );
        let scale = |v: i32| v * (dpi as i32) / 96;
        let height = fit.height;
        let hwnd = CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            class,
            PCWSTR(title.as_ptr()),
            WS_CAPTION | WS_SYSMENU | WS_POPUP,
            work.left + ((work.right - work.left - fit.width) / 2).max(0),
            work.top + ((work.bottom - work.top - height) / 2).max(0),
            fit.width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            Some((&*state as *const RefCell<Form>).cast()),
        )
        .ok()?;
        let _window_guard = FormWindow(hwnd);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let viewport = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("STATIC"),
            PCWSTR::null(),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL | WS_CLIPCHILDREN,
            0,
            0,
            client.right,
            (client.bottom - scale(110)).max(1),
            Some(hwnd),
            None,
            None,
            None,
        )
        .ok()?;
        if !SetWindowSubclass(
            viewport,
            Some(viewport_proc),
            4,
            (&*state as *const RefCell<Form>) as usize,
        )
        .as_bool()
        {
            return None;
        }
        {
            let mut form = state.borrow_mut();
            form.viewport = viewport;
            form.content_height = (fit.content_height - scale(52)).max(1);
        }
        let font = theme::font(dpi);
        {
            let mut form = state.borrow_mut();
            form.font = font;
            form.dpi = dpi;
        }
        let dark = state.borrow().colors.dark;
        theme::apply(hwnd, dark);
        let mut field_y = 12;
        for (i, (label, value)) in fields.iter().enumerate() {
            let label = wide(label);
            let value = wide(match value {
                Field::Text(t) | Field::Password(t) | Field::Multiline(t) | Field::Model(t) => t,
                Field::Choice(_)
                | Field::AuthChoice(_)
                | Field::IndexChoice(_)
                | Field::SmartChoice(_)
                | Field::EffortChoice(_)
                | Field::OrganizerModeChoice(_)
                | Field::OrganizerScopeChoice(_) => "",
                Field::Toggle(_) => "暂停剪贴板记录",
                Field::Switch(_, label) => label,
            });
            let y = scale(field_y);
            field_y += 62
                + if matches!(fields[i].1, Field::Multiline(_)) {
                    92
                } else {
                    0
                };
            let l = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                PCWSTR(label.as_ptr()),
                WS_CHILD | WS_VISIBLE,
                scale(18),
                y,
                (client.right - scale(48)).max(1),
                scale(22),
                Some(viewport),
                None,
                None,
                None,
            )
            .ok()?;
            let e = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                match fields[i].1 {
                    Field::Text(_) | Field::Password(_) | Field::Multiline(_) => w!("EDIT"),
                    Field::Choice(_)
                    | Field::AuthChoice(_)
                    | Field::IndexChoice(_)
                    | Field::SmartChoice(_) => {
                        w!("COMBOBOX")
                    }
                    Field::Model(_)
                    | Field::EffortChoice(_)
                    | Field::OrganizerModeChoice(_)
                    | Field::OrganizerScopeChoice(_) => w!("COMBOBOX"),
                    Field::Toggle(_) | Field::Switch(_, _) => w!("BUTTON"),
                },
                PCWSTR(value.as_ptr()),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_TABSTOP
                    | WINDOW_STYLE(match fields[i].1 {
                        Field::Text(_) => ES_AUTOHSCROLL as u32,
                        Field::Model(_) => (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32,
                        Field::EffortChoice(_)
                        | Field::OrganizerModeChoice(_)
                        | Field::OrganizerScopeChoice(_) => CBS_DROPDOWNLIST as u32,
                        Field::Multiline(_) => {
                            (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32 | WS_VSCROLL.0
                        }
                        Field::Password(_) => (ES_AUTOHSCROLL | ES_PASSWORD) as u32,
                        Field::Choice(_)
                        | Field::AuthChoice(_)
                        | Field::IndexChoice(_)
                        | Field::SmartChoice(_) => CBS_DROPDOWNLIST as u32,
                        Field::Toggle(_) | Field::Switch(_, _) => BS_AUTOCHECKBOX as u32,
                    }),
                scale(18),
                y + scale(24),
                (client.right - scale(48)).max(1),
                scale(
                    if matches!(
                        fields[i].1,
                        Field::Choice(_)
                            | Field::AuthChoice(_)
                            | Field::IndexChoice(_)
                            | Field::SmartChoice(_)
                            | Field::Model(_)
                            | Field::EffortChoice(_)
                            | Field::OrganizerModeChoice(_)
                            | Field::OrganizerScopeChoice(_)
                    ) {
                        150
                    } else if matches!(fields[i].1, Field::Multiline(_)) {
                        120
                    } else {
                        28
                    },
                ),
                Some(viewport),
                None,
                None,
                None,
            )
            .ok()?;
            SendMessageW(l, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            SendMessageW(e, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            match fields[i].1 {
                Field::Choice(selected)
                | Field::AuthChoice(selected)
                | Field::IndexChoice(selected)
                | Field::SmartChoice(selected) => {
                    let choices: &[&str] = if matches!(fields[i].1, Field::AuthChoice(_)) {
                        &["API Key（x-api-key）", "Bearer token（兼容网关）"]
                    } else if matches!(fields[i].1, Field::IndexChoice(_)) {
                        &["固定内容与收藏", "全部文本"]
                    } else if matches!(fields[i].1, Field::SmartChoice(_)) {
                        &[
                            "自动：有索引查相似内容，否则用 Claude",
                            "Claude：理解描述并查本机内容与功能",
                            "语义：在已建立的索引中找相似内容",
                        ]
                    } else {
                        &["跟随系统", "浅色", "深色"]
                    };
                    for choice in choices {
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
                Field::EffortChoice(selected)
                | Field::OrganizerModeChoice(selected)
                | Field::OrganizerScopeChoice(selected) => {
                    let choices: &[&str] = match fields[i].1 {
                        Field::EffortChoice(_) => &[
                            "默认（省略 effort）",
                            "low",
                            "medium",
                            "high",
                            "xhigh",
                            "max",
                        ],
                        Field::OrganizerModeChoice(_) => {
                            &["后台生成建议，由我确认", "后台自动应用空别名与标签"]
                        }
                        _ => &[
                            "收藏、固定剪贴板和外部路径",
                            "仅收藏和固定剪贴板",
                            "仅外部常用路径",
                            "上述范围加普通剪贴板文本",
                        ],
                    };
                    for choice in choices {
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
                Field::Model(_) => {
                    SendMessageW(e, CB_LIMITTEXT, Some(WPARAM(256)), None);
                }
                Field::Toggle(checked) | Field::Switch(checked, _) => {
                    SendMessageW(e, BM_SETCHECK, Some(WPARAM(usize::from(checked))), None);
                }
                Field::Multiline(_) => {
                    SendMessageW(e, EM_SETLIMITTEXT, Some(WPARAM(128 * 2050)), None);
                }
                Field::Text(_) | Field::Password(_) => {}
            }
            {
                let mut form = state.borrow_mut();
                form.edits.push(e);
                form.controls.extend([l, e]);
            }
        }
        if validating {
            let y = scale(field_y);
            let button = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                w!("测试连接"),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                scale(18),
                y,
                scale(136),
                scale(30),
                Some(viewport),
                Some(HMENU(TEST_BUTTON as *mut _)),
                None,
                None,
            )
            .ok()?;
            if matches!(state.borrow().test, Some(TestForm::Organizer { .. })) {
                let label = wide("测试 Pi 运行时");
                let _ = SetWindowTextW(button, PCWSTR(label.as_ptr()));
            }
            if !testing {
                let _ = ShowWindow(button, SW_HIDE);
            }
            if model_listing {
                let models = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    w!("获取模型"),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                    scale(166),
                    y,
                    scale(116),
                    scale(30),
                    Some(viewport),
                    Some(HMENU(MODEL_BUTTON as *mut _)),
                    None,
                    None,
                )
                .ok()?;
                SendMessageW(models, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
                state.borrow_mut().controls.push(models);
                state.borrow_mut().model_button = Some(models);
            }
            let output = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                if testing {
                    w!("获取模型不发推理请求；测试只发送内置文本")
                } else {
                    w!("管家复用 AI 设置；保存配置后立即生效")
                },
                WS_CHILD | WS_VISIBLE,
                scale(18),
                y + scale(36),
                (client.right - scale(48)).max(1),
                scale(126),
                Some(viewport),
                None,
                None,
                None,
            )
            .ok()?;
            SendMessageW(button, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            SendMessageW(output, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            state.borrow_mut().controls.extend([button, output]);
            {
                let mut form = state.borrow_mut();
                form.test_button = Some(button);
                form.test_result = Some(output);
            }
            let note = {
                let form = state.borrow();
                match &form.test {
                    Some(TestForm::Organizer {
                        ai, path_enabled, ..
                    }) => Some(format!(
                        "{}{}",
                        crate::organizer_ui::organizer_config_warning(ai)
                            .unwrap_or_else(|| "复用 AI 设置；检索开关可保持关闭".into()),
                        if *path_enabled {
                            ""
                        } else {
                            "\r\n路径范围需在“常用推荐设置”中启用观察"
                        }
                    )),
                    _ => None,
                }
            };
            if let Some(note) = note {
                state.borrow_mut().status.config = note;
            }
            render_status(&*state);
            let available = matches!(state.borrow().test, Some(TestForm::Organizer { .. }))
                || !TEST_WORKER.with(|worker| worker.busy());
            let _ = EnableWindow(button, available);
            if testing && SetTimer(Some(hwnd), TEST_TIMER, 100, None) == 0 {
                let _ = EnableWindow(button, false);
                test_status(&*state, "无法启动连接测试状态更新，请重新打开设置");
            }
        }
        let error_output = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!(""),
            WS_CHILD | WS_VISIBLE,
            scale(18),
            client.bottom - scale(102),
            (client.right - scale(36)).max(1),
            scale(50),
            Some(hwnd),
            None,
            None,
            None,
        )
        .ok()?;
        SendMessageW(
            error_output,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            None,
        );
        {
            let mut form = state.borrow_mut();
            form.error_result = Some(error_output);
            form.controls.push(error_output);
        }
        for (id, label, x) in [
            (1, "保存", client.right - scale(218)),
            (2, "取消", client.right - scale(120)),
        ] {
            let s = wide(label);
            let b = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("BUTTON"),
                PCWSTR(s.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                x,
                client.bottom - scale(46),
                scale(90),
                scale(30),
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )
            .ok()?;
            SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), None);
            state.borrow_mut().controls.push(b);
            if id == 1 {
                state.borrow_mut().save_button = Some(b);
            }
        }
        scroll_form(&*state, 0);
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let first_edit = state.borrow().edits.first().copied();
        if let Some(e) = first_edit {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(e));
        }
        let mut msg = MSG::default();
        let mut quit = None;
        let mut submission = Submission::default();
        loop {
            if state.borrow().done {
                if state.borrow().accepted {
                    state.borrow_mut().status.error = None;
                    let values = read_values(&*state);
                    submission.attempt(&values.values, |raw| {
                        validate_form(&*state, raw)?;
                        if let Some(callback) = submit.as_deref_mut() {
                            callback(raw)?;
                        }
                        Ok(())
                    });
                    if !submission.accepted {
                        {
                            let mut form = state.borrow_mut();
                            form.done = false;
                            form.accepted = false;
                        }
                        if let Some(error) = submission.error.take() {
                            form_error(&*state, error);
                        }
                        continue;
                    }
                }
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
                cancel_test(&*state);
                continue;
            }
            if composing || !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        cancel_test(&*state);
        if testing {
            let _ = KillTimer(Some(hwnd), TEST_TIMER);
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
                    Field::Text(_) | Field::Password(_) | Field::Multiline(_) | Field::Model(_) => {
                        if submit.is_some() && matches!(fields[i].1, Field::Password(_)) {
                            String::new()
                        } else {
                            text(*e)
                        }
                    }
                    Field::Choice(_)
                    | Field::AuthChoice(_)
                    | Field::IndexChoice(_)
                    | Field::SmartChoice(_)
                    | Field::EffortChoice(_)
                    | Field::OrganizerModeChoice(_)
                    | Field::OrganizerScopeChoice(_) => {
                        SendMessageW(*e, CB_GETCURSEL, None, None).0.to_string()
                    }
                    Field::Toggle(_) | Field::Switch(_, _) => {
                        SendMessageW(*e, BM_GETCHECK, None, None).0.to_string()
                    }
                })
                .collect()
        });
        let _ = DestroyWindow(hwnd);

        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        if let Some(code) = quit {
            PostQuitMessage(code);
        }
        result
    }
}
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, DeleteObject, HBRUSH, HFONT};
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
