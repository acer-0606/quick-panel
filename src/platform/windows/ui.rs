#![allow(unsafe_op_in_unsafe_fn)]
use super::{
    actions, ai_client, ai_time, clipboard,
    dialogs::{self, wide},
    images::{self, Image},
    organizer_ui,
    path_monitor::PathMonitor,
    screenshot, theme,
};
use crate::{
    ai::{AiPlan, AiSettings, AuthMethod},
    ai_jobs::{AiRequest, AiResultGate, AiWorker, revoke_adopted_plan},
    background_job::{ActionLoads, ActionPermit, ActionTicket, BackgroundWorker},
    embedding_jobs::{EmbeddingRequest, EmbeddingWorker},
    engine::Engine,
    hotkeys::{HotkeyBindings, format_hotkey, parse_hotkey, settings_hotkeys},
    model::*,
    palette::{self, Context as PaletteContext, Entry as PaletteEntry, Function},
    preview::{PreviewLoads, bounded_text},
    query_feedback::{IndexReply, QueryReply, claude_reply, embedding_reply, index_reply},
    screenshot::{ScreenshotGate, ScreenshotTicket},
    semantic::{self, EmbeddingSettings, IndexScope},
    semantic_ui::{
        IndexSession, QuerySession, finish_semantic_effect_with_revoke, prepare_semantic_effect,
        sync_semantic_view,
    },
    smart_search::{ClipboardMetadataForm, SmartAction, SmartPreflight, SmartView, StatusCache},
    ui_state::{KeyAction, Lifecycle, MenuTarget, PanelKey, PathResultBatches, UiState},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::Path,
    rc::Rc,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, DataExchange::*, LibraryLoader::GetModuleHandleW},
        UI::{Controls::*, HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
const ENGINE_WAKE: u32 = WM_APP + 1;
const WORK_WAKE: u32 = WM_APP + 2;
const TRAY: u32 = WM_APP + 3;
const DEFERRED_WAKE: u32 = WM_APP + 4;
const SHOW_EXISTING: u32 = WM_APP + 5;
const ORGANIZER_UI_WAKE: u32 = WM_APP + 6;
const SEARCH: usize = 100;
const LIST: usize = 101;
const ADD: usize = 102;
const PREVIEW: usize = 103;
const SETTINGS: usize = 104;
const SMART_FIND: usize = 105;
const GENERAL_SETTINGS: usize = 106;
const AI_SETTINGS: usize = 107;
const INDEX_STATUS: usize = 108;
const EMBEDDING_SETTINGS: usize = 109;
const INDEX_START: usize = 110;
const INDEX_CANCEL: usize = 111;
const PATH_SETTINGS: usize = 112;
const SCREENSHOT: usize = 113;
const SCREENSHOT_SETTINGS: usize = 114;
const ORGANIZER_SETTINGS: usize = 115;
const ORGANIZER_REVIEW: usize = 116;
const ORGANIZER_TIMER: usize = 401;
const PATH_TIMER: usize = 400;
const FAVORITE_PATH: usize = 210;
const SUPPRESS_PATH: usize = 211;
const OPEN: usize = 200;
const COPY: usize = 201;
const PLAIN: usize = 202;
const PIN: usize = 203;
const EDIT: usize = 204;
const RELINK: usize = 205;
const REVEAL: usize = 206;
const DELETE: usize = 207;
const COPY_PATH: usize = 208;
const OPEN_WITH: usize = 209;
const EXIT: usize = 220;
#[derive(Clone, Copy)]
enum Action {
    Open(bool),
    Copy(bool),
    Pin,
    Edit,
    Relink,
    Reveal,
    Delete,
    CopyPath,
    Preview,
    Favorite,
    Suppress,
}
enum Work {
    Capture(u32),
    Stop,
}
enum Done {
    Captured(Result<Option<ClipboardSnapshot>, String>),
}
type PendingPaste = (
    u64,
    StoredItem,
    Action,
    HWND,
    Option<clipboard::PreparedCompatibility>,
);
struct PreviewResult {
    generation: u64,
    text: String,
    image: Option<Result<Image, String>>,
}
struct ScreenshotJob {
    ticket: ScreenshotTicket,
    pixels: screenshot::Pixels,
}
struct ScreenshotResult {
    ticket: ScreenshotTicket,
    result: Result<clipboard::PreparedScreenshot, String>,
}
fn encode_screenshot(job: ScreenshotJob) -> ScreenshotResult {
    let ScreenshotJob { ticket, pixels } = job;
    // A worker panic still yields a terminal receipt, releasing the actual slot.
    let result = std::panic::catch_unwind(|| {
        if !ticket.current() {
            return Err("截图已取消".into());
        }
        let png = images::encode_png(pixels.width, pixels.height, pixels.bgra)?;
        if !ticket.current() {
            return Err("截图已取消".into());
        }
        clipboard::prepare_screenshot(png)
    })
    .unwrap_or_else(|_| Err("截图编码失败，可重新截图".into()));
    ScreenshotResult { ticket, result }
}
enum NativeJob {
    Action {
        item: StoredItem,
        action: Action,
        ticket: ActionTicket,
    },
    Paths {
        item: Option<StoredItem>,
        paths: Vec<String>,
        ticket: ActionTicket,
    },
}
enum NativeResult {
    Action {
        item: StoredItem,
        action: Action,
        ticket: ActionTicket,
        dib: Option<clipboard::PreparedCompatibility>,
        result: Result<bool, String>,
        missing: bool,
    },
    Paths {
        item: Option<StoredItem>,
        paths: Vec<(String, ItemKind)>,
        ticket: ActionTicket,
    },
}
fn native_job(job: NativeJob) -> NativeResult {
    match job {
        NativeJob::Action {
            item,
            action,
            ticket,
        } => {
            let mut dib = None;
            let result = if !ticket.current() {
                Ok(false)
            } else {
                match action {
                    Action::Open(_) if item.summary.kind != ItemKind::Clipboard => {
                        actions::open_target(&item, &ticket)
                    }
                    Action::Reveal => actions::reveal_target(&item, &ticket),
                    Action::Open(plain) | Action::Copy(plain) => {
                        dib = clipboard::prepare_compatibility(&item, plain);
                        Ok(false)
                    }
                    _ => Ok(false),
                }
            };
            let missing = result.is_err()
                && ticket.current()
                && item
                    .target
                    .as_ref()
                    .is_some_and(|p| std::fs::metadata(p).is_err());
            NativeResult::Action {
                item,
                action,
                ticket,
                dib,
                result,
                missing,
            }
        }
        NativeJob::Paths {
            item,
            paths,
            ticket,
        } => {
            let paths = paths
                .into_iter()
                .map(|path| {
                    let kind = if std::fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
                        ItemKind::Folder
                    } else {
                        ItemKind::Document
                    };
                    (path, kind)
                })
                .collect();
            NativeResult::Paths {
                item,
                paths,
                ticket,
            }
        }
    }
}
fn prepare_preview((generation, item): (u64, StoredItem)) -> PreviewResult {
    let text = if item.summary.kind == ItemKind::Clipboard {
        bounded_text(&item.text)
    } else {
        let path = item.target.as_deref().unwrap_or_default();
        bounded_text(&format!(
            "{}\n\n{}\n\n{}",
            item.summary.title,
            path,
            match std::fs::metadata(path) {
                Ok(m) => format!(
                    "大小：{} 字节\n标签：{}",
                    m.len(),
                    item.summary.tags.join("、")
                ),
                Err(_) => "目标已失效 · Ctrl+K → 重新定位".into(),
            }
        ))
    };
    let image = item
        .representations
        .iter()
        .any(|r| matches!(r.format.as_str(), "PNG" | "CF_DIB" | "CF_DIBV5"))
        .then(|| images::preview(&item));
    PreviewResult {
        generation,
        text,
        image,
    }
}
#[derive(Clone)]
struct RenderState {
    dpi: u32,
    colors: Rc<theme::Colors>,
    font: HFONT,
    saved_times: Rc<Vec<String>>,
    row_details: Rc<Vec<palette::RowDetail>>,
    organizer_revision: u64,
    query: String,
    items: Vec<PaletteEntry>,
    image: Option<Arc<Image>>,
}
impl RenderState {
    fn s(&self, n: i32) -> i32 {
        n * self.dpi as i32 / 96
    }
}
enum UiEffect {
    OrganizerSettings {
        owner: HWND,
        settings: crate::organizer::OrganizerSettings,
    },
    HarnessMemory {
        owner: HWND,
        state: Rc<RefCell<crate::organizer_ui::MemoryUiState>>,
        outbox: Rc<RefCell<(Option<Command>, Option<Command>)>>,
        appearance: Theme,
    },
    OrganizerReview {
        owner: HWND,
        state: Rc<RefCell<crate::organizer_ui::ReviewState>>,
        outbox: Rc<RefCell<(Option<Command>, Option<Command>)>>,
        appearance: Theme,
        enabled: bool,
    },
    ScreenshotSelect {
        owner: HWND,
        target: HWND,
        ticket: ScreenshotTicket,
    },
    ScreenshotDeliver {
        owner: HWND,
        ticket: ScreenshotTicket,
        prepared: clipboard::PreparedScreenshot,
    },
    ScreenshotSettings {
        owner: HWND,
        settings: Settings,
    },
    Picker {
        owner: HWND,
        folder: bool,
        item: Option<StoredItem>,
    },
    Favorite {
        owner: HWND,
        item: Option<StoredItem>,
        kind: ItemKind,
        fields: Vec<(&'static str, String)>,
    },
    Settings {
        owner: HWND,
        settings: Settings,
    },
    PathSettings {
        owner: HWND,
        settings: crate::path_activity::PathActivitySettings,
    },
    AiSettings {
        owner: HWND,
        settings: AiSettings,
    },
    EmbeddingSettings {
        owner: HWND,
        settings: EmbeddingSettings,
    },
    SmartPrompt {
        owner: HWND,
        preferred: Option<SmartSearchMode>,
    },
    ClipboardMetadata {
        owner: HWND,
        form: ClipboardMetadataForm,
        target: MenuTarget,
    },
    ExitError {
        owner: HWND,
        message: String,
    },
    Message {
        owner: HWND,
        message: String,
    },
    Menu {
        owner: HWND,
        x: i32,
        y: i32,
        entries: Vec<(usize, &'static str)>,
        target: Option<MenuTarget>,
    },
    Destroy(HWND),
}
enum Deferred {
    Message(u32, WPARAM, LPARAM),
    Dpi(WPARAM, RECT),
}
/// Every callback retains only a shared Host. App is borrowed through one RefCell;
/// rendering has its own immutable snapshot and never borrows App during SendMessage.
struct Host {
    app: RefCell<App>,
    render: RefCell<Rc<RenderState>>,
    hwnd: Cell<HWND>,
    composing: Cell<bool>,
    deferred: RefCell<VecDeque<Deferred>>,
    wake_posted: Cell<bool>,
    effects_running: Cell<bool>,
}
/// Holding the named kernel handle, rather than mutex ownership, establishes
/// instance lifetime. A duplicate closes its extra handle before waiting/notifying.
struct SingleInstance(windows::Win32::Foundation::HANDLE);
impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl SingleInstance {
    unsafe fn acquire_or_notify() -> Result<Option<Self>, String> {
        SetLastError(WIN32_ERROR(0));
        let handle = windows::Win32::System::Threading::CreateMutexW(
            None,
            false,
            w!("Local\\QuickPanel.SingleInstance.6CD5F4E3"),
        )
        .map_err(|error| format!("无法建立单实例保护：{error}"))?;
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        let instance = Self(handle);
        if !already_running {
            return Ok(Some(instance));
        }
        drop(instance);
        // The first process may still be between acquiring its guard and creating
        // its window. Never open its database or become another writer in this case.
        for _ in 0..30 {
            if let Ok(hwnd) = FindWindowW(w!("QuickPanelWindow"), PCWSTR::null()) {
                // A user-launched process can grant the existing process permission
                // to take foreground; failure retains the normal Win32 fallback.
                let mut process = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut process));
                if process != 0 {
                    let _ = AllowSetForegroundWindow(process);
                }
                if let Err(error) = PostMessageW(Some(hwnd), SHOW_EXISTING, WPARAM(0), LPARAM(0)) {
                    let text = wide(&format!(
                        "快捷面板已经运行，但无法唤起现有窗口。请从托盘打开。\n{error}"
                    ));
                    MessageBoxW(
                        None,
                        PCWSTR(text.as_ptr()),
                        w!("快捷面板已经运行"),
                        MB_OK | MB_ICONINFORMATION,
                    );
                }
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        MessageBoxW(
            None,
            w!("已有快捷面板正在启动或退出，请稍后重试。"),
            w!("快捷面板已经运行"),
            MB_OK | MB_ICONINFORMATION,
        );
        Ok(None)
    }
}
struct WindowGuard(HWND);
impl Drop for WindowGuard {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.0)).as_bool() {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
type OrganizerWorker = crate::harness_worker::HarnessWorker;
pub(super) const PI_HEALTH_WAKE: u32 = WM_APP + 81;
struct App {
    organizer_progress: crate::organizer_display::ProgressCache,
    organizer_label: HWND,
    organizer_preserve_order: bool,
    organizer_worker: Option<OrganizerWorker>,
    organizer_session: crate::organizer_ui::OrganizerSession,
    organizer_tick_pending: bool,
    organizer_review: Rc<RefCell<crate::organizer_ui::ReviewState>>,
    organizer_outbox: Rc<RefCell<(Option<Command>, Option<Command>)>>,
    organizer_page_open: bool,
    harness_memory: Rc<RefCell<crate::organizer_ui::MemoryUiState>>,
    runtime_health: Arc<std::sync::Mutex<String>>,
    host: *const Host,
    effects: VecDeque<UiEffect>,
    hotkey_bindings: HotkeyBindings,
    hwnd: HWND,
    edit: HWND,
    list: HWND,
    footer: HWND,
    preview: HWND,
    smart_button: HWND,
    index_label: HWND,
    buttons: Vec<HWND>,
    font: HFONT,
    colors: Rc<theme::Colors>,
    engine: Option<Engine>,
    state: UiState,
    settings: Settings,
    items: Vec<PaletteEntry>,
    saved_items: Vec<ItemSummary>,
    saved_ready: bool,
    suggested_items: Vec<crate::path_activity::PathSuggestion>,
    path_batches: PathResultBatches,
    path_monitor: Option<PathMonitor>,
    query: String,
    result_scope: Scope,
    ai_worker: Option<AiWorker>,
    ai_gate: AiResultGate,
    ai_plan: Option<AiPlan>,
    semantic_query: QuerySession,
    semantic_index: IndexSession,
    semantic_status: StatusCache,
    smart_preflight: SmartPreflight,
    smart_mode: Option<SmartSearchMode>,
    query_worker: Option<EmbeddingWorker>,
    index_worker: Option<EmbeddingWorker>,
    scope: Scope,
    target: HWND,
    load_id: u64,
    action_loads: ActionLoads,
    pending_action: Option<(u64, Action)>,
    pending_paste: Option<PendingPaste>,
    preview_id: u64,
    preview_open: bool,
    preview_loading: bool,
    image: Option<Arc<Image>>,
    tx: mpsc::Sender<Work>,
    rx: mpsc::Receiver<Done>,
    capture_worker: Option<std::thread::JoinHandle<()>>,
    preview_loads: PreviewLoads,
    preview_worker: Option<BackgroundWorker<(u64, StoredItem), PreviewResult>>,
    action_worker: Option<BackgroundWorker<NativeJob, NativeResult>>,
    screenshot_worker: Option<BackgroundWorker<ScreenshotJob, ScreenshotResult>>,
    screenshot_gate: ScreenshotGate,
    action_permit: ActionPermit,
    dpi: u32,
    suppress_sequence: Option<u32>,
    last_capture_sequence: u32,
}
/// Windows converts the stored Unix timestamp using the current local time zone.
fn local_saved_time(milliseconds: i64) -> String {
    let Some(ticks) = milliseconds
        .checked_add(11_644_473_600_000)
        .and_then(|ms| ms.checked_mul(10_000))
    else {
        return String::new();
    };
    if ticks < 0 {
        return String::new();
    }
    let ticks = ticks as u64;
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc_time = SYSTEMTIME::default();
    let mut time = SYSTEMTIME::default();
    unsafe {
        if windows::Win32::System::Time::FileTimeToSystemTime(&utc, &mut utc_time).is_err()
            || windows::Win32::System::Time::SystemTimeToTzSpecificLocalTimeEx(
                None, &utc_time, &mut time,
            )
            .is_err()
        {
            return String::new();
        }
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
    )
}
unsafe fn post(handle: usize, msg: u32) {
    let _ = PostMessageW(Some(HWND(handle as *mut _)), msg, WPARAM(0), LPARAM(0));
}
pub fn run() -> Result<(), String> {
    unsafe {
        let Some(_instance_guard) = SingleInstance::acquire_or_notify()? else {
            return Ok(());
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("QuickPanelWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.to_string())?,
            hIcon: LoadIconW(Some(instance.into()), PCWSTR(101usize as *const u16))
                .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let (tx, work_rx) = mpsc::channel();
        let (done_tx, rx) = mpsc::channel();
        let app_data = App {
            organizer_progress: crate::organizer_display::ProgressCache::default(),
            organizer_label: HWND::default(),
            organizer_preserve_order: false,
            organizer_worker: None,
            organizer_session: crate::organizer_ui::OrganizerSession::default(),
            organizer_tick_pending: false,
            organizer_review: Rc::new(RefCell::new(crate::organizer_ui::ReviewState::default())),
            organizer_outbox: Rc::new(RefCell::new((None, None))),
            organizer_page_open: false,
            harness_memory: Rc::new(RefCell::new(Default::default())),
            runtime_health: Arc::new(std::sync::Mutex::new("启动测试不调用模型".into())),
            host: std::ptr::null(),
            effects: VecDeque::new(),
            hotkey_bindings: HotkeyBindings::default(),
            hwnd: HWND::default(),
            edit: HWND::default(),
            list: HWND::default(),
            footer: HWND::default(),
            preview: HWND::default(),
            smart_button: HWND::default(),
            index_label: HWND::default(),
            buttons: Vec::new(),
            font: HFONT::default(),
            colors: Rc::new(theme::Colors::new(Theme::System)),
            engine: None,
            state: UiState::default(),
            settings: Settings::default(),
            items: Vec::new(),
            saved_items: Vec::new(),
            saved_ready: false,
            suggested_items: Vec::new(),
            path_batches: PathResultBatches::default(),
            path_monitor: None,
            query: String::new(),
            result_scope: Scope::All,
            ai_worker: None,
            ai_gate: AiResultGate::default(),
            ai_plan: None,
            semantic_query: QuerySession::default(),
            semantic_index: IndexSession::default(),
            semantic_status: StatusCache::default(),
            smart_preflight: SmartPreflight::default(),
            smart_mode: None,
            query_worker: None,
            index_worker: None,
            scope: Scope::All,
            target: HWND::default(),
            load_id: 0,
            action_loads: ActionLoads::default(),
            pending_action: None,
            pending_paste: None,
            preview_id: 0,
            preview_open: false,
            preview_loading: false,
            image: None,
            tx,
            rx,
            capture_worker: None,
            preview_loads: PreviewLoads::default(),
            preview_worker: None,
            action_worker: None,
            screenshot_worker: None,
            screenshot_gate: ScreenshotGate::default(),
            action_permit: ActionPermit::default(),
            dpi: 96,
            suppress_sequence: None,
            last_capture_sequence: 0,
        };
        let host_colors = app_data.colors.clone();
        let host = Box::new(Host {
            app: RefCell::new(app_data),
            render: RefCell::new(Rc::new(RenderState {
                dpi: 96,
                colors: host_colors,
                saved_times: Rc::new(Vec::new()),
                row_details: Rc::new(Vec::new()),
                organizer_revision: 0,
                query: String::new(),
                font: HFONT::default(),
                items: Vec::new(),
                image: None,
            })),
            hwnd: Cell::new(HWND::default()),
            composing: Cell::new(false),
            deferred: RefCell::new(VecDeque::new()),
            wake_posted: Cell::new(false),
            effects_running: Cell::new(false),
        });
        let host_ptr = &*host as *const Host;
        host.app.borrow_mut().host = host_ptr;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("快捷面板"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            680,
            480,
            None,
            None,
            Some(instance.into()),
            Some(host_ptr.cast()),
        )
        .map_err(|e| e.to_string())?;
        let _window_guard = WindowGuard(hwnd);
        let mut app = host.app.borrow_mut();
        app.hwnd = hwnd;
        app.dpi = GetDpiForWindow(hwnd).max(96);
        app.create_controls()?;
        let handle = hwnd.0 as usize;
        let notify = Arc::new(move || post(handle, ENGINE_WAKE));
        let base = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let dir = base.join("QuickPanel");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        app.engine = Some(Engine::start(
            EngineOptions::new(dir.join("quick-panel.sqlite3")),
            notify,
        )?);
        let capture_done_tx = done_tx.clone();
        app.capture_worker = Some(std::thread::spawn(move || {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            while let Ok(work) = work_rx.recv() {
                let done = match work {
                    Work::Stop => break,
                    Work::Capture(expected) => Done::Captured(clipboard::capture_for_sequence(
                        HWND(handle as *mut _),
                        8 * 1024 * 1024,
                        expected,
                    )),
                };
                if capture_done_tx.send(done).is_err() {
                    break;
                }
                post(handle, WORK_WAKE);
            }
            CoUninitialize();
        }));
        let notify = Arc::new(move || post(handle, WORK_WAKE));
        app.preview_worker = Some(BackgroundWorker::new(prepare_preview, notify.clone()));
        app.action_worker = Some(BackgroundWorker::new(native_job, notify.clone()));
        app.screenshot_worker = Some(BackgroundWorker::new(encode_screenshot, notify.clone()));
        app.path_monitor = Some(PathMonitor::new(notify));
        AddClipboardFormatListener(hwnd).map_err(|e| e.to_string())?;
        app.tray(NIM_ADD);
        DragAcceptFiles(hwnd, true);
        app.capture();
        drop(app);
        host.drain_deferred();
        host.drain_effects();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if host.is_panel_surface(msg.hwnd) && host.route_key(&msg) {
                host.drain_effects();
                continue;
            }
            if !host.is_panel_surface(msg.hwnd)
                || host.composing.get()
                || !IsDialogMessageW(hwnd, &msg).as_bool()
            {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            host.drain_deferred();
            host.drain_effects();
        }
        let mut app = host.app.borrow_mut();
        let _ = app.tx.send(Work::Stop);
        if let Some(worker) = app.capture_worker.take() {
            let _ = worker.join();
        }
        app.action_permit.cancel();
        app.preview_worker.take();
        app.action_worker.take();
        app.path_monitor.take();
        CoUninitialize();
        Ok(())
    }
}
impl App {
    unsafe fn update_organizer_timer(&mut self) {
        self.organizer_progress.clear();
        self.update_organizer_label();
        self.refresh_organizer_rows();
        self.layout();
        let warning = if self.settings.organizer.enabled {
            crate::organizer_ui::organizer_config_warning(&self.settings.ai)
        } else {
            None
        };
        {
            let mut review = self.organizer_review.borrow_mut();
            review.config_warning = warning.clone();
            review.revision += 1;
        }
        self.organizer_tick_pending = false;
        let _ = KillTimer(Some(self.hwnd), ORGANIZER_TIMER);
        if self.settings.organizer.enabled
            && warning.is_none()
            && self.state.lifecycle == Lifecycle::Running
        {
            SetTimer(
                Some(self.hwnd),
                ORGANIZER_TIMER,
                self.settings.organizer.interval_seconds.clamp(30, 3600) * 1000,
                None,
            );
        }
        if self.organizer_page_open {
            self.organizer_snapshot();
        }
    }
    fn request_organizer_progress(&mut self) {
        if self.state.visible
            && self.settings.organizer.enabled
            && self.state.lifecycle == Lifecycle::Running
            && let Some(request_id) = self.organizer_progress.request()
        {
            self.send(Command::OrganizerProgress { request_id });
        }
    }
    unsafe fn update_organizer_label(&self) {
        if self.organizer_label.is_invalid() {
            return;
        }
        let text = if self.settings.organizer.enabled
            && crate::organizer_ui::organizer_config_warning(&self.settings.ai).is_some()
        {
            "AI 管家 · 检查配置".into()
        } else {
            self.organizer_progress.snapshot().label()
        };
        let _ = SetWindowTextW(self.organizer_label, PCWSTR(wide(&text).as_ptr()));
        let _ = ShowWindow(
            self.organizer_label,
            if self.settings.organizer.enabled {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
    }
    /// Update only affected native rows at their existing indices. Never search,
    /// reorder, change focus, reload the preview, or replace an editor's contents.
    unsafe fn refresh_organizer_rows(&self) {
        let Some(host) = self.host.as_ref() else {
            return;
        };
        let previous = host.render.borrow().clone();
        self.sync_render();
        let current = host.render.borrow().clone();
        let changed: Vec<_> = current
            .row_details
            .iter()
            .enumerate()
            .filter(|(i, row)| {
                previous.row_details.get(*i).is_some_and(|old| {
                    old.title != row.title
                        || old.text != row.text
                        || old.organizer_stage != row.organizer_stage
                })
            })
            .collect();
        if changed.is_empty() {
            return;
        }
        let top = SendMessageW(self.list, LB_GETTOPINDEX, None, None);
        let selected = SendMessageW(self.list, LB_GETCURSEL, None, None);
        SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(0)), None);
        for (index, row) in &changed {
            let text = palette_row_text(row);
            // Keep native text in sync for keyboard/type-ahead and accessibility.
            SendMessageW(self.list, LB_DELETESTRING, Some(WPARAM(*index)), None);
            SendMessageW(
                self.list,
                LB_INSERTSTRING,
                Some(WPARAM(*index)),
                Some(LPARAM(text.as_ptr() as isize)),
            );
        }
        SendMessageW(
            self.list,
            LB_SETCURSEL,
            Some(WPARAM(selected.0 as usize)),
            None,
        );
        SendMessageW(
            self.list,
            LB_SETTOPINDEX,
            Some(WPARAM(top.0.max(0) as usize)),
            None,
        );
        SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(1)), None);
        for (index, _) in changed {
            let mut rect = RECT::default();
            if SendMessageW(
                self.list,
                LB_GETITEMRECT,
                Some(WPARAM(index)),
                Some(LPARAM((&mut rect as *mut RECT) as isize)),
            )
            .0 != LB_ERR as isize
            {
                let _ = InvalidateRect(Some(self.list), Some(&rect), false);
            }
        }
    }
    unsafe fn organizer_metadata_updated(
        &mut self,
        updates: &[crate::organizer_display::MetadataUpdate],
    ) {
        if updates
            .iter()
            .any(|u| matches!(u.source, crate::organizer::Source::SavedItem { .. }))
        {
            self.semantic_status.mark_dirty();
            self.update_index_label();
        }
        for update in updates {
            for item in &mut self.saved_items {
                update.apply_saved(item);
            }
            for path in &mut self.suggested_items {
                update.apply_path(path);
            }
        }
        // Keep pending saved/path replies alive; they may be the user's current
        // query. Their results keep the visible order until the next search.
        self.organizer_preserve_order |= self.saved_ready;
        if palette::apply_organizer_metadata(&mut self.items, updates) {
            self.refresh_organizer_rows();
        }
    }
    fn cancel_organizer_for_settings(&mut self, next: &Settings) {
        if let Some(_id) = self
            .organizer_session
            .settings_changed(&self.settings, next)
            && let Some(worker) = &self.organizer_worker
        {
            worker.cancel();
        }
    }
    fn organizer_snapshot(&self) {
        let request_id = self.organizer_review.borrow_mut().request_snapshot();
        if let Some(request_id) = request_id {
            self.send(Command::OrganizerSnapshot { request_id });
        }
    }
    fn organizer_tick(&mut self) {
        if self.settings.organizer.enabled
            && self.state.lifecycle == Lifecycle::Running
            && !self.organizer_tick_pending
            && !self.organizer_worker.as_ref().is_some_and(|w| w.busy())
        {
            let request_id = self.organizer_review.borrow_mut().request_snapshot();
            let Some(request_id) = request_id else { return };
            self.organizer_tick_pending = true;
            self.send(Command::OrganizerTick);
            // FIFO local barrier acknowledges even an empty/cooling tick, avoiding
            // a queue of timer commands while the actor is busy.
            self.send(Command::OrganizerSnapshot { request_id });
        }
    }
    unsafe fn organizer_start(&mut self, work: crate::organizer::Work) {
        if self.state.lifecycle != Lifecycle::Running
            || work.ai_settings != self.settings.ai
            || work.organizer_settings != self.settings.organizer
        {
            return;
        }
        self.ensure_harness_worker();
        let token = work.token;
        match self.organizer_worker.as_ref().unwrap().start(work) {
            Ok(()) => self.organizer_session.begin(token, token),
            Err(message) => self.send(Command::HarnessFailed {
                token,
                code: "RUNTIME_UNAVAILABLE".into(),
                message,
                retryable: false,
            }),
        }
    }
    fn ensure_harness_worker(&mut self) {
        if self
            .organizer_worker
            .as_ref()
            .is_some_and(|worker| worker.retired_and_idle())
        {
            self.organizer_worker = None;
        }
        if self.organizer_worker.is_none() {
            let address = self.hwnd.0 as usize;
            let paths = crate::harness_worker::RuntimePaths::bundled().unwrap_or_else(|_| {
                crate::harness_worker::RuntimePaths {
                    node: Default::default(),
                    module: Default::default(),
                }
            });
            self.organizer_worker = Some(OrganizerWorker::new(
                paths,
                ai_client::harness_model,
                Arc::new(move || unsafe {
                    let _ = PostMessageW(
                        Some(HWND(address as *mut _)),
                        WORK_WAKE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }),
            ));
        }
    }
    fn memory_snapshot(&self) {
        let command = self.harness_memory.borrow_mut().request_snapshot();
        if let Some(command) = command {
            self.send(command);
        }
    }
    unsafe fn show_harness_memory(&mut self) {
        self.memory_snapshot();
        self.effects.push_back(UiEffect::HarnessMemory {
            owner: self.hwnd,
            state: self.harness_memory.clone(),
            outbox: self.organizer_outbox.clone(),
            appearance: self.settings.theme,
        });
    }
    unsafe fn organizer_settings(&mut self) {
        self.effects.push_back(UiEffect::OrganizerSettings {
            owner: self.hwnd,
            settings: self.settings.organizer.clone(),
        });
    }
    unsafe fn review_organizer(&mut self) {
        if self.organizer_page_open {
            return;
        }
        self.organizer_page_open = true;
        self.organizer_review.borrow_mut().open();
        self.organizer_snapshot();
        self.effects.push_back(UiEffect::OrganizerReview {
            owner: self.hwnd,
            state: self.organizer_review.clone(),
            outbox: self.organizer_outbox.clone(),
            appearance: self.settings.theme,
            enabled: self.settings.organizer.enabled,
        });
    }
    fn sync_render(&self) {
        unsafe {
            if let Some(host) = self.host.as_ref() {
                let previous = host.render.borrow().clone();
                let (row_details, saved_times) = if previous.items == self.items
                    && previous.query == self.query
                    && previous.organizer_revision == self.organizer_progress.revision()
                {
                    (previous.row_details.clone(), previous.saved_times.clone())
                } else {
                    let details = palette::row_details_with_progress(
                        &self.items,
                        &self.query,
                        self.organizer_progress.snapshot(),
                    );
                    let times = details
                        .iter()
                        .map(|detail| {
                            detail
                                .saved_at_ms
                                .map(local_saved_time)
                                .or_else(|| {
                                    detail
                                        .visited_at_ms
                                        .map(|time| format!("访 {}", local_saved_time(time)))
                                })
                                .unwrap_or_default()
                        })
                        .collect();
                    (Rc::new(details), Rc::new(times))
                };
                *host.render.borrow_mut() = Rc::new(RenderState {
                    dpi: self.dpi,
                    colors: self.colors.clone(),
                    font: self.font,
                    saved_times,
                    row_details,
                    organizer_revision: self.organizer_progress.revision(),
                    query: self.query.clone(),
                    items: self.items.clone(),
                    image: self.image.clone(),
                });
            }
        }
    }
    fn cancel_actions(&mut self) {
        self.action_permit.cancel();
        self.load_id = self.load_id.wrapping_add(1);
        self.pending_action = None;
        self.pending_paste = None;
        self.state.target_changed();
    }
    fn smart_view(&self) -> SmartView {
        SmartView {
            query: unsafe { dialogs::text(self.edit) },
            scope: self.scope,
            profile: semantic::profile_id(&self.settings.embedding).unwrap_or_default(),
            preferred: self.smart_mode.unwrap_or(self.settings.smart_search),
            ai_enabled: self.settings.ai.enabled,
            embedding_enabled: self.settings.embedding.enabled,
        }
    }
    unsafe fn smart_button_label(&self) {
        let active = self.smart_preflight.busy()
            || self.ai_gate.busy()
            || self.ai_plan.is_some()
            || self.semantic_query.busy()
            || self.semantic_query.adopted();
        let _ = SetWindowTextW(
            self.smart_button,
            if active {
                w!("取消智能")
            } else {
                w!("智能查找")
            },
        );
    }
    unsafe fn cancel_preflight(&mut self) {
        self.smart_preflight.cancel();
        self.smart_mode = None;
        self.smart_button_label();
    }
    unsafe fn update_index_label(&self) {
        let profile = semantic::profile_id(&self.settings.embedding).unwrap_or_default();
        let message = if self.semantic_index.active() {
            format!(
                "索引更新中 · 已接收 {} 条 · 点击取消",
                self.semantic_index.accepted()
            )
        } else if let Some(status) = self.semantic_status.ready(&profile) {
            if status.indexed == 0 {
                "未建立可用索引 · 建立索引".into()
            } else if status.needs_update {
                format!("已索引 {} 条 · 待更新 · 更新索引", status.indexed)
            } else {
                format!("已索引 {} 条 · 更新索引", status.indexed)
            }
        } else {
            "索引状态待刷新 · 建立/更新索引".into()
        };
        let text = wide(&message);
        let _ = SetWindowTextW(self.index_label, PCWSTR(text.as_ptr()));
    }
    unsafe fn refresh_semantic_status(&mut self) {
        if self.state.lifecycle != Lifecycle::Running
            || !self.settings.embedding.enabled
            || self.semantic_index.active() && !self.smart_preflight.busy()
        {
            return;
        }
        let profile = semantic::profile_id(&self.settings.embedding).unwrap_or_default();
        if let Some(command) = self.semantic_status.request(&profile) {
            let Command::SemanticStatus { request_id } = command else {
                unreachable!()
            };
            if let Some(engine) = &self.engine {
                if let Err(message) = engine.send(Command::SemanticStatus { request_id }) {
                    self.semantic_status.fail(request_id);
                    self.cancel_preflight();
                    self.status(&message);
                }
            } else {
                self.semantic_status.fail(request_id);
                self.cancel_preflight();
            }
        }
        self.update_index_label();
    }
    unsafe fn index_guidance(&self) {
        self.update_index_label();
        self.status("尚未建立可用索引；点击“建立索引”后再智能查找");
        let _ = SetFocus(Some(self.index_label));
    }
    unsafe fn begin_smart(&mut self, preferred: Option<SmartSearchMode>) {
        if !self.state.visible
            || self.state.modal_open
            || self.state.lifecycle != Lifecycle::Running
        {
            return;
        }
        if self.smart_preflight.busy()
            || self.ai_gate.busy()
            || self.ai_plan.is_some()
            || self.semantic_query.busy()
            || self.semantic_query.adopted()
        {
            self.cancel_preflight();
            self.cancel_semantic();
            self.cancel_ai();
            self.search();
            self.smart_button_label();
            self.status("已取消智能查找，恢复本地检索");
            return;
        }
        self.smart_mode = preferred;
        let view = self.smart_view();
        if !view.embedding_enabled {
            let action = crate::smart_search::choose(&view, None);
            if !matches!(action, SmartAction::Claude) {
                self.run_smart_action(action);
                return;
            }
        }
        if [
            "智能查找",
            "智能检索",
            "智能搜索",
            "按意思查找",
            "Claude 按意思查找",
            "AI检索",
            "AI搜索",
            "语义查找",
            "语义搜索",
            "向量检索",
            "语义查找（已建索引）",
        ]
        .contains(&view.query.trim())
        {
            self.effects.push_back(UiEffect::SmartPrompt {
                owner: self.hwnd,
                preferred,
            });
            return;
        }
        if view.query.trim().is_empty() || view.query.chars().count() > semantic::MAX_INPUT_CHARS {
            self.status("请输入不超过 4096 字的检索描述");
            return;
        }
        if view.embedding_enabled && view.profile.is_empty() {
            if view.preferred != SmartSearchMode::Semantic && view.ai_enabled {
                self.begin_ai();
            } else {
                self.embedding_settings();
            }
            return;
        }
        self.cancel_actions();
        self.smart_preflight.begin(view);
        self.smart_button_label();
        self.refresh_semantic_status();
        self.finish_smart_preflight();
        if self.smart_preflight.busy() {
            self.status("正在检查本地索引…点击智能查找可取消");
        }
    }
    unsafe fn finish_smart_preflight(&mut self) {
        let view = self.smart_view();
        let active = self.state.visible
            && !self.state.modal_open
            && self.state.lifecycle == Lifecycle::Running
            && GetForegroundWindow() == self.hwnd;
        if let Some(action) =
            self.smart_preflight
                .finish(&view, active, self.semantic_status.ready(&view.profile))
        {
            self.smart_mode = None;
            self.run_smart_action(action);
        }
        self.smart_button_label();
    }
    unsafe fn run_smart_action(&mut self, action: SmartAction) {
        match action {
            SmartAction::Semantic => self.begin_semantic(),
            SmartAction::Claude => self.begin_ai(),
            SmartAction::BuildIndex => self.index_guidance(),
            SmartAction::ConfigureClaude => self.ai_settings(),
            SmartAction::ConfigureSemantic => self.embedding_settings(),
            SmartAction::Configure => {
                let mut rect = RECT::default();
                let _ = GetWindowRect(self.smart_button, &mut rect);
                self.effects.push_back(UiEffect::Menu {
                    owner: self.hwnd,
                    x: rect.left,
                    y: rect.bottom,
                    target: None,
                    entries: vec![
                        (AI_SETTINGS, "配置按意思查找（Claude）…"),
                        (EMBEDDING_SETTINGS, "配置相似内容查找（Embedding）…"),
                    ],
                });
            }
        }
    }
    unsafe fn stop_ai_request(&mut self) {
        if let Some(worker) = self.ai_worker.as_ref() {
            worker.cancel();
        }
        self.ai_gate.cancel();
        self.smart_button_label();
    }
    unsafe fn cancel_ai(&mut self) {
        self.stop_ai_request();
        if revoke_adopted_plan(
            &mut self.ai_plan,
            &mut self.state,
            &mut self.saved_items,
            &mut self.saved_ready,
        ) {
            self.cancel_actions();
            self.items.clear();
            self.clear_preview();
            self.rebuild_results();
        }
        self.smart_button_label();
    }
    unsafe fn begin_ai(&mut self) {
        if self.state.lifecycle != Lifecycle::Running
            || self.state.modal_open
            || !self.state.visible
        {
            return;
        }
        self.cancel_semantic();
        self.cancel_actions();
        if !self.settings.ai.enabled {
            self.ai_settings();
            return;
        }
        let query = dialogs::text(self.edit);
        if query.trim().is_empty() || query.chars().count() > 2048 {
            self.status("请输入不超过 2048 字的检索描述");
            return;
        }
        // Switching from adopted semantic rows also refreshes ordinary results.
        self.cancel_ai();
        self.search();
        if self.ai_worker.is_none() {
            let handle = self.hwnd.0 as usize;
            self.ai_worker = Some(AiWorker::start(
                |request| {
                    ai_client::interpret(
                        &request.settings,
                        &request.query,
                        request.scope,
                        &request.functions,
                    )
                },
                Arc::new(move || {
                    post(handle, WORK_WAKE);
                }),
            ));
        }
        let functions = palette::ai_catalog(PaletteContext {
            capture_paused: self.settings.capture_paused,
            preview_open: self.preview_open,
            content_ready: self.saved_ready,
            ai_enabled: true,
            embedding_enabled: self.settings.embedding.enabled,
            indexing: self.semantic_index.active(),
        });
        let request = AiRequest {
            settings: self.settings.ai.clone(),
            query: query.clone(),
            scope: self.scope,
            functions,
        };
        match self.ai_worker.as_ref().unwrap().submit(request) {
            Ok(id) => {
                self.ai_gate.begin(id, &query, self.scope);
                self.smart_button_label();
                self.status("智能查找正在理解查询…本地结果仍可使用");
            }
            Err(_) => self.local_search_status(),
        }
    }
    unsafe fn ai_done(&mut self) {
        let Some(completion) = self.ai_worker.as_ref().and_then(AiWorker::try_result) else {
            return;
        };
        let active = self.state.visible
            && !self.state.modal_open
            && self.state.lifecycle == Lifecycle::Running
            && self.settings.ai.enabled;
        if !self
            .ai_gate
            .finish(completion.id, &dialogs::text(self.edit), self.scope, active)
        {
            return;
        }
        self.smart_button_label();
        match claude_reply(
            completion.result,
            &mut self.ai_plan,
            &mut self.saved_items,
            &mut self.saved_ready,
        ) {
            QueryReply::Applied => self.search(),
            QueryReply::LocalFallback => self.local_search_status(),
            QueryReply::Ignored => {}
        }
        self.smart_button_label();
    }
    unsafe fn cancel_semantic(&mut self) {
        self.prepare_semantic_effect(false);
    }
    unsafe fn prepare_semantic_effect(&mut self, preserve_results: bool) -> bool {
        if let Some(worker) = &self.query_worker {
            worker.cancel();
        }
        let revoked = prepare_semantic_effect(
            &mut self.semantic_query,
            &mut self.state,
            &mut self.saved_items,
            &mut self.saved_ready,
            preserve_results,
        );
        if revoked {
            self.cancel_actions();
            self.items.clear();
            self.clear_preview();
            self.rebuild_results();
        }
        self.smart_button_label();
        revoked
    }
    unsafe fn restore_after_modal(&mut self, revoked: bool) {
        let text = dialogs::text(self.edit);
        if let Some(query) = finish_semantic_effect_with_revoke(
            &mut self.semantic_query,
            &mut self.state,
            &mut self.saved_items,
            &mut self.saved_ready,
            (&text, self.scope, revoked),
            |state, saved, ready| {
                revoke_adopted_plan(&mut self.ai_plan, state, saved, ready);
            },
        ) {
            self.stop_ai_request();
            self.query = text;
            self.result_scope = self.scope;
            self.cancel_actions();
            self.clear_preview();
            self.rebuild_results();
            self.path_batches.begin(query.request_id);
            self.suggested_items.clear();
            self.send(Command::Search(query));
        }
    }
    fn embedding_worker(&self) -> EmbeddingWorker {
        let handle = self.hwnd.0 as usize;
        EmbeddingWorker::start(
            |request| ai_client::embed(&request.settings, &request.inputs),
            Arc::new(move || unsafe { post(handle, WORK_WAKE) }),
        )
    }
    unsafe fn begin_semantic(&mut self) {
        if self.state.lifecycle != Lifecycle::Running
            || self.state.modal_open
            || !self.state.visible
        {
            return;
        }
        self.cancel_ai();
        if !self.settings.embedding.enabled {
            self.embedding_settings();
            return;
        }
        let query = dialogs::text(self.edit);
        if query.trim().is_empty() || query.chars().count() > semantic::MAX_INPUT_CHARS {
            self.status("请输入不超过 4096 字的语义查询");
            return;
        }
        let profile = match semantic::profile_id(&self.settings.embedding) {
            Ok(profile) => profile,
            Err(_) => {
                self.local_search_status();
                return;
            }
        };
        if self
            .semantic_status
            .ready(&profile)
            .is_none_or(|s| s.indexed == 0)
        {
            self.index_guidance();
            return;
        }
        if self.query_worker.is_none() {
            self.query_worker = Some(self.embedding_worker());
        }
        // Return immediately to ordinary results while the query vector is computed.
        self.search();
        match self
            .query_worker
            .as_ref()
            .unwrap()
            .submit(EmbeddingRequest {
                settings: self.settings.embedding.clone(),
                inputs: vec![query.clone()],
            }) {
            Ok(id) => {
                self.semantic_query.begin(id, &query, self.scope, &profile);
                self.smart_button_label();
                self.status("语义查询处理中…普通结果仍可使用");
            }
            Err(_) => self.local_search_status(),
        }
    }
    unsafe fn embedding_done(&mut self) {
        let profile = semantic::profile_id(&self.settings.embedding).unwrap_or_default();
        if let Some(completion) = self
            .query_worker
            .as_ref()
            .and_then(EmbeddingWorker::try_result)
        {
            let query = dialogs::text(self.edit);
            let active = self.settings.embedding.enabled
                && self.state.visible
                && !self.state.modal_open
                && self.state.lifecycle == Lifecycle::Running;
            match embedding_reply(
                completion.result,
                &mut self.semantic_query,
                completion.id,
                (&query, self.scope, &profile, active),
            ) {
                QueryReply::Applied => {
                    self.saved_items.clear();
                    self.saved_ready = false;
                    self.clear_preview();
                    self.search();
                    self.smart_button_label();
                }
                QueryReply::LocalFallback => {
                    self.smart_button_label();
                    self.local_search_status();
                }
                QueryReply::Ignored => {}
            }
        }
        if let Some(completion) = self
            .index_worker
            .as_ref()
            .and_then(EmbeddingWorker::try_result)
        {
            match index_reply(
                completion.result,
                &mut self.semantic_index,
                completion.id,
                &profile,
            ) {
                IndexReply::Store(command) => self.send_index(*command),
                IndexReply::LocalFallback => {
                    self.cancel_index();
                    self.local_search_status();
                }
                IndexReply::Ignored => {}
            }
        }
    }
    unsafe fn send_index(&mut self, command: Command) {
        if let Some(engine) = &self.engine
            && let Err(message) = engine.send(command)
        {
            self.cancel_index();
            self.status(&message);
        }
    }
    unsafe fn begin_index(&mut self) {
        if self.state.lifecycle != Lifecycle::Running
            || !self.settings.embedding.enabled
            || self.semantic_index.active()
        {
            return;
        }
        let profile = match semantic::profile_id(&self.settings.embedding) {
            Ok(profile) => profile,
            Err(_) => {
                self.local_search_status();
                return;
            }
        };
        if self.index_worker.is_none() {
            self.index_worker = Some(self.embedding_worker());
        }
        let command = self.semantic_index.begin(&profile);
        self.send_index(command);
        self.update_index_label();
        self.rebuild_results();
        self.status("正在建立语义索引…仅发送选定范围的保存文本和元数据");
    }
    unsafe fn cancel_index(&mut self) {
        if let Some(worker) = &self.index_worker {
            worker.cancel();
        }
        self.semantic_index.cancel();
        self.update_index_label();
        self.rebuild_results();
    }
    unsafe fn index_status(&mut self) {
        if !self.semantic_index.active() {
            self.refresh_semantic_status();
        }
        self.update_index_label();
        self.rebuild_results();
        self.status(&format!(
            "语义索引{} · 已接收 {} 条",
            if self.semantic_index.active() {
                "更新中"
            } else {
                "完成"
            },
            self.semantic_index.accepted()
        ));
    }
    unsafe fn index_batch(&mut self, batch: semantic::IndexBatch) {
        if self.state.lifecycle != Lifecycle::Running || !self.settings.embedding.enabled {
            return;
        }
        let Some(inputs) = self.semantic_index.accept_batch(batch) else {
            return;
        };
        if inputs.is_empty() {
            if let Some(command) = self.semantic_index.empty_page() {
                self.send_index(command);
            } else {
                self.index_status();
            }
            return;
        }
        let Some(worker) = &self.index_worker else {
            self.cancel_index();
            return;
        };
        match worker.submit(EmbeddingRequest {
            settings: self.settings.embedding.clone(),
            inputs,
        }) {
            Ok(id) => self.semantic_index.network_started(id),
            Err(_) => {
                self.cancel_index();
                self.local_search_status();
            }
        }
    }
    unsafe fn embedding_settings(&mut self) {
        self.cancel_semantic();
        self.cancel_ai();
        self.effects.push_back(UiEffect::EmbeddingSettings {
            owner: self.hwnd,
            settings: self.settings.embedding.clone(),
        });
    }
    unsafe fn apply_embedding_settings(&mut self, mut values: Vec<String>) -> Result<(), String> {
        let password = ai_client::SecretString(std::mem::take(&mut values[3]));
        if self.state.lifecycle != Lifecycle::Running {
            return Ok(());
        }
        let mut next = self.settings.embedding.clone();
        next.enabled = values[0] == "1";
        next.endpoint = values[1].trim().into();
        next.model = values[2].trim().into();
        next.index_scope = if values[5] == "1" {
            IndexScope::AllText
        } else {
            IndexScope::PinnedAndFavorites
        };
        if values[4] == "1" {
            if !password.0.is_empty() {
                return Err("清除密钥与填写新密钥请选择其中一个操作".into());
            }
            next.encrypted_key.clear();
            next.enabled = false;
        } else if !password.0.is_empty() {
            if password.0.len() > 4096 || !password.0.bytes().all(|byte| (33..=126).contains(&byte))
            {
                return Err("Embedding API Key 格式无效，配置未保存".into());
            }
            next.encrypted_key = ai_client::protect_key(&password.0)?;
        }
        if next.enabled {
            ai_client::validate_embedding_settings(&next)
                .map_err(|message| format!("{message}，语义配置未保存"))?;
        } else if !next.endpoint.is_empty()
            && crate::embedding::parse_endpoint(&next.endpoint).is_err()
        {
            return Err("Embedding API 地址无效，配置未保存".into());
        }
        self.cancel_semantic();
        self.cancel_index();
        self.cancel_preflight();
        self.semantic_status.mark_dirty();
        self.settings.embedding = next;
        self.send(Command::SaveSettings(self.settings.clone()));
        self.retheme();
        Ok(())
    }
    unsafe fn preview_unavailable(&mut self, message: &str) {
        self.preview_loading = false;
        self.image = None;
        self.sync_render();
        let text = wide(message);
        let _ = SetWindowTextW(self.preview, PCWSTR(text.as_ptr()));
        let _ = InvalidateRect(Some(self.preview), None, true);
    }
    unsafe fn clear_preview(&mut self) {
        self.load_id = self.load_id.wrapping_add(1);
        self.preview_id = self.load_id;
        self.image = None;
        self.preview_loading = false;
        self.preview_loads.cancel();
        if let Some(worker) = &self.preview_worker {
            worker.cancel_pending();
        }
        self.sync_render();
        let _ = SetWindowTextW(self.preview, w!(""));
        let _ = InvalidateRect(Some(self.preview), None, true);
    }

    fn send(&self, c: Command) {
        if let Some(e) = &self.engine
            && let Err(error) = e.send(c)
        {
            unsafe {
                self.status(&error);
            }
        }
    }
    fn s(&self, n: i32) -> i32 {
        n * self.dpi as i32 / 96
    }
    unsafe fn control(
        &self,
        class: PCWSTR,
        title: &str,
        style: WINDOW_STYLE,
        id: usize,
    ) -> Result<HWND, String> {
        let t = wide(title);
        CreateWindowExW(
            if id == SEARCH {
                WS_EX_CLIENTEDGE
            } else {
                WINDOW_EX_STYLE::default()
            },
            class,
            PCWSTR(t.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            10,
            10,
            Some(self.hwnd),
            Some(HMENU(id as *mut _)),
            None,
            None,
        )
        .map_err(|e| e.to_string())
    }
    unsafe fn create_controls(&mut self) -> Result<(), String> {
        self.edit = self.control(
            w!("EDIT"),
            "",
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            SEARCH,
        )?;
        let _ = SendMessageW(
            self.edit,
            EM_SETCUEBANNER,
            Some(WPARAM(0)),
            Some(LPARAM(
                w!("搜索内容或功能，如 周报、设置、添加文件夹…").0 as isize,
            )),
        );
        self.list = self.control(
            w!("LISTBOX"),
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE(
                    (LBS_NOTIFY | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS | LBS_NOINTEGRALHEIGHT)
                        as u32,
                ),
            LIST,
        )?;
        self.footer = self.control(
            w!("STATIC"),
            "↑↓ 选择  Enter 执行  Esc 关闭",
            WINDOW_STYLE::default(),
            109,
        )?;
        self.preview = self.control(
            w!("EDIT"),
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32),
            112,
        )?;
        self.smart_button = self.control(w!("BUTTON"), "智能查找", WS_TABSTOP, SMART_FIND)?;
        self.index_label = self.control(
            w!("BUTTON"),
            "索引状态待刷新",
            WS_TABSTOP | WINDOW_STYLE((BS_FLAT | BS_LEFT) as u32),
            INDEX_STATUS,
        )?;
        self.organizer_label = self.control(
            w!("BUTTON"),
            "AI 管家",
            WS_TABSTOP | WINDOW_STYLE((BS_FLAT | BS_LEFT) as u32),
            ORGANIZER_REVIEW,
        )?;
        let _ = ShowWindow(self.preview, SW_HIDE);
        for (id, title) in [
            (ADD, "添加 ＋"),
            (PREVIEW, "预览 ›"),
            (SETTINGS, "设置 ▾"),
            (300, "全部"),
            (301, "剪贴板"),
            (302, "文件夹"),
            (303, "文档"),
        ] {
            self.buttons.push(self.control(
                w!("BUTTON"),
                title,
                WS_TABSTOP
                    | if (300..=303).contains(&id) {
                        WINDOW_STYLE((BS_AUTORADIOBUTTON | BS_PUSHLIKE) as u32)
                            | if id == 300 {
                                WS_GROUP
                            } else {
                                WINDOW_STYLE::default()
                            }
                    } else {
                        WINDOW_STYLE::default()
                    },
                id,
            )?);
        }
        let _ = SetWindowSubclass(self.edit, Some(edit_proc), 1, self.host as usize);
        let _ = SetWindowSubclass(self.list, Some(edit_proc), 2, self.host as usize);
        let _ = SetWindowSubclass(self.preview, Some(preview_proc), 3, self.host as usize);
        self.retheme();
        self.layout();
        Ok(())
    }
    unsafe fn retheme(&mut self) {
        if !self.font.is_invalid() {
            let _ = DeleteObject(self.font.into());
        }
        self.font = theme::font(self.dpi);
        self.colors = Rc::new(theme::Colors::new(self.settings.theme));
        self.sync_render();
        theme::apply(self.hwnd, self.colors.dark);
        for h in [self.edit, self.list, self.footer, self.preview]
            .into_iter()
            .chain(self.buttons.iter().copied())
            .chain([self.smart_button, self.index_label, self.organizer_label])
        {
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        let _ = ShowWindow(self.smart_button, SW_SHOW);
        let _ = ShowWindow(
            self.index_label,
            if self.settings.embedding.enabled {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
        self.update_index_label();
        self.update_organizer_label();
        self.smart_button_label();
        self.layout();
        SendMessageW(
            self.list,
            LB_SETITEMHEIGHT,
            Some(WPARAM(0)),
            Some(LPARAM(self.s(62) as isize)),
        );
        let _ = InvalidateRect(Some(self.hwnd), None, true);
    }
    unsafe fn layout(&self) {
        if self.buttons.len() < 7 || self.edit.is_invalid() || self.list.is_invalid() {
            return;
        }
        let mut r = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut r);
        let w = r.right;
        let h = r.bottom;
        let edge = self.s(18);
        let preview_width = if self.preview_open { self.s(250) } else { 0 };
        let list_width = w - 2 * edge - preview_width;
        let mov = |hwnd: HWND, x, y, width, height| {
            let _ = MoveWindow(hwnd, x, y, width, height, true);
        };
        mov(
            self.edit,
            edge,
            self.s(18),
            (w - self.s(240)).max(self.s(80)),
            self.s(36),
        );
        mov(
            self.smart_button,
            w - self.s(212),
            self.s(18),
            self.s(106),
            self.s(36),
        );
        mov(
            self.buttons[0],
            w - self.s(98),
            self.s(18),
            self.s(80),
            self.s(36),
        );
        for i in 0..4 {
            mov(
                self.buttons[3 + i],
                edge + self.s(i as i32 * 95),
                self.s(66),
                self.s(89),
                self.s(30),
            );
        }
        mov(
            self.buttons[2],
            w - self.s(80),
            self.s(66),
            self.s(62),
            self.s(30),
        );
        let index_height = if self.settings.embedding.enabled {
            self.s(30)
        } else {
            0
        };
        mov(self.index_label, edge, self.s(102), list_width, self.s(26));
        let organizer_height = if self.settings.organizer.enabled {
            self.s(30)
        } else {
            0
        };
        mov(
            self.organizer_label,
            edge,
            self.s(102) + index_height,
            list_width,
            self.s(26),
        );
        mov(
            self.list,
            edge,
            self.s(110) + index_height + organizer_height,
            list_width,
            h - self.s(165) - index_height - organizer_height,
        );
        mov(
            self.footer,
            edge,
            h - self.s(42),
            w - self.s(120),
            self.s(30),
        );
        mov(
            self.buttons[1],
            w - self.s(94),
            h - self.s(44),
            self.s(76),
            self.s(30),
        );
        mov(
            self.preview,
            w - preview_width,
            self.s(114),
            preview_width - edge,
            h - self.s(180),
        );
    }
    unsafe fn status(&self, message: &str) {
        let text = wide(message);
        let _ = SetWindowTextW(self.footer, PCWSTR(text.as_ptr()));
    }
    unsafe fn local_search_status(&self) {
        self.status("本地检索 · ↑↓ 选择  Enter 打开/粘贴");
    }
    unsafe fn tray(&self, operation: NOTIFY_ICON_MESSAGE) {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: TRAY,
            hIcon: LoadIconW(
                GetModuleHandleW(None).ok().map(Into::into),
                PCWSTR(101usize as *const u16),
            )
            .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default()),
            ..Default::default()
        };
        let key = format_hotkey(self.settings.hotkey_modifiers, self.settings.hotkey_key);
        let title = wide(&if key.is_empty() {
            "快捷面板 · 点击打开".into()
        } else {
            format!("快捷面板 · {key}")
        });
        data.szTip[..title.len()].copy_from_slice(&title);
        let _ = Shell_NotifyIconW(operation, &data);
    }
    unsafe fn replace_hotkeys(&mut self, next: &Settings) -> Result<(), String> {
        let keys = settings_hotkeys(next)?;
        let hwnd = self.hwnd;
        self.hotkey_bindings.replace(
            keys,
            |id, key| {
                RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(key.modifiers) | MOD_NOREPEAT, key.key)
                    .map_err(|_| "快捷键注册失败，可能被占用或受系统限制。原快捷键保持不变，请在设置中换用其他组合或留空关闭。".into())
            },
            |id| { let _ = UnregisterHotKey(Some(hwnd), id); },
        )
    }
    unsafe fn show(&mut self, scope: Option<Scope>) {
        self.show_panel(scope, true);
    }
    unsafe fn show_panel(&mut self, scope: Option<Scope>, update_target: bool) {
        if !self.state.can_show_panel() {
            return;
        }
        self.screenshot_gate.cancel();
        let foreground = GetForegroundWindow();
        let mut process = 0u32;
        GetWindowThreadProcessId(foreground, Some(&mut process));
        let own_window = process == windows::Win32::System::Threading::GetCurrentProcessId();
        if update_target
            && !foreground.is_invalid()
            && self.state.may_update_target(own_window)
            && foreground != self.target
        {
            self.cancel_actions();
            self.clear_preview();
            self.target = foreground;
        }
        if let Some(s) = scope {
            self.scope = s;
        }
        self.state.visible = true;
        let monitor = MonitorFromWindow(self.target, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let mut r = RECT::default();
        let _ = GetWindowRect(self.hwnd, &mut r);
        let work = info.rcWork;
        let width = self
            .s(if self.preview_open { 930 } else { 680 })
            .min(work.right - work.left);
        let height = self.s(480).min(work.bottom - work.top);
        let _ = SetWindowPos(
            self.hwnd,
            Some(HWND_TOP),
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 3,
            width,
            height,
            SWP_SHOWWINDOW,
        );
        let _ = SetForegroundWindow(self.hwnd);
        let _ = SetFocus(Some(self.edit));
        if update_target && !own_window {
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        self.refresh_semantic_status();
        self.organizer_progress.mark_dirty();
        self.request_organizer_progress();
        self.search();
    }
    unsafe fn hide(&mut self) {
        // Selection hides the native panel only after setting state.visible=false.
        // Its resulting WA_INACTIVE must not revoke its own screenshot.
        if self.state.visible {
            self.screenshot_gate.cancel();
        }
        self.cancel_preflight();
        self.cancel_semantic();
        self.cancel_ai();
        self.cancel_actions();
        self.clear_preview();
        self.state.hide();
        self.organizer_progress.clear();
        let _ = ShowWindow(self.hwnd, SW_HIDE);
    }
    unsafe fn search(&mut self) {
        if self.state.lifecycle != Lifecycle::Running {
            return;
        }
        self.organizer_preserve_order = false;
        let current = match self.scope {
            Scope::All => 0,
            Scope::Clipboard => 1,
            Scope::Folder => 2,
            Scope::Document => 3,
        };
        for (i, button) in self.buttons.iter().skip(3).enumerate() {
            SendMessageW(
                *button,
                BM_SETCHECK,
                Some(WPARAM(usize::from(i == current))),
                None,
            );
        }
        let query = dialogs::text(self.edit);
        let profile = semantic::profile_id(&self.settings.embedding).unwrap_or_default();
        let active =
            self.settings.embedding.enabled && self.state.visible && !self.state.modal_open;
        if sync_semantic_view(
            &mut self.semantic_query,
            &mut self.state,
            &mut self.saved_items,
            &mut self.saved_ready,
            (&query, self.scope, &profile, active),
        ) {
            if let Some(worker) = &self.query_worker {
                worker.cancel();
            }
            self.items.clear();
            self.clear_preview();
            self.smart_button_label();
        }
        if query != self.query || self.scope != self.result_scope {
            self.cancel_preflight();
            self.cancel_actions();
            self.clear_preview();
            self.cancel_ai();
            self.cancel_semantic();
            self.saved_items.clear();
            self.saved_ready = false;
        }
        self.query = query;
        self.result_scope = self.scope;
        let id = self.state.next_refresh_request();
        self.path_batches.begin(id);
        if let Some(query) = self.semantic_query.local_query(id) {
            self.rebuild_results();
            self.send(Command::SemanticSearch(query));
            return;
        }
        if let Some(plan) = self.ai_plan.as_ref() {
            let (from_ms, until_ms) = match ai_time::period_bounds(plan.period) {
                Ok(bounds) => bounds,
                Err(message) => {
                    self.status(&message);
                    return;
                }
            };
            let scope = plan.scope.unwrap_or(self.scope);
            if plan.queries.is_empty() && scope == Scope::All && from_ms.is_none() {
                self.suggested_items.clear();
                self.saved_items.clear();
                self.saved_ready = true;
                self.rebuild_results();
                return;
            }
            let expanded = AiSearchQuery {
                queries: plan.queries.clone(),
                scope,
                from_ms,
                until_ms,
                request_id: id,
            };
            self.rebuild_results();
            self.send(Command::AiSearch(expanded));
            return;
        }
        self.rebuild_results();
        self.send(Command::Search(SearchQuery {
            text: self.query.clone(),
            scope: self.scope,
            request_id: id,
        }));
    }
    unsafe fn rebuild_results(&mut self) {
        let context = PaletteContext {
            capture_paused: self.settings.capture_paused,
            preview_open: self.preview_open,
            content_ready: self.saved_ready,
            ai_enabled: self.settings.ai.enabled,
            embedding_enabled: self.settings.embedding.enabled,
            indexing: self.semantic_index.active(),
        };
        if !self.saved_ready {
            self.suggested_items.clear();
        }
        let items = if let Some(plan) = self.ai_plan.as_ref() {
            palette::ai_results(plan, self.saved_items.clone(), context)
        } else if self.semantic_query.adopted() {
            palette::semantic_results(&self.query, self.saved_items.clone(), context)
        } else {
            palette::results(&self.query, self.saved_items.clone(), context)
        };
        let items = palette::with_suggestions(
            &self.query,
            items,
            if self.settings.path_activity.enabled {
                self.suggested_items.clone()
            } else {
                vec![]
            },
        );
        let items = if self.organizer_preserve_order {
            palette::preserve_visible_order(&self.items, items)
        } else {
            items
        };
        if self.items == items {
            self.sync_render();
            let _ = InvalidateRect(Some(self.list), None, false);
            self.selection_with_preview(self.organizer_preserve_order);
            return;
        }
        let selected_entry = self
            .state
            .selected_index()
            .and_then(|i| self.items.get(i))
            .cloned();
        let selected_content = selected_entry.as_ref().and_then(PaletteEntry::saved);
        let top_index = SendMessageW(self.list, LB_GETTOPINDEX, None, None).0.max(0) as usize;
        let top_id = self.items.get(top_index).map(|e| e.id().to_owned());
        self.items = items;
        let selected_before = self.state.selected_id().map(str::to_owned);
        self.state.replace_ids(
            self.items
                .iter()
                .map(|entry| entry.id().to_owned())
                .collect(),
        );
        let current_content = self
            .state
            .selected_index()
            .and_then(|i| self.items.get(i))
            .and_then(PaletteEntry::saved);
        let version_changed = selected_content
            .zip(current_content)
            .is_some_and(|(old, next)| !crate::preview::same_content_version(old, next));
        if selected_before.as_deref() != self.state.selected_id() || version_changed {
            self.cancel_actions();
        }
        self.sync_render();
        SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(self.list, LB_RESETCONTENT, None, None);
        let details = self
            .host
            .as_ref()
            .map(|host| host.render.borrow().row_details.clone());
        for (index, item) in self.items.iter().enumerate() {
            let title = details
                .as_ref()
                .and_then(|details| details.get(index))
                .map(palette_row_text)
                .unwrap_or_else(|| wide(item.title()));
            SendMessageW(
                self.list,
                LB_ADDSTRING,
                None,
                Some(LPARAM(title.as_ptr() as isize)),
            );
        }
        if let Some(index) = self.state.selected_index() {
            SendMessageW(self.list, LB_SETCURSEL, Some(WPARAM(index)), None);
        }
        let top = top_id
            .and_then(|id| self.items.iter().position(|e| e.id() == id))
            .unwrap_or(top_index.min(self.items.len().saturating_sub(1)));
        SendMessageW(self.list, LB_SETTOPINDEX, Some(WPARAM(top)), None);
        SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = InvalidateRect(Some(self.list), None, true);
        let keep_preview = self.organizer_preserve_order
            && palette::same_preview_source(
                selected_entry.as_ref(),
                self.state.selected_index().and_then(|i| self.items.get(i)),
            );
        self.selection_with_preview(keep_preview);
    }
    unsafe fn capture(&mut self) {
        let sequence = clipboard::sequence_number();
        if self.suppress_sequence == Some(sequence) || sequence == self.last_capture_sequence {
            return;
        }
        self.last_capture_sequence = sequence;
        if self.settings.capture_paused || !self.state.accept_capture() {
            return;
        }
        if self.tx.send(Work::Capture(sequence)).is_err() && self.state.capture_completed() {
            self.send(Command::Shutdown);
        }
    }
    unsafe fn work_done(&mut self) {
        while let Some(output) = self.organizer_worker.as_ref().and_then(|w| w.take()) {
            if self.state.lifecycle != Lifecycle::Running {
                continue;
            }
            match output {
                crate::harness_worker::Output::Actor(command) => {
                    let terminal = match &command {
                        Command::OrganizerComplete { token, .. }
                        | Command::HarnessFailed { token, .. } => Some(*token),
                        _ => None,
                    };
                    if terminal.is_none_or(|token| self.organizer_session.finish(token).is_some()) {
                        self.send(command);
                    }
                }
                crate::harness_worker::Output::Health(result) => {
                    *self.runtime_health.lock().unwrap() = match result {
                        Ok(()) => "Pi 运行时可用（未调用模型）".into(),
                        Err(message) => message,
                    };
                }
            }
        }
        if let Some(done) = self
            .screenshot_worker
            .as_ref()
            .and_then(BackgroundWorker::try_result)
        {
            if done.ticket.current() && self.state.lifecycle == Lifecycle::Running {
                match done.result {
                    Ok(prepared) => self.effects.push_back(UiEffect::ScreenshotDeliver {
                        owner: self.hwnd,
                        ticket: done.ticket,
                        prepared,
                    }),
                    Err(message) => {
                        self.screenshot_gate.finish(&done.ticket);
                        self.effects.push_back(UiEffect::Message {
                            owner: self.hwnd,
                            message,
                        });
                    }
                }
            } else {
                self.screenshot_gate.finish(&done.ticket);
            }
        }
        let observations = self
            .path_monitor
            .as_mut()
            .map(PathMonitor::receive)
            .unwrap_or_default();
        if !observations.is_empty() && self.state.lifecycle == Lifecycle::Running {
            self.send(Command::ObservePaths(observations));
        }
        self.path_monitor_status();
        self.ai_done();
        self.embedding_done();
        if let Some(result) = self
            .preview_worker
            .as_ref()
            .and_then(BackgroundWorker::try_result)
            && result.generation == self.preview_id
            && self.state.visible
            && self.state.lifecycle == Lifecycle::Running
        {
            self.image = None;
            self.preview_loading = false;
            let mut text = result.text;
            if let Some(image) = result.image {
                match image {
                    Ok(image) => {
                        self.status(&format!(
                            "图片：{} × {} · ↑↓ 选择 · Enter 粘贴",
                            image.source_width, image.source_height
                        ));
                        self.image = Some(Arc::new(image));
                    }
                    Err(error) => {
                        text = error;
                        self.status("图片预览不可用；仍可复制原图");
                    }
                }
            }
            self.sync_render();
            let text = wide(&text);
            let _ = SetWindowTextW(self.preview, PCWSTR(text.as_ptr()));
            let _ = InvalidateRect(Some(self.preview), None, true);
        }
        if let Some(result) = self
            .action_worker
            .as_ref()
            .and_then(BackgroundWorker::try_result)
        {
            match result {
                NativeResult::Action {
                    item,
                    action,
                    ticket,
                    dib,
                    result,
                    missing,
                } => {
                    let id = ticket.generation;
                    let shell = matches!(action, Action::Reveal)
                        || matches!(action, Action::Open(_))
                            && item.summary.kind != ItemKind::Clipboard;
                    // Shell has already changed foreground; a successful launch still earns its receipt.
                    if shell
                        && matches!(result, Ok(true))
                        && !item.summary.id.starts_with("path-suggestion:")
                    {
                        self.send(Command::Used(item.summary.id.clone()));
                    }
                    if GetForegroundWindow() == self.hwnd && self.state.action_current(id) {
                        if shell {
                            self.state.finish_action(id);
                            if let Err(error) = result {
                                self.status(&error);
                                if missing
                                    && matches!(action, Action::Open(_))
                                    && !item.summary.id.starts_with("path-suggestion:")
                                {
                                    self.perform(item, Action::Relink);
                                }
                            }
                        } else if matches!(action, Action::Open(_)) {
                            self.pending_paste = Some((id, item, action, self.target, dib));
                            self.release_paste();
                        } else if ticket.commit()
                            && self.state.finish_action(id)
                            && let Action::Copy(plain) = action
                        {
                            let result = clipboard::restore_prepared(self.hwnd, &item, plain, dib);
                            self.suppress_sequence = Some(clipboard::sequence_number());
                            match result {
                                Ok(()) => self.status("已复制"),
                                Err(error) => self.status(&error),
                            }
                        }
                    }
                }
                NativeResult::Paths {
                    item,
                    paths,
                    ticket,
                } => {
                    if GetForegroundWindow() == self.hwnd
                        && self.state.action_current(ticket.generation)
                        && self.state.finish_action(ticket.generation)
                    {
                        for (path, kind) in paths {
                            self.favorite_form(item.clone(), Some(path), kind);
                        }
                    }
                }
            }
        }
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Captured(result) => {
                    match result {
                        Ok(Some(snapshot)) => self.send(Command::Capture(snapshot)),
                        Ok(None) => {}
                        Err(e) => self.status(&e),
                    }
                    if self.state.capture_completed() {
                        self.send(Command::Shutdown);
                    }
                }
            }
        }
    }
    unsafe fn events(&mut self) {
        while let Some(event) = self.engine.as_ref().and_then(|e| e.try_recv().ok()) {
            match event {
                event @ (Event::HarnessContext { .. }
                | Event::HarnessToolResult { .. }
                | Event::HarnessFinished { .. }) => {
                    if let Some(worker) = &self.organizer_worker {
                        worker.reply(event);
                    }
                }
                Event::HarnessMemoryState {
                    request_id,
                    snapshot,
                } => {
                    self.harness_memory.borrow_mut().adopt(request_id, snapshot);
                }
                Event::HarnessMemoryReceipt {
                    request_id,
                    result,
                    pending_save,
                } => {
                    let admitted = result.is_ok() || pending_save;
                    if self
                        .harness_memory
                        .borrow_mut()
                        .receipt(request_id, result, pending_save)
                    {
                        if admitted {
                            self.organizer_session.cancel();
                            if let Some(worker) = &self.organizer_worker {
                                worker.cancel();
                            }
                        }
                        self.memory_snapshot();
                        self.organizer_snapshot();
                    }
                }
                Event::OrganizerProgress {
                    request_id,
                    progress,
                } => {
                    if self.organizer_progress.complete(request_id, progress) {
                        self.update_organizer_label();
                        self.refresh_organizer_rows();
                    }
                    self.request_organizer_progress();
                }
                Event::OrganizerMetadataUpdated(updates) => {
                    self.organizer_metadata_updated(&updates)
                }
                Event::OrganizerWork(work) => self.organizer_start(work),
                Event::OrganizerDetail { request_id, result } => {
                    self.organizer_review
                        .borrow_mut()
                        .adopt_detail(request_id, result);
                    if self.organizer_page_open {
                        let request = self.organizer_review.borrow_mut().request_detail();
                        if let Some((request_id, source, expected_version)) = request {
                            self.send(Command::OrganizerDetail {
                                request_id,
                                source,
                                expected_version,
                            });
                        }
                    }
                }
                Event::OrganizerState {
                    request_id,
                    snapshot,
                } => {
                    self.organizer_tick_pending = false;
                    self.organizer_review
                        .borrow_mut()
                        .adopt_snapshot(request_id, snapshot);
                }
                Event::OrganizerChanged => {
                    self.organizer_progress.mark_dirty();
                    self.request_organizer_progress();
                    if self.organizer_page_open {
                        self.organizer_snapshot();
                    }
                }
                Event::OrganizerReceipt {
                    request_id,
                    result,
                    pending_save,
                } => {
                    let success = result.is_ok();
                    if self
                        .organizer_review
                        .borrow_mut()
                        .receipt(request_id, result, pending_save)
                        && success
                    {
                        self.organizer_snapshot();
                    }
                }
                Event::PathSuggestions { request_id, items } => {
                    if self.state.is_current(request_id)
                        && let Some(paths) = self.path_batches.paths(request_id, items)
                    {
                        self.suggested_items = paths;
                        self.rebuild_results();
                    }
                }
                Event::PathActivityChanged => {
                    self.organizer_progress.mark_dirty();
                    self.request_organizer_progress();
                    if self.state.visible && self.state.lifecycle == Lifecycle::Running {
                        self.search();
                    }
                }
                Event::Ready(settings) => {
                    self.settings = settings;
                    self.update_organizer_timer();
                    self.update_path_monitor();
                    self.retheme();
                    if let Err(e) = self.replace_hotkeys(&self.settings.clone()) {
                        self.show(None);
                        self.effects.push_back(UiEffect::Message {
                            owner: self.hwnd,
                            message: e,
                        });
                    }
                    self.tray(NIM_MODIFY);
                    self.refresh_semantic_status();
                    self.search();
                }
                Event::SemanticStatus { request_id, status } => {
                    let profile = self.smart_view().profile;
                    let accepted = self.semantic_status.complete(request_id, status, &profile);
                    self.update_index_label();
                    if accepted {
                        self.finish_smart_preflight();
                    } else if self.state.visible {
                        self.refresh_semantic_status();
                    }
                }
                Event::SemanticStatusFailed {
                    request_id,
                    message,
                } => {
                    if self.semantic_status.fail(request_id) {
                        self.cancel_preflight();
                        self.update_index_label();
                        // Read/storage failures keep their detail, without automatic retries.
                        if self.state.lifecycle == Lifecycle::Running {
                            self.status(&message);
                        }
                    }
                }
                Event::SemanticBatch(batch) => self.index_batch(batch),
                Event::EmbeddingsStored {
                    request_id,
                    accepted,
                } => {
                    let was_active = self.semantic_index.active();
                    if let Some(command) = self.semantic_index.acknowledge(request_id, accepted) {
                        self.send_index(command);
                    }
                    if was_active {
                        self.semantic_status.mark_dirty();
                        self.index_status();
                        if !self.semantic_index.active() {
                            self.refresh_semantic_status();
                        }
                    }
                    if self.state.visible && self.semantic_query.adopted() {
                        self.search();
                    }
                }
                Event::Results { request_id, items } => {
                    if self.state.is_current(request_id) {
                        self.saved_items = items;
                        self.suggested_items =
                            self.path_batches.results(request_id).unwrap_or_default();
                        self.saved_ready = true;
                        self.rebuild_results();
                    }
                }
                Event::Loaded { request_id, item } => {
                    self.action_loads.complete(request_id);
                    let (preview_current, next) = if item.is_some() {
                        self.preview_loads.complete(request_id)
                    } else {
                        self.preview_loads.fail(request_id)
                    };
                    let preview_retrying = next.is_some();
                    if let Some((request_id, summary)) = next {
                        self.send(Command::Load {
                            id: summary.id,
                            request_id,
                        });
                    }
                    if self
                        .pending_action
                        .as_ref()
                        .is_some_and(|(id, _)| *id == request_id)
                    {
                        if GetForegroundWindow() == self.hwnd
                            && self.state.action_current(request_id)
                        {
                            if let Some((_, action)) = self.pending_action.take() {
                                if let Some(item) = item {
                                    if matches!(
                                        action,
                                        Action::Open(_) | Action::Copy(_) | Action::Reveal
                                    ) && (item.summary.kind == ItemKind::Clipboard
                                        || !matches!(action, Action::Copy(_)))
                                    {
                                        self.start_native(item, action, request_id);
                                    } else if self.state.finish_action(request_id) {
                                        self.perform(item, action);
                                    }
                                } else {
                                    self.state.finish_action(request_id);
                                }
                            }
                        } else {
                            self.cancel_actions();
                        }
                    } else if preview_current
                        && self.state.visible
                        && self.state.lifecycle == Lifecycle::Running
                    {
                        if let Some(item) = item {
                            self.perform(item, Action::Preview);
                        } else if !preview_retrying {
                            self.preview_unavailable("所选记录已不可用，可重新选择重试");
                        }
                    }
                }
                Event::LoadFailed {
                    request_id,
                    message,
                } => {
                    self.action_loads.complete(request_id);
                    if self
                        .pending_action
                        .as_ref()
                        .is_some_and(|(id, _)| *id == request_id)
                    {
                        self.pending_action = None;
                        self.state.finish_action(request_id);
                    }
                    let (preview_current, next) = self.preview_loads.fail(request_id);
                    let preview_retrying = next.is_some();
                    if preview_current
                        && !preview_retrying
                        && self.state.visible
                        && self.state.lifecycle == Lifecycle::Running
                    {
                        self.preview_unavailable(&message);
                    }
                    if let Some((request_id, summary)) = next {
                        self.send(Command::Load {
                            id: summary.id,
                            request_id,
                        });
                    }
                    // Storage errors remain visible; unrelated generic errors never release Load slots.
                    if self.preview_open && self.preview_loading {
                        self.status(&format!("{message} · 正在加载所选内容预览…"));
                    } else {
                        self.status(&message);
                    }
                }
                Event::Changed => {
                    self.organizer_progress.mark_dirty();
                    self.request_organizer_progress();
                    self.semantic_status.mark_dirty();
                    self.update_index_label();
                    if self.state.visible {
                        self.refresh_semantic_status();
                        self.search();
                    }
                }
                Event::Saved(stats) => {
                    if stats.semantic_status_changed {
                        self.semantic_status.mark_dirty();
                        if self.state.visible {
                            self.refresh_semantic_status();
                        }
                    }
                    if self.state.lifecycle == Lifecycle::Running
                        && !self.organizer_review.borrow().pending()
                    {
                        self.status("已保存 · ↑↓ 选择  Enter 打开/粘贴");
                    }
                }
                Event::Error { message, exiting } => {
                    if exiting && self.state.exit_failed() {
                        self.cancel_actions();
                        self.clear_preview();
                        self.effects.push_back(UiEffect::ExitError {
                            owner: self.hwnd,
                            message,
                        });
                    } else if !exiting {
                        if self.organizer_review.borrow().pending()
                            || self.organizer_review.borrow().needs_save()
                        {
                            let mut review = self.organizer_review.borrow_mut();
                            review.message = format!("未保存：{message}");
                            review.revision += 1;
                            continue;
                        }
                        // Admission/profile failures have no store acknowledgement.
                        self.cancel_index();
                        self.status(&message);
                    }
                }
                Event::ExitReady => {
                    self.effects.push_back(UiEffect::Destroy(self.hwnd));
                }
            }
        }
    }
    unsafe fn selection(&mut self) {
        self.selection_with_preview(false);
    }
    unsafe fn selection_with_preview(&mut self, keep_preview: bool) {
        let previous = self.state.selected_id().map(str::to_owned);
        let i = SendMessageW(self.list, LB_GETCURSEL, None, None).0;
        if i >= 0 {
            self.state.select(i as usize);
        } else {
            self.state.select(usize::MAX);
            if previous.is_some() {
                self.cancel_actions();
                self.clear_preview();
            }
            self.status(if self.saved_ready {
                "没有匹配结果 · 可换个关键词或添加收藏"
            } else {
                "正在检索保存内容…"
            });
            return;
        }
        if previous.as_deref() != self.state.selected_id() {
            self.cancel_actions();
        }
        let selected = self.items.get(i as usize);
        let function = selected.is_some_and(|entry| entry.function().is_some());
        let clipboard = selected
            .and_then(PaletteEntry::saved)
            .is_some_and(|item| item.kind == ItemKind::Clipboard);
        self.status(if function {
            "Enter 执行功能 · Esc 关闭"
        } else if clipboard {
            "Enter 粘贴 · Esc 关闭 · 右键更多"
        } else {
            "Enter 打开 · Esc 关闭 · 右键更多"
        });
        if self.preview_open && !(keep_preview && previous.as_deref() == self.state.selected_id()) {
            if function {
                self.clear_preview();
            } else {
                self.request(Action::Preview);
            }
        }
        if self.preview_open && self.preview_loading {
            self.status("正在加载所选内容预览…");
        }
        if self.smart_preflight.busy() {
            self.status("正在检查本地索引…点击智能查找可取消");
        } else if self.ai_gate.busy() || self.semantic_query.busy() {
            self.status("智能查找处理中…可点击取消");
        }
    }
    unsafe fn request(&mut self, action: Action) {
        if !self.state.visible || self.state.modal_open {
            return;
        }
        if self.state.lifecycle != Lifecycle::Running {
            return;
        }
        if let Some(function) = self
            .state
            .selected_index()
            .and_then(|index| self.items.get(index))
            .and_then(PaletteEntry::function)
        {
            if matches!(action, Action::Open(_)) {
                self.execute_function(function);
            }
            return;
        }
        if let Some(entry) = self
            .state
            .selected_index()
            .and_then(|index| self.items.get(index))
            .filter(|entry| entry.suggested().is_some())
            .cloned()
        {
            let path = entry.suggested().unwrap();
            if matches!(action, Action::Preview) {
                self.clear_preview();
                let text = wide(&format!(
                    "{}\n\n{}\n\n外部观察 · {} 次有效访问\n最近访问：{}",
                    path.title,
                    path.path,
                    path.visits,
                    local_saved_time(path.last_seen_ms)
                ));
                let _ = SetWindowTextW(self.preview, PCWSTR(text.as_ptr()));
                return;
            }
            if self.action_loads.busy()
                || self.pending_action.is_some()
                || self.pending_paste.is_some()
                || self
                    .action_worker
                    .as_ref()
                    .is_some_and(BackgroundWorker::busy)
            {
                return;
            }
            self.load_id = self.load_id.wrapping_add(1);
            if !self.state.begin_action(self.load_id) {
                return;
            }
            match action {
                Action::Open(_) | Action::Reveal => {
                    self.start_native(entry.suggestion_action().unwrap(), action, self.load_id)
                }
                Action::Favorite => {
                    if self.state.finish_action(self.load_id) {
                        self.favorite_form(None, Some(path.path.clone()), path.kind);
                    }
                }
                Action::Suppress => {
                    if self.state.finish_action(self.load_id) {
                        self.send(Command::SuppressPath(path.key.clone()));
                    }
                }
                Action::Copy(_) | Action::CopyPath => {
                    if self.state.finish_action(self.load_id) {
                        self.perform(entry.suggestion_action().unwrap(), Action::CopyPath);
                    }
                }
                _ => {
                    self.state.finish_action(self.load_id);
                }
            }
            return;
        }
        if let Some(id) = self.state.selected_id().map(str::to_owned) {
            if matches!(action, Action::Preview) {
                let summary = self
                    .state
                    .selected_index()
                    .and_then(|i| self.items.get(i))
                    .and_then(PaletteEntry::saved)
                    .cloned();
                self.load_id = self.load_id.wrapping_add(1);
                let previous = self.preview_loads.generation();
                let next = self.preview_loads.select(summary, self.load_id);
                if self.preview_loads.generation() != previous {
                    self.preview_id = self.preview_loads.generation();
                    self.preview_loading = true;
                    if let Some(worker) = &self.preview_worker {
                        worker.cancel_pending();
                    }
                    self.status("正在加载所选内容预览…");
                }
                if let Some((request_id, summary)) = next {
                    self.send(Command::Load {
                        id: summary.id,
                        request_id,
                    });
                }
                return;
            }
            if self.action_loads.busy()
                || self.pending_action.is_some()
                || self.pending_paste.is_some()
                || self
                    .action_worker
                    .as_ref()
                    .is_some_and(BackgroundWorker::busy)
            {
                return;
            }
            self.load_id = self.load_id.wrapping_add(1);
            if !self.state.begin_action(self.load_id) || !self.action_loads.begin(self.load_id) {
                return;
            }
            self.pending_action = Some((self.load_id, action));
            self.send(Command::Load {
                id,
                request_id: self.load_id,
            });
        }
    }
    fn start_native(&mut self, item: StoredItem, action: Action, id: u64) {
        let ticket = self.action_permit.begin(id);
        if !self.action_worker.as_ref().is_some_and(|w| {
            w.submit(NativeJob::Action {
                item,
                action,
                ticket,
            })
        }) {
            self.cancel_actions();
        }
    }
    unsafe fn release_paste(&mut self) {
        let Some((request_id, _, _, target, _)) = self.pending_paste.as_ref() else {
            return;
        };
        if self.target != *target
            || GetForegroundWindow() != self.hwnd
            || !self.state.action_current(*request_id)
        {
            self.cancel_actions();
            return;
        }
        // Do not alter the user's physical key state. Enter and every modifier
        // must actually be released, including when Paste came from a menu.
        let keys_down = [VK_RETURN, VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
            .into_iter()
            .any(|key| GetAsyncKeyState(key.0 as i32) < 0);
        if self.state.finish_paste(*request_id, keys_down)
            && let Some((_, item, action, _, dib)) = self.pending_paste.take()
            && let Action::Open(plain) = action
        {
            let result = clipboard::restore_prepared(self.hwnd, &item, plain, dib);
            self.suppress_sequence = Some(clipboard::sequence_number());
            match result {
                Ok(()) => {
                    self.send(Command::Used(item.summary.id));
                    let target = self.target;
                    self.hide();
                    if let Err(error) = clipboard::paste_to(target) {
                        self.show_panel(None, false);
                        self.status(&error);
                    }
                }
                Err(error) => self.status(&error),
            }
        }
    }
    unsafe fn perform(&mut self, item: StoredItem, action: Action) {
        match action {
            Action::Open(_) | Action::Reveal | Action::Favorite | Action::Suppress => {}
            Action::Copy(_) => self.perform(item, Action::CopyPath),
            Action::Pin => self.send(Command::Pin {
                id: item.summary.id,
                pinned: !item.summary.pinned,
            }),
            Action::Delete => self.send(Command::Delete(item.summary.id)),
            Action::CopyPath => {
                let path = item.target.clone().unwrap_or_default();
                let mut copied = item;
                copied.text = path;
                copied.representations.clear();
                if let Err(e) = clipboard::restore(self.hwnd, &copied, true) {
                    self.status(&e);
                }
                self.suppress_sequence = Some(clipboard::sequence_number());
            }
            Action::Edit => {
                if item.summary.kind == ItemKind::Clipboard {
                    if let Some(form) = ClipboardMetadataForm::new(&item.summary) {
                        self.effects.push_back(UiEffect::ClipboardMetadata {
                            owner: self.hwnd,
                            form,
                            target: self.state.menu_target(),
                        });
                    } else {
                        self.status("请先固定，再编辑名称、别名和标签");
                    }
                } else {
                    self.edit_favorite(Some(item), None);
                }
            }
            Action::Relink => {
                self.effects.push_back(UiEffect::Picker {
                    owner: self.hwnd,
                    folder: item.summary.kind == ItemKind::Folder,
                    item: Some(item),
                });
            }
            Action::Preview => {
                if let Some(worker) = &self.preview_worker {
                    worker.replace((self.preview_id, item));
                }
            }
        }
    }
    unsafe fn edit_favorite(&mut self, item: Option<StoredItem>, target: Option<String>) {
        if let Some(kind) = item.as_ref().map(|i| i.summary.kind) {
            self.favorite_form(item, target, kind);
        } else if let Some(path) = target {
            self.classify_paths(item, vec![path]);
        }
    }
    fn classify_paths(&mut self, item: Option<StoredItem>, paths: Vec<String>) {
        if self
            .action_worker
            .as_ref()
            .is_some_and(BackgroundWorker::busy)
        {
            unsafe {
                self.status("正在处理文件操作，请稍后再试");
            }
            return;
        }
        self.load_id = self.load_id.wrapping_add(1);
        if !self.state.begin_action(self.load_id) {
            return;
        }
        let ticket = self.action_permit.begin(self.load_id);
        if !self.action_worker.as_ref().is_some_and(|w| {
            w.submit(NativeJob::Paths {
                item,
                paths,
                ticket,
            })
        }) {
            self.cancel_actions();
        }
    }
    unsafe fn favorite_form(
        &mut self,
        item: Option<StoredItem>,
        target: Option<String>,
        kind: ItemKind,
    ) {
        if kind == ItemKind::Clipboard {
            self.status("剪贴板记录可固定；文件与文件夹可编辑收藏信息。");
            return;
        }
        let path = target
            .or_else(|| item.as_ref().and_then(|i| i.target.clone()))
            .unwrap_or_default();
        let name = item
            .as_ref()
            .map(|i| i.summary.title.clone())
            .unwrap_or_else(|| {
                Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let fields = vec![
            ("名称", name),
            ("目标路径", path),
            (
                "别名",
                item.as_ref()
                    .map(|i| i.summary.alias.clone())
                    .unwrap_or_default(),
            ),
            (
                "标签（逗号分隔）",
                item.as_ref()
                    .map(|i| i.summary.tags.join(", "))
                    .unwrap_or_default(),
            ),
            (
                "打开方式（程序完整路径，可留空）",
                item.as_ref()
                    .and_then(|i| i.open_with.clone())
                    .unwrap_or_default(),
            ),
        ];
        self.effects.push_back(UiEffect::Favorite {
            owner: self.hwnd,
            item,
            kind,
            fields,
        });
    }
    unsafe fn apply_favorite(&mut self, item: Option<StoredItem>, kind: ItemKind, v: Vec<String>) {
        if self.state.lifecycle != Lifecycle::Running {
            return;
        }
        if v[0].trim().is_empty() || v[1].trim().is_empty() {
            self.status("名称和路径不能为空");
            return;
        }
        self.send(Command::SaveFavorite(FavoriteInput {
            id: item.map(|i| i.summary.id),
            kind,
            title: v[0].trim().into(),
            target: v[1].trim().into(),
            alias: v[2].trim().into(),
            tags: v[3]
                .split([',', '，'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            open_with: (!v[4].trim().is_empty()).then(|| v[4].trim().into()),
        }));
    }
    unsafe fn add(&mut self, folder: bool) {
        self.effects.push_back(UiEffect::Picker {
            owner: self.hwnd,
            folder,
            item: None,
        });
    }
    unsafe fn update_path_monitor(&mut self) {
        if let Some(monitor) = &mut self.path_monitor {
            monitor.configure(self.settings.path_activity.clone());
        }
        if self.settings.path_activity.enabled && self.state.lifecycle == Lifecycle::Running {
            if SetTimer(Some(self.hwnd), PATH_TIMER, 2000, None) == 0 {
                self.status("常用路径观察计时器不可用");
            }
            if let Some(monitor) = &mut self.path_monitor {
                monitor.tick();
            }
        } else {
            let _ = KillTimer(Some(self.hwnd), PATH_TIMER);
            self.suggested_items.clear();
        }
        self.path_monitor_status();
    }
    unsafe fn stop_path_monitor(&mut self) {
        let _ = KillTimer(Some(self.hwnd), PATH_TIMER);
        if let Some(monitor) = &mut self.path_monitor {
            monitor.stop();
        }
    }
    unsafe fn path_monitor_status(&mut self) {
        if let Some(message) = self
            .path_monitor
            .as_mut()
            .and_then(PathMonitor::take_status)
        {
            self.status(&message);
        }
    }
    unsafe fn path_settings(&mut self) {
        self.effects.push_back(UiEffect::PathSettings {
            owner: self.hwnd,
            settings: self.settings.path_activity.clone(),
        });
    }
    unsafe fn apply_path_settings(&mut self, values: Vec<String>) -> Result<(), String> {
        if self.state.lifecycle != Lifecycle::Running {
            return Ok(());
        }
        let exclusions = crate::path_observer::parse_exclusions(&values[2])?;
        let next = crate::path_activity::PathActivitySettings {
            enabled: values[0] == "1",
            recent_documents: values[1] == "1",
            excluded_paths: exclusions,
        };
        let changed = self.settings.path_activity != next;
        self.settings.path_activity = next;
        self.update_path_monitor();
        if changed {
            self.send(Command::SaveSettings(self.settings.clone()));
        }
        if values[3] == "1" {
            if let Some(monitor) = &mut self.path_monitor {
                monitor.clear();
            }
            self.suggested_items.clear();
            self.send(Command::ClearPathActivity);
        }
        self.search();
        Ok(())
    }
    unsafe fn settings(&mut self) {
        self.effects.push_back(UiEffect::Settings {
            owner: self.hwnd,
            settings: self.settings.clone(),
        });
    }
    unsafe fn begin_screenshot(&mut self) {
        self.begin_screenshot_for(None);
    }
    unsafe fn begin_screenshot_for(&mut self, menu_target: Option<HWND>) {
        if self.state.lifecycle != Lifecycle::Running || self.state.modal_open {
            return;
        }
        let Some(ticket) = self.screenshot_gate.begin(clipboard::sequence_number()) else {
            self.effects.push_back(UiEffect::Message {
                owner: self.hwnd,
                message: "截图正在编码或交付，请等待本次处理完成".into(),
            });
            return;
        };
        let foreground = GetForegroundWindow();
        let target = menu_target.unwrap_or_else(|| {
            if self.state.visible || foreground == self.hwnd {
                self.target
            } else {
                foreground
            }
        });
        self.cancel_preflight();
        self.cancel_semantic();
        self.cancel_ai();
        self.cancel_actions();
        self.clear_preview();
        self.state.hide();
        self.effects.push_back(UiEffect::ScreenshotSelect {
            owner: self.hwnd,
            target,
            ticket,
        });
    }
    unsafe fn screenshot_settings(&mut self) {
        self.effects.push_back(UiEffect::ScreenshotSettings {
            owner: self.hwnd,
            settings: self.settings.clone(),
        });
    }
    unsafe fn apply_screenshot_settings(&mut self, values: Vec<String>) -> Result<(), String> {
        if self.state.lifecycle != Lifecycle::Running {
            return Ok(());
        }
        let (modifiers, key) = parse_hotkey(values.first().ok_or("截图快捷键设置无效")?)?;
        let mut next = self.settings.clone();
        next.screenshot_hotkey_modifiers = modifiers;
        next.screenshot_hotkey_key = key;
        self.replace_hotkeys(&next)?;
        self.settings = next;
        self.send(Command::SaveSettings(self.settings.clone()));
        Ok(())
    }
    unsafe fn settings_menu(&mut self) {
        let mut rect = RECT::default();
        let _ = GetWindowRect(self.buttons[2], &mut rect);
        self.effects.push_back(UiEffect::Menu {
            owner: self.hwnd,
            x: rect.left,
            y: rect.bottom,
            entries: vec![
                (GENERAL_SETTINGS, "常规设置…"),
                (PATH_SETTINGS, "常用推荐设置…"),
                (SCREENSHOT, "区域截图"),
                (SCREENSHOT_SETTINGS, "截图快捷键设置…"),
                (AI_SETTINGS, "AI 检索设置…"),
                (ORGANIZER_SETTINGS, "AI 管家设置…"),
                (ORGANIZER_REVIEW, "整理建议…"),
                (EMBEDDING_SETTINGS, "语义检索设置…"),
            ],
            target: None,
        });
        if self.settings.embedding.enabled
            && let Some(UiEffect::Menu { entries, .. }) = self.effects.back_mut()
        {
            entries.push(if self.semantic_index.active() {
                (INDEX_CANCEL, "取消索引")
            } else {
                (INDEX_START, "建立/更新语义索引")
            });
        }
    }
    unsafe fn ai_settings(&mut self) {
        self.cancel_semantic();
        self.cancel_ai();
        self.effects.push_back(UiEffect::AiSettings {
            owner: self.hwnd,
            settings: self.settings.ai.clone(),
        });
    }
    unsafe fn apply_ai_settings(&mut self, mut values: Vec<String>) -> Result<(), String> {
        let parsed = crate::organizer_ui::parse_ai_fields(&self.settings.ai, &values);
        let password = ai_client::SecretString(std::mem::take(&mut values[4]));
        if self.state.lifecycle != Lifecycle::Running {
            return Ok(());
        }
        let mut next = parsed?;
        next.enabled = values[0] == "1";
        next.endpoint = values[1].trim().into();
        next.model = values[2].trim().into();
        next.auth = if values[3] == "1" {
            AuthMethod::Bearer
        } else {
            AuthMethod::ApiKey
        };
        if values[5] == "1" {
            if !password.0.is_empty() {
                return Err("清除密钥与填写新密钥请选择其中一个操作".into());
            }
            next.encrypted_key.clear();
            next.enabled = false;
        } else if !password.0.is_empty() {
            if password.0.len() > 4096 || !password.0.bytes().all(|byte| (33..=126).contains(&byte))
            {
                return Err("API Key 格式无效".into());
            }
            match ai_client::protect_key(&password.0) {
                Ok(cipher) => next.encrypted_key = cipher,
                Err(message) => return Err(message),
            }
        }
        if next.enabled {
            ai_client::validate_settings(&next)
                .map_err(|message| format!("{message}，AI 配置未保存"))?;
        } else if !next.endpoint.is_empty()
            && crate::claude::parse_endpoint(&next.endpoint).is_err()
        {
            return Err("API 地址无效，配置未保存".into());
        }
        self.cancel_ai();
        self.cancel_preflight();
        let mut settings = self.settings.clone();
        settings.ai = next;
        self.cancel_organizer_for_settings(&settings);
        self.settings = settings;
        self.update_organizer_timer();
        self.send(Command::SaveSettings(self.settings.clone()));
        self.retheme();
        Ok(())
    }
    unsafe fn apply_settings(&mut self, v: Vec<String>) {
        if self.state.lifecycle != Lifecycle::Running {
            return;
        }

        let mut settings = self.settings.clone();
        settings.theme = match v[0].as_str() {
            "1" => Theme::Light,
            "2" => Theme::Dark,
            _ => Theme::System,
        };
        let keys = parse_hotkey(&v[1])
            .and_then(|main| parse_hotkey(&v[2]).map(|clipboard| (main, clipboard)));
        let ((m, k), (cm, ck)) = match keys {
            Ok(keys) => keys,
            Err(message) => {
                self.status(&message);
                return;
            }
        };
        settings.hotkey_modifiers = m;
        settings.hotkey_key = k;
        settings.clipboard_hotkey_modifiers = cm;
        settings.clipboard_hotkey_key = ck;
        settings.smart_search = match v[5].as_str() {
            "1" => SmartSearchMode::Claude,
            "2" => SmartSearchMode::Semantic,
            _ => SmartSearchMode::Auto,
        };
        settings.capture_paused = v[3] == "1";
        settings.excluded_apps = v[4]
            .split([',', '，'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if let Err(message) = self.replace_hotkeys(&settings) {
            self.effects.push_back(UiEffect::Message {
                owner: self.hwnd,
                message,
            });
            return;
        }
        self.cancel_preflight();
        self.settings = settings.clone();
        self.retheme();
        self.tray(NIM_MODIFY);
        self.send(Command::SaveSettings(settings));
    }
    unsafe fn context(&mut self, x: i32, y: i32, tray: bool, add: bool) {
        let entries: Vec<(usize, &str)> = if tray {
            vec![
                (221, "打开面板"),
                (GENERAL_SETTINGS, "常规设置…"),
                (PATH_SETTINGS, "常用推荐设置…"),
                (SCREENSHOT, "区域截图"),
                (SCREENSHOT_SETTINGS, "截图快捷键设置…"),
                (AI_SETTINGS, "AI 检索设置…"),
                (ORGANIZER_SETTINGS, "AI 管家设置…"),
                (ORGANIZER_REVIEW, "整理建议…"),
                (EMBEDDING_SETTINGS, "语义检索设置…"),
                (222, "暂停 / 恢复剪贴板记录"),
                (EXIT, "保存后退出"),
            ]
        } else if add {
            vec![(223, "添加文件夹…"), (224, "添加文档…")]
        } else if self
            .state
            .selected_index()
            .and_then(|index| self.items.get(index))
            .is_some_and(|entry| entry.function().is_some())
        {
            vec![(OPEN, "执行功能")]
        } else if self
            .state
            .selected_index()
            .and_then(|i| self.items.get(i))
            .is_some_and(|entry| entry.suggested().is_some())
        {
            vec![
                (OPEN, "打开"),
                (REVEAL, "打开所在目录"),
                (COPY_PATH, "复制路径"),
                (FAVORITE_PATH, "收藏…"),
                (SUPPRESS_PATH, "不再推荐此路径"),
            ]
        } else if self
            .state
            .selected_index()
            .and_then(|i| self.items.get(i))
            .and_then(PaletteEntry::saved)
            .is_some_and(|i| i.kind == ItemKind::Clipboard)
        {
            let pinned = self
                .state
                .selected_index()
                .and_then(|i| self.items.get(i))
                .is_some_and(PaletteEntry::pinned);
            vec![
                (OPEN, "粘贴"),
                (COPY, "复制"),
                (PLAIN, "纯文本粘贴"),
                (PIN, "固定 / 取消固定"),
                if pinned {
                    (EDIT, "编辑名称、别名和标签…")
                } else {
                    (PIN, "先固定以编辑名称、别名和标签")
                },
                (DELETE, "删除"),
            ]
        } else {
            vec![
                (OPEN, "打开"),
                (COPY_PATH, "复制路径"),
                (PIN, "固定 / 取消固定"),
                (EDIT, "编辑收藏…"),
                (OPEN_WITH, "设置打开方式…"),
                (RELINK, "重新定位…"),
                (REVEAL, "打开所在目录"),
                (DELETE, "删除"),
            ]
        };
        self.effects.push_back(UiEffect::Menu {
            owner: self.hwnd,
            x,
            y,
            entries,
            target: (!tray && !add).then(|| self.state.menu_target()),
        });
    }
    unsafe fn execute_function(&mut self, function: Function) {
        self.cancel_actions();
        if function != Function::Exit
            && function != Function::AiSearch
            && function != Function::SemanticSearch
            && function != Function::SmartSearch
        {
            // Result commands return to browsing content. Scope and preview
            // commands read the cleared native Edit through their normal handler.
            let _ = SetWindowTextW(self.edit, w!(""));
        }
        match function {
            Function::Screenshot => self.begin_screenshot(),
            Function::ScreenshotSettings => self.screenshot_settings(),
            Function::Settings => self.settings(),
            Function::PathActivitySettings => self.path_settings(),
            Function::AiSettings => self.ai_settings(),
            Function::OrganizerSettings => self.organizer_settings(),
            Function::OrganizerReview => self.review_organizer(),
            Function::HarnessMemory => self.show_harness_memory(),
            Function::AiSearch => self.begin_smart(Some(SmartSearchMode::Claude)),
            Function::EmbeddingSettings => self.embedding_settings(),
            Function::SemanticSearch => self.begin_smart(Some(SmartSearchMode::Semantic)),
            Function::SmartSearch => self.begin_smart(None),
            Function::UpdateSemanticIndex => self.begin_index(),
            Function::CancelSemanticIndex => self.cancel_index(),
            Function::AddFolder => self.command(223),
            Function::AddDocument => self.command(224),
            Function::PauseCapture if !self.settings.capture_paused => self.command(222),
            Function::ResumeCapture if self.settings.capture_paused => self.command(222),
            Function::ShowPreview if !self.preview_open => self.command(PREVIEW),
            Function::HidePreview if self.preview_open => self.command(PREVIEW),
            Function::All => self.command(300),
            Function::Clipboard => self.command(301),
            Function::Folders => self.command(302),
            Function::Documents => self.command(303),
            Function::Exit => self.command(EXIT),
            _ => {}
        }
    }
    unsafe fn command(&mut self, id: usize) {
        if self.state.lifecycle != Lifecycle::Running && id != EXIT {
            return;
        }
        match id {
            ADD => {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                self.context(p.x, p.y, false, true);
            }
            PREVIEW => {
                self.preview_open = !self.preview_open;
                let _ = ShowWindow(
                    self.preview,
                    if self.preview_open { SW_SHOW } else { SW_HIDE },
                );
                let label = wide(if self.preview_open {
                    "收起 ‹"
                } else {
                    "预览 ›"
                });
                let _ = SetWindowTextW(self.buttons[1], PCWSTR(label.as_ptr()));
                self.show_panel(None, false);
                if self.preview_open {
                    self.request(Action::Preview);
                }
            }
            SETTINGS => self.settings_menu(),
            GENERAL_SETTINGS => self.settings(),
            PATH_SETTINGS => self.path_settings(),
            SCREENSHOT => self.begin_screenshot(),
            SCREENSHOT_SETTINGS => self.screenshot_settings(),
            FAVORITE_PATH => self.request(Action::Favorite),
            SUPPRESS_PATH => self.request(Action::Suppress),
            AI_SETTINGS => self.ai_settings(),
            ORGANIZER_SETTINGS => self.organizer_settings(),
            ORGANIZER_REVIEW => self.review_organizer(),
            SMART_FIND => self.begin_smart(None),
            INDEX_STATUS if self.semantic_index.active() => self.cancel_index(),
            INDEX_STATUS => self.begin_index(),
            EMBEDDING_SETTINGS => self.embedding_settings(),
            INDEX_START => self.begin_index(),
            INDEX_CANCEL => self.cancel_index(),
            300..=303 => {
                self.cancel_preflight();
                self.cancel_semantic();
                self.cancel_ai();
                self.scope = match id {
                    301 => Scope::Clipboard,
                    302 => Scope::Folder,
                    303 => Scope::Document,
                    _ => Scope::All,
                };
                self.search();
            }
            OPEN => self.request(Action::Open(false)),
            COPY => self.request(Action::Copy(false)),
            PLAIN => self.request(Action::Open(true)),
            PIN => self.request(Action::Pin),
            EDIT | OPEN_WITH => self.request(Action::Edit),
            RELINK => self.request(Action::Relink),
            REVEAL => self.request(Action::Reveal),
            DELETE => self.request(Action::Delete),
            COPY_PATH => self.request(Action::CopyPath),
            221 => self.show(None),
            222 => {
                self.settings.capture_paused = !self.settings.capture_paused;
                self.send(Command::SaveSettings(self.settings.clone()));
                self.status(if self.settings.capture_paused {
                    "已暂停剪贴板记录"
                } else {
                    "已恢复剪贴板记录"
                });
            }
            223 => self.add(true),
            224 => self.add(false),
            EXIT if self.state.lifecycle == Lifecycle::Running => {
                let _ = KillTimer(Some(self.hwnd), ORGANIZER_TIMER);
                self.organizer_session.cancel();
                if let Some(worker) = &self.organizer_worker
                    && let Some(checkpoint) = worker.shutdown()
                {
                    self.send(checkpoint);
                }
                self.screenshot_gate.close();
                self.cancel_preflight();
                self.cancel_index();
                self.cancel_semantic();
                self.cancel_ai();
                self.cancel_actions();
                self.clear_preview();
                self.state.visible = true;
                let _ = ShowWindow(self.hwnd, SW_SHOWNA);
                self.stop_path_monitor();
                self.status("正在保存后退出…");
                if self.state.begin_exit() {
                    self.send(Command::Shutdown);
                }
            }
            _ => {}
        }
    }
}
unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let host = &*(data as *const Host);
    match msg {
        WM_IME_STARTCOMPOSITION => host.composing.set(true),
        WM_IME_ENDCOMPOSITION => host.composing.set(false),
        WM_GETDLGCODE if host.composing.get() => {
            return LRESULT(DefSubclassProc(hwnd, msg, wp, lp).0 | DLGC_WANTALLKEYS as isize);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
        let host = &*(cs.lpCreateParams as *const Host);
        host.hwnd.set(hwnd);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Host;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let host = &*ptr;
    if msg == WM_QUERYENDSESSION {
        return LRESULT(1);
    }
    if msg == WM_ENDSESSION {
        if wp.0 != 0 {
            if host.app.try_borrow_mut().is_err() {
                host.defer(Deferred::Message(msg, wp, lp));
            } else {
                host.end_session(hwnd);
            }
        }
        return LRESULT(0);
    }
    if let Some(result) = render_message(host, hwnd, msg, wp, lp) {
        return result;
    }
    if msg == DEFERRED_WAKE {
        host.wake_posted.set(false);
        host.drain_deferred();
        host.drain_effects();
        return LRESULT(0);
    }
    let result = {
        match host.app.try_borrow_mut() {
            Ok(mut app) => app_message(&mut app, hwnd, msg, wp, lp),
            Err(_) => {
                match msg {
                    WM_DPICHANGED => host.defer(Deferred::Dpi(wp, *(lp.0 as *const RECT))),
                    ENGINE_WAKE | WORK_WAKE | ORGANIZER_UI_WAKE | SHOW_EXISTING
                    | WM_CLIPBOARDUPDATE | WM_HOTKEY | WM_CLOSE | WM_ACTIVATE | WM_SIZE
                    | WM_SETTINGCHANGE | WM_COMMAND | WM_THEMECHANGED | WM_CONTEXTMENU | TRAY
                    | WM_DROPFILES | WM_ENDSESSION | WM_DESTROY | WM_DISPLAYCHANGE => {
                        host.defer(Deferred::Message(msg, wp, lp))
                    }
                    WM_QUERYENDSESSION => return LRESULT(1),
                    _ => return DefWindowProcW(hwnd, msg, wp, lp),
                }
                return LRESULT(0);
            }
        }
    };
    host.drain_effects();
    result.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
}

unsafe fn app_message(
    app: &mut App,
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> Option<LRESULT> {
    match msg {
        SHOW_EXISTING => {
            if app.state.lifecycle == Lifecycle::Running && app.state.modal_open {
                let popup = GetLastActivePopup(app.hwnd);
                if popup != app.hwnd && IsWindowVisible(popup).as_bool() {
                    let _ = SetForegroundWindow(popup);
                }
            } else {
                let update_target = app.state.instance_show_updates_target();
                app.show_panel(None, update_target);
                if app.state.can_show_panel() {
                    SendMessageW(app.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
            }
            return Some(LRESULT(0));
        }
        PI_HEALTH_WAKE => {
            if app.state.lifecycle != Lifecycle::Running || app.organizer_tick_pending {
                *app.runtime_health.lock().unwrap() = "正在调度或退出，请稍后测试".into();
                return Some(LRESULT(0));
            }
            app.ensure_harness_worker();
            let result = app.organizer_worker.as_ref().unwrap().health();
            *app.runtime_health.lock().unwrap() = result
                .err()
                .unwrap_or_else(|| "正在启动 Pi（不调用模型）…".into());
            return Some(LRESULT(0));
        }
        ENGINE_WAKE => {
            app.events();
            return Some(LRESULT(0));
        }
        ORGANIZER_UI_WAKE => {
            let commands = {
                let mut outbox = app.organizer_outbox.borrow_mut();
                [outbox.0.take(), outbox.1.take()]
            };
            for command in commands.into_iter().flatten() {
                if app.state.lifecycle == Lifecycle::Running {
                    app.send(command);
                    if app.organizer_page_open {
                        app.organizer_snapshot();
                    }
                }
            }
            return Some(LRESULT(0));
        }
        WM_TIMER if wp.0 == ORGANIZER_TIMER => {
            app.organizer_tick();
            return Some(LRESULT(0));
        }
        WM_TIMER if wp.0 == PATH_TIMER => {
            if app.state.lifecycle == Lifecycle::Running
                && let Some(monitor) = &mut app.path_monitor
            {
                monitor.tick();
            }
            app.path_monitor_status();
            return Some(LRESULT(0));
        }
        WORK_WAKE => {
            app.work_done();
            return Some(LRESULT(0));
        }
        WM_CLIPBOARDUPDATE => {
            app.capture();
            return Some(LRESULT(0));
        }
        WM_HOTKEY => {
            if let Some(action) = app.hotkey_bindings.action_for(wp.0 as i32) {
                if action == 2 {
                    app.begin_screenshot();
                } else {
                    app.show((action == 1).then_some(Scope::Clipboard));
                }
            }
            return Some(LRESULT(0));
        }
        WM_CLOSE => {
            app.hide();
            return Some(LRESULT(0));
        }
        WM_DISPLAYCHANGE => {
            app.screenshot_gate.cancel();
            return Some(LRESULT(0));
        }
        WM_ACTIVATE if (wp.0 & 0xffff) == WA_INACTIVE as usize && !app.state.modal_open => {
            app.hide();
            return Some(LRESULT(0));
        }
        WM_SIZE => {
            app.layout();
            return Some(LRESULT(0));
        }
        WM_DPICHANGED => {
            app.dpi = (wp.0 & 0xffff) as u32;
            let rect = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            app.retheme();
            app.layout();
            return Some(LRESULT(0));
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            app.retheme();
            return Some(LRESULT(0));
        }
        WM_COMMAND => {
            let id = wp.0 & 0xffff;
            let notification = (wp.0 >> 16) & 0xffff;
            if id == SEARCH && notification == EN_CHANGE as usize {
                app.search();
            } else if id == LIST && notification == LBN_SELCHANGE as usize {
                app.selection();
            } else if id == LIST && notification == LBN_DBLCLK as usize {
                app.request(Action::Open(false));
            } else if notification == BN_CLICKED as usize {
                app.command(id);
            }
            return Some(LRESULT(0));
        }
        WM_CONTEXTMENU => {
            let (x, y) = if lp.0 == -1 {
                let mut r = RECT::default();
                let _ = GetWindowRect(app.list, &mut r);
                (r.left + 20, r.top + 20)
            } else {
                (
                    (lp.0 & 0xffff) as i16 as i32,
                    ((lp.0 >> 16) & 0xffff) as i16 as i32,
                )
            };
            if lp.0 != -1 {
                let mut point = POINT { x, y };
                let _ = ScreenToClient(app.list, &mut point);
                let result = SendMessageW(
                    app.list,
                    LB_ITEMFROMPOINT,
                    None,
                    Some(LPARAM(
                        ((point.y as u32) << 16 | point.x as u32 & 0xffff) as isize,
                    )),
                );
                if result.0 >> 16 == 0 {
                    SendMessageW(
                        app.list,
                        LB_SETCURSEL,
                        Some(WPARAM((result.0 & 0xffff) as usize)),
                        None,
                    );
                    app.selection();
                }
            }
            app.context(x, y, false, false);
            return Some(LRESULT(0));
        }
        TRAY => {
            match lp.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => app.show(None),
                WM_RBUTTONUP => {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&mut p);
                    app.context(p.x, p.y, true, false);
                }
                _ => {}
            }
            return Some(LRESULT(0));
        }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as *mut _);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let mut paths = Vec::new();
            for i in 0..count.min(100) {
                let len = DragQueryFileW(drop, i, None);
                let mut b = vec![0; len as usize + 1];
                DragQueryFileW(drop, i, Some(&mut b));
                paths.push(String::from_utf16_lossy(&b[..len as usize]));
            }
            DragFinish(drop);
            app.classify_paths(None, paths);
            return Some(LRESULT(0));
        }
        WM_QUERYENDSESSION => return Some(LRESULT(1)),
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), ORGANIZER_TIMER);
            app.organizer_worker.take();
            app.screenshot_gate.close();
            app.stop_path_monitor();
            let _ = RemoveClipboardFormatListener(hwnd);
            app.hotkey_bindings.clear(|id| {
                let _ = UnregisterHotKey(Some(hwnd), id);
            });
            app.tray(NIM_DELETE);
            let _ = RemoveWindowSubclass(app.preview, Some(preview_proc), 3);
            let _ = RemoveWindowSubclass(app.edit, Some(edit_proc), 1);
            let _ = RemoveWindowSubclass(app.list, Some(edit_proc), 2);
            if !app.font.is_invalid() {
                let _ = DeleteObject(app.font.into());
            }
            PostQuitMessage(0);
            return Some(LRESULT(0));
        }
        _ => {}
    }
    None
}

fn palette_row_text(row: &palette::RowDetail) -> Vec<u16> {
    wide(&match row.organizer_stage {
        Some(stage) => format!("{} · {}", row.title, stage.label()),
        None => row.title.clone(),
    })
}

unsafe fn render_message(
    host: &Host,
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> Option<LRESULT> {
    let render = host.render.borrow().clone();
    match msg {
        WM_GETMINMAXINFO => {
            let info = &mut *(lp.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT {
                x: render.s(480),
                y: render.s(340),
            };
            return Some(LRESULT(0));
        }
        WM_DRAWITEM => {
            let draw = &*(lp.0 as *const DRAWITEMSTRUCT);
            if draw.CtlID == LIST as u32
                && draw.itemID != u32::MAX
                && let Some(item) = render.items.get(draw.itemID as usize)
            {
                let p = render.colors.palette;
                let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
                FillRect(
                    draw.hDC,
                    &draw.rcItem,
                    if selected {
                        render.colors.selected
                    } else {
                        render.colors.surface
                    },
                );
                let _ = SetBkMode(draw.hDC, TRANSPARENT);
                let _ = SelectObject(draw.hDC, render.font.into());
                let kind = item.kind_label();
                let mut title = wide(&format!(
                    "{}{}",
                    if item.pinned() { "★ " } else { "" },
                    render
                        .row_details
                        .get(draw.itemID as usize)
                        .map(|detail| detail.title.as_str())
                        .unwrap_or_else(|| item.title())
                ));
                let mut rect = draw.rcItem;
                rect.left += render.s(14);
                rect.right -= render.s(105);
                rect.top += render.s(8);
                rect.bottom = rect.top + render.s(25);
                SetTextColor(draw.hDC, p.text);
                DrawTextW(
                    draw.hDC,
                    &mut title,
                    &mut rect,
                    DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                let mut typ = wide(kind);
                let mut type_rect = draw.rcItem;
                type_rect.left = type_rect.right - render.s(98);
                type_rect.top += render.s(9);
                type_rect.bottom = type_rect.top + render.s(24);
                SetTextColor(draw.hDC, p.muted);
                DrawTextW(
                    draw.hDC,
                    &mut typ,
                    &mut type_rect,
                    DT_SINGLELINE | DT_NOPREFIX,
                );
                let mut subtitle = wide(
                    render
                        .row_details
                        .get(draw.itemID as usize)
                        .map(|detail| detail.text.as_str())
                        .unwrap_or_default(),
                );
                rect.top += render.s(26);
                rect.bottom = rect.top + render.s(22);
                let saved_time = render
                    .saved_times
                    .get(draw.itemID as usize)
                    .map(String::as_str)
                    .unwrap_or_default();
                let stage = render
                    .row_details
                    .get(draw.itemID as usize)
                    .and_then(|d| d.organizer_stage);
                let show_time = !saved_time.is_empty()
                    && (stage.is_none() || draw.rcItem.right - draw.rcItem.left >= render.s(340));
                rect.right = draw.rcItem.right - render.s(if show_time { 150 } else { 14 });
                if let Some(stage) = stage {
                    let mut badge = wide(&format!("● {}", stage.label()));
                    let width = render.s(if stage == crate::organizer_display::Stage::Ready {
                        112
                    } else {
                        86
                    });
                    let mut badge_rect = rect;
                    badge_rect.right = (badge_rect.left + width).min(rect.right);
                    SetTextColor(
                        draw.hDC,
                        if stage == crate::organizer_display::Stage::Waiting {
                            p.muted
                        } else {
                            p.accent
                        },
                    );
                    DrawTextW(
                        draw.hDC,
                        &mut badge,
                        &mut badge_rect,
                        DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );
                    rect.left += width;
                }
                SetTextColor(draw.hDC, p.muted);
                if rect.right > rect.left {
                    DrawTextW(
                        draw.hDC,
                        &mut subtitle,
                        &mut rect,
                        DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                    );
                }
                if show_time {
                    let mut time = wide(saved_time);
                    let mut time_rect = rect;
                    time_rect.left = draw.rcItem.right - render.s(145);
                    time_rect.right = draw.rcItem.right - render.s(14);
                    DrawTextW(
                        draw.hDC,
                        &mut time,
                        &mut time_rect,
                        DT_SINGLELINE | DT_RIGHT | DT_NOPREFIX,
                    );
                }
                if selected {
                    FrameRect(draw.hDC, &draw.rcItem, render.colors.accent);
                }
                if draw.itemState.0 & ODS_FOCUS.0 != 0 {
                    let _ = DrawFocusRect(draw.hDC, &draw.rcItem);
                }
            }
            return Some(LRESULT(1));
        }
        WM_MEASUREITEM => {
            let m = &mut *(lp.0 as *mut MEASUREITEMSTRUCT);
            if m.CtlID == LIST as u32 {
                m.itemHeight = render.s(62) as u32;
                return Some(LRESULT(1));
            }
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
            let dc = HDC(wp.0 as *mut _);
            let p = render.colors.palette;
            SetTextColor(dc, p.text);
            SetBkColor(dc, p.background);
            return Some(LRESULT(render.colors.background.0 as isize));
        }
        WM_ERASEBKGND => {
            let dc = HDC(wp.0 as *mut _);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            FillRect(dc, &r, render.colors.background);
            return Some(LRESULT(1));
        }
        _ => {}
    }
    None
}

unsafe extern "system" fn preview_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let host = &*(data as *const Host);
    let render = host.render.borrow().clone();
    if msg == WM_PAINT
        && let Some(image) = &render.image
    {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut r = RECT::default();
        let _ = GetClientRect(hwnd, &mut r);
        FillRect(dc, &r, render.colors.background);
        let caption_height = render.s(28);
        let mut caption = wide(&format!(
            "原图：{} × {}",
            image.source_width, image.source_height
        ));
        let mut caption_rect = RECT {
            left: render.s(8),
            top: render.s(4),
            right: r.right - render.s(8),
            bottom: caption_height,
        };
        let previous_font = SelectObject(dc, render.font.into());
        SetTextColor(dc, render.colors.palette.muted);
        SetBkMode(dc, TRANSPARENT);
        DrawTextW(
            dc,
            &mut caption,
            &mut caption_rect,
            DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(dc, previous_font);
        let ratio = (r.right as f64 / image.width as f64)
            .min((r.bottom - caption_height).max(0) as f64 / image.height as f64)
            .min(1.0);
        let width = (image.width as f64 * ratio) as i32;
        let height = (image.height as f64 * ratio) as i32;
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: image.width as i32,
                biHeight: -(image.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        SetStretchBltMode(dc, HALFTONE);
        StretchDIBits(
            dc,
            (r.right - width) / 2,
            caption_height + (r.bottom - caption_height - height) / 2,
            width,
            height,
            0,
            0,
            image.width as i32,
            image.height as i32,
            Some(image.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        let _ = EndPaint(hwnd, &ps);
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

impl Host {
    unsafe fn is_panel_surface(&self, window: HWND) -> bool {
        window == self.hwnd.get() || IsChild(self.hwnd.get(), window).as_bool()
    }
    fn defer(&self, message: Deferred) {
        self.deferred.borrow_mut().push_back(message);
        if !self.wake_posted.replace(true) {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd.get()), DEFERRED_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }
    unsafe fn drain_deferred(&self) {
        if self.app.try_borrow_mut().is_err() {
            return;
        }
        loop {
            let next = self.deferred.borrow_mut().pop_front();
            match next {
                Some(Deferred::Message(msg, wp, lp)) => {
                    window_proc(self.hwnd.get(), msg, wp, lp);
                }
                Some(Deferred::Dpi(wp, rect)) => {
                    window_proc(
                        self.hwnd.get(),
                        WM_DPICHANGED,
                        wp,
                        LPARAM((&rect as *const RECT) as isize),
                    );
                }
                None => break,
            }
        }
    }
    /// Keyboard handling precedes IsDialogMessage and applies to Edit, List and Buttons.
    /// During IME composition the original key reaches the native Edit unchanged.
    unsafe fn route_key(&self, msg: &MSG) -> bool {
        if matches!(msg.message, WM_KEYUP | WM_SYSKEYUP) {
            if let Ok(mut app) = self.app.try_borrow_mut() {
                app.release_paste();
            }
            return false;
        }
        if msg.message != WM_KEYDOWN || self.composing.get() {
            return false;
        }
        let mut app = match self.app.try_borrow_mut() {
            Ok(app) => app,
            Err(_) => return false,
        };
        if !app.state.visible || app.state.lifecycle != Lifecycle::Running || app.state.modal_open {
            return false;
        }
        app.state.composing = self.composing.get();
        let key = msg.wParam.0 as u32;
        if msg.hwnd == app.preview && key != VK_ESCAPE.0 as u32 {
            return false;
        }
        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
        // WM_KEYDOWN bit 30 distinguishes auto-repeat from a fresh press. A
        // function may clear the query before keyup; repeats must not activate
        // the newly selected saved item, including after native menus return.
        let repeated = msg.lParam.0 & (1 << 30) != 0;
        if key == VK_RETURN.0 as u32
            && app.state.key_down(PanelKey::Enter, repeated) == KeyAction::Ignore
        {
            return true;
        }
        // Native IsDialogMessage handles Enter for a focused Button. Only the
        // search/list result surfaces activate the chosen stored item.
        if key == VK_RETURN.0 as u32
            && (app.buttons.contains(&msg.hwnd)
                || msg.hwnd == app.smart_button
                || msg.hwnd == app.index_label
                || msg.hwnd == app.organizer_label)
        {
            return false;
        }
        let panel_key = match key {
            k if k == VK_ESCAPE.0 as u32 => Some(PanelKey::Escape),
            k if k == VK_RETURN.0 as u32 => Some(PanelKey::Enter),
            k if k == VK_UP.0 as u32 => Some(PanelKey::Up),
            k if k == VK_DOWN.0 as u32 => Some(PanelKey::Down),
            _ => None,
        };
        if let Some(panel_key) = panel_key {
            match app.state.key_down(panel_key, repeated) {
                KeyAction::Ignore => {}
                KeyAction::NativeInput => return false,
                KeyAction::Hide => app.hide(),
                KeyAction::Activate => app.request(Action::Open(shift)),
                action => {
                    let old = app.state.selected_index().unwrap_or(0);
                    let index = if action == KeyAction::SelectPrevious {
                        old.saturating_sub(1)
                    } else {
                        (old + 1).min(app.items.len().saturating_sub(1))
                    };
                    SendMessageW(app.list, LB_SETCURSEL, Some(WPARAM(index)), None);
                    app.selection();
                }
            }
            return true;
        }
        match key {
            67 if ctrl => {
                let (mut start, mut end) = (0u32, 0u32);
                SendMessageW(
                    app.edit,
                    EM_GETSEL,
                    Some(WPARAM((&mut start as *mut u32) as usize)),
                    Some(LPARAM((&mut end as *mut u32) as isize)),
                );
                if start != end {
                    SendMessageW(app.edit, WM_COPY, None, None);
                    app.suppress_sequence = Some(clipboard::sequence_number());
                } else {
                    app.request(Action::Copy(false));
                }
                true
            }
            75 if ctrl => {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                app.context(p.x, p.y, false, false);
                true
            }
            49..=52 if ctrl => {
                app.command(300 + (key - 49) as usize);
                true
            }
            _ => false,
        }
    }
    /// This path also runs inside native modal pumps; it never waits for a dialog to close.
    unsafe fn end_session(&self, hwnd: HWND) {
        {
            let mut app = self.app.borrow_mut();
            app.stop_path_monitor();
            app.screenshot_gate.close();
            app.cancel_index();
            app.cancel_semantic();
            app.cancel_ai();
            app.cancel_actions();
            app.clear_preview();
            if app.state.begin_exit() {
                app.send(Command::Shutdown);
            } else if app.state.lifecycle == Lifecycle::SaveFailed {
                app.state.retry_exit();
                app.send(Command::Shutdown);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let finished = {
                let mut app = self.app.borrow_mut();
                app.work_done();
                matches!(
                    app.engine
                        .as_ref()
                        .and_then(|engine| engine.try_recv().ok()),
                    Some(Event::ExitReady | Event::Error { .. })
                )
            };
            if finished {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = DestroyWindow(hwnd);
        std::process::exit(0);
    }
    /// Effect payloads own all dialog/menu inputs. No RefMut<App> survives this pump.
    unsafe fn drain_effects(&self) {
        if self.effects_running.replace(true) {
            return;
        }
        loop {
            let mut semantic_revoked = false;
            let effect = {
                let mut app = match self.app.try_borrow_mut() {
                    Ok(app) => app,
                    Err(_) => break,
                };
                let effect = app.effects.pop_front();
                if let Some(effect) = &effect {
                    if matches!(effect, UiEffect::ExitError { .. })
                        && app.state.lifecycle != Lifecycle::SaveFailed
                    {
                        continue;
                    }
                    if app.state.lifecycle != Lifecycle::Running
                        && !matches!(
                            effect,
                            UiEffect::ExitError { .. }
                                | UiEffect::Message { .. }
                                | UiEffect::Destroy(_)
                        )
                    {
                        if let UiEffect::ScreenshotSelect { ticket, .. }
                        | UiEffect::ScreenshotDeliver { ticket, .. } = effect
                        {
                            app.screenshot_gate.finish(ticket);
                        }
                        continue;
                    }
                    app.action_permit.cancel();
                    app.pending_action = None;
                    app.pending_paste = None;
                    app.cancel_preflight();
                    app.stop_ai_request();
                    // Menu targets are captured before the native pump; preserve their
                    // epoch/selection while suppressing in-flight network publication.
                    semantic_revoked = app.prepare_semantic_effect(matches!(
                        effect,
                        UiEffect::Menu { .. } | UiEffect::ClipboardMetadata { .. }
                    ));
                    app.state.modal_open = !matches!(effect, UiEffect::Destroy(_));
                }
                effect
            };
            let Some(effect) = effect else {
                break;
            };
            match effect {
                UiEffect::OrganizerSettings { owner, settings } => {
                    let (appearance, ai, path_enabled) = {
                        let app = self.app.borrow();
                        (
                            app.settings.theme,
                            app.settings.ai.clone(),
                            app.settings.path_activity.enabled,
                        )
                    };
                    let runtime_health = self.app.borrow().runtime_health.clone();
                    let values = dialogs::organizer_settings_form(
                        owner,
                        &settings,
                        &ai,
                        path_enabled,
                        appearance,
                        runtime_health,
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if app.state.lifecycle == Lifecycle::Running
                        && let Some(values) = values
                    {
                        match crate::organizer_ui::parse_organizer_fields(&values) {
                            Ok(settings) => {
                                let mut next = app.settings.clone();
                                next.organizer = settings;
                                app.cancel_organizer_for_settings(&next);
                                app.settings = next;
                                app.update_organizer_timer();
                                app.send(Command::SaveSettings(app.settings.clone()));
                            }
                            Err(message) => {
                                let mut review = app.organizer_review.borrow_mut();
                                review.message = message;
                                review.revision += 1;
                            }
                        }
                    }
                }
                UiEffect::HarnessMemory {
                    owner,
                    state,
                    outbox,
                    appearance,
                } => {
                    let address = owner.0 as usize;
                    super::harness_memory_ui::show(
                        owner,
                        state,
                        Rc::new(move |command| {
                            outbox.borrow_mut().0 = Some(command);
                            let _ = PostMessageW(
                                Some(HWND(address as *mut _)),
                                ORGANIZER_UI_WAKE,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }),
                        appearance,
                    );
                    self.app.borrow_mut().state.modal_open = false;
                }
                UiEffect::OrganizerReview {
                    owner,
                    state,
                    outbox,
                    appearance,
                    enabled,
                } => {
                    let address = owner.0 as usize;
                    let memory = self.app.borrow().harness_memory.clone();
                    organizer_ui::show(
                        owner,
                        state,
                        memory,
                        Rc::new(move |command| {
                            let detail = matches!(command, Command::OrganizerDetail { .. });
                            let mut slot = outbox.borrow_mut();
                            if detail {
                                slot.1 = Some(command);
                            } else {
                                slot.0 = Some(command);
                            }
                            drop(slot);
                            let _ = PostMessageW(
                                Some(HWND(address as *mut _)),
                                ORGANIZER_UI_WAKE,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }),
                        appearance,
                        enabled,
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    app.organizer_page_open = false;
                }
                UiEffect::ScreenshotSelect {
                    owner,
                    target,
                    ticket,
                } => {
                    // All callback-capable native calls run after dropping the App borrow.
                    let _ = ShowWindow(owner, SW_HIDE);
                    let result = screenshot::select_and_capture(&ticket, target);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    semantic_revoked = false;
                    if !ticket.current() || app.state.lifecycle != Lifecycle::Running {
                        app.screenshot_gate.finish(&ticket);
                    } else {
                        match result {
                            Ok(Some(pixels)) if app.screenshot_gate.encoding(&ticket) => {
                                if !app.screenshot_worker.as_ref().is_some_and(|worker| {
                                    worker.submit(ScreenshotJob {
                                        ticket: ticket.clone(),
                                        pixels,
                                    })
                                }) {
                                    app.screenshot_gate.finish(&ticket);
                                    app.effects.push_back(UiEffect::Message {
                                        owner,
                                        message: "无法启动截图编码，请重试；剪贴板保持原样".into(),
                                    });
                                }
                            }
                            Ok(_) => {
                                app.screenshot_gate.finish(&ticket);
                            }
                            Err(message) => {
                                app.screenshot_gate.finish(&ticket);
                                app.effects.push_back(UiEffect::Message { owner, message });
                            }
                        }
                    }
                }
                UiEffect::ScreenshotDeliver {
                    owner,
                    ticket,
                    prepared,
                } => {
                    let result = clipboard::publish_screenshot(owner, &ticket, prepared);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    semantic_revoked = false;
                    app.screenshot_gate.finish(&ticket);
                    match result {
                        clipboard::ScreenshotWrite::Copied {
                            sequence,
                            snapshot,
                            mut warning,
                        } => {
                            // Pair the exact self-write sequence with one explicit PNG capture.
                            app.suppress_sequence = Some(sequence);
                            app.last_capture_sequence = sequence;
                            let allowed =
                                !app.settings.capture_paused
                                    && !app.settings.excluded_apps.iter().any(|name| {
                                        name.eq_ignore_ascii_case(&snapshot.source_app)
                                    });
                            if allowed {
                                let queued = app
                                    .engine
                                    .as_ref()
                                    .ok_or_else(|| "历史引擎不可用".to_owned())
                                    .and_then(|engine| engine.send(Command::Capture(snapshot)));
                                if let Err(error) = queued {
                                    let message = format!("截图已复制，但历史未入队：{error}");
                                    warning = Some(warning.map_or(message.clone(), |old| {
                                        format!("{old}\n{message}")
                                    }));
                                }
                            }
                            app.status(if allowed {
                                "截图已复制"
                            } else {
                                "截图已复制；本次未记录历史"
                            });
                            if let Some(message) = warning {
                                app.effects.push_back(UiEffect::Message { owner, message });
                            }
                        }
                        clipboard::ScreenshotWrite::NotCopied {
                            changed_sequence,
                            message,
                        } => {
                            if let Some(sequence) = changed_sequence {
                                app.suppress_sequence = Some(sequence);
                                app.last_capture_sequence = sequence;
                            }
                            if let Some(message) = message {
                                app.effects.push_back(UiEffect::Message { owner, message });
                            }
                        }
                    }
                }
                UiEffect::ScreenshotSettings { owner, settings } => {
                    let result = dialogs::screenshot_settings_form(
                        owner,
                        format_hotkey(
                            settings.screenshot_hotkey_modifiers,
                            settings.screenshot_hotkey_key,
                        ),
                        settings.theme,
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    let error =
                        result.and_then(|values| app.apply_screenshot_settings(values).err());
                    app.restore_after_modal(true);
                    semantic_revoked = false;
                    if let Some(message) = error {
                        app.effects.push_back(UiEffect::Message { owner, message });
                    }
                }
                UiEffect::Picker {
                    owner,
                    folder,
                    item,
                } => {
                    let result = dialogs::pick(owner, folder);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    match result {
                        Ok(Some(path)) => app.edit_favorite(item, Some(path)),
                        Ok(None) => {}
                        Err(error) => app.status(&error),
                    }
                }
                UiEffect::Favorite {
                    owner,
                    item,
                    kind,
                    fields,
                } => {
                    let appearance = self.app.borrow().settings.theme;
                    let result = dialogs::form(owner, "编辑收藏", &fields, appearance);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if let Some(values) = result {
                        app.apply_favorite(item, kind, values);
                    }
                }
                UiEffect::Settings { owner, settings } => {
                    let result = dialogs::settings_form(
                        owner,
                        match settings.theme {
                            Theme::System => 0,
                            Theme::Light => 1,
                            Theme::Dark => 2,
                        },
                        format_hotkey(settings.hotkey_modifiers, settings.hotkey_key),
                        format_hotkey(
                            settings.clipboard_hotkey_modifiers,
                            settings.clipboard_hotkey_key,
                        ),
                        settings.capture_paused,
                        settings.excluded_apps.join(", "),
                        settings.smart_search,
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if let Some(values) = result {
                        app.apply_settings(values);
                    }
                    // Cancel and both validation/registration early returns also reload.
                    app.restore_after_modal(true);
                    semantic_revoked = false;
                }
                UiEffect::PathSettings { owner, settings } => {
                    let appearance = self.app.borrow().settings.theme;
                    let result = dialogs::path_settings_form(owner, &settings, appearance);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    let error = result.and_then(|values| app.apply_path_settings(values).err());
                    app.restore_after_modal(true);
                    semantic_revoked = false;
                    if let Some(message) = error {
                        app.status(&message);
                    }
                }
                UiEffect::AiSettings { owner, settings } => {
                    let appearance = self.app.borrow().settings.theme;
                    let result = dialogs::ai_settings_form(owner, &settings, appearance);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    let error = result.and_then(|values| app.apply_ai_settings(values).err());
                    // Opening this form revoked its adopted plan. Cancel and validation
                    // failure also need a fresh ordinary query rather than derived rows.
                    app.refresh_semantic_status();
                    app.search();
                    semantic_revoked = false;
                    if let Some(message) = error {
                        app.effects.push_back(UiEffect::Message { owner, message });
                    }
                }
                UiEffect::EmbeddingSettings { owner, settings } => {
                    let appearance = self.app.borrow().settings.theme;
                    let result = dialogs::embedding_settings_form(owner, &settings, appearance);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    let error =
                        result.and_then(|values| app.apply_embedding_settings(values).err());
                    app.refresh_semantic_status();
                    app.search();
                    semantic_revoked = false;
                    if let Some(message) = error {
                        app.effects.push_back(UiEffect::Message { owner, message });
                    }
                }
                UiEffect::SmartPrompt { owner, preferred } => {
                    let appearance = self.app.borrow().settings.theme;
                    let result = dialogs::form(
                        owner,
                        "智能查找",
                        &[("描述想找的内容（点击保存后发送本次查询）", String::new())],
                        appearance,
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if let Some(values) = result {
                        let text = wide(&values[0]);
                        let _ = SetWindowTextW(app.edit, PCWSTR(text.as_ptr()));
                        app.search();
                        app.begin_smart(preferred);
                        semantic_revoked = false;
                    }
                }
                UiEffect::ClipboardMetadata {
                    owner,
                    form,
                    target,
                } => {
                    let appearance = self.app.borrow().settings.theme;
                    let result = dialogs::clipboard_metadata_form(owner, &form.summary, appearance);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    match form.finish(result, app.state.menu_target_current(&target)) {
                        Ok(Some(input)) => app.send(Command::EditClipboardMetadata(input)),
                        Ok(None) => (),
                        Err(message) => app.status(&message),
                    }
                }
                UiEffect::ExitError { owner, message } => {
                    let text = wide(&format!(
                        "保存失败，数据仍保留在内存中。\n{message}\n\n重试保存，或取消退出？"
                    ));
                    let answer = MessageBoxW(
                        Some(owner),
                        PCWSTR(text.as_ptr()),
                        w!("保存后退出"),
                        MB_RETRYCANCEL | MB_ICONERROR,
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if answer == IDRETRY {
                        app.state.retry_exit();
                        app.send(Command::Shutdown);
                    } else {
                        app.send(Command::CancelShutdown);
                        app.state.cancel_exit();
                        app.update_organizer_timer();
                        app.screenshot_gate.resume();
                        app.update_path_monitor();
                        app.last_capture_sequence = 0;
                        app.capture();
                        app.show(None);
                    }
                }
                UiEffect::Message { owner, message } => {
                    let text = wide(&message);
                    MessageBoxW(
                        Some(owner),
                        PCWSTR(text.as_ptr()),
                        w!("快捷面板"),
                        MB_OK | MB_ICONWARNING,
                    );
                    self.app.borrow_mut().state.modal_open = false;
                }
                UiEffect::Menu {
                    owner,
                    x,
                    y,
                    entries,
                    target,
                } => {
                    // Keep the pre-menu app for tray screenshot cancellation; this menu
                    // temporarily makes its (possibly hidden) owner foreground.
                    let screenshot_target = GetForegroundWindow();
                    if let Ok(menu) = CreatePopupMenu() {
                        for (id, title) in entries {
                            let text = wide(title);
                            let _ = AppendMenuW(menu, MF_STRING, id, PCWSTR(text.as_ptr()));
                        }
                        let _ = SetForegroundWindow(owner);
                        let command = TrackPopupMenu(
                            menu,
                            TPM_RETURNCMD | TPM_RIGHTBUTTON,
                            x,
                            y,
                            None,
                            owner,
                            None,
                        )
                        .0 as usize;
                        let _ = DestroyMenu(menu);
                        {
                            self.app.borrow_mut().state.modal_open = false;
                        }
                        // Cancellation may have consumed WA_INACTIVE inside the menu pump.
                        // Read Win32 focus/ownership without retaining any App borrow.
                        let foreground = GetForegroundWindow();
                        let in_panel = self.is_panel_surface(foreground);
                        let mut process = 0u32;
                        GetWindowThreadProcessId(foreground, Some(&mut process));
                        let in_owned_modal = !foreground.is_invalid()
                            && process == windows::Win32::System::Threading::GetCurrentProcessId()
                            && GetAncestor(foreground, GA_ROOTOWNER) == owner;
                        let mut app = self.app.borrow_mut();
                        if app
                            .state
                            .menu_cancel_should_hide(command != 0, in_panel, in_owned_modal)
                        {
                            app.hide();
                        } else if command != 0 {
                            if target
                                .as_ref()
                                .is_none_or(|target| app.state.menu_target_current(target))
                            {
                                if command == SCREENSHOT {
                                    let target = if app.state.visible || screenshot_target == owner
                                    {
                                        app.target
                                    } else {
                                        screenshot_target
                                    };
                                    app.begin_screenshot_for(Some(target));
                                } else {
                                    app.command(command);
                                }
                            } else {
                                app.status("结果已更新，请重新选择后操作");
                            }
                        }
                    } else {
                        self.app.borrow_mut().state.modal_open = false;
                    }
                }
                UiEffect::Destroy(hwnd) => {
                    let _ = DestroyWindow(hwnd);
                }
            }
            // One completion gate covers Picker/Favorite cancellation, errors,
            // validation early returns, and other effects that revoked rows.
            if semantic_revoked {
                self.app.borrow_mut().restore_after_modal(true);
            }
        }
        self.effects_running.set(false);
    }
}
