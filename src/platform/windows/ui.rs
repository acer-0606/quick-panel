#![allow(unsafe_op_in_unsafe_fn)]
use super::{
    actions, ai_client, ai_time, clipboard,
    dialogs::{self, wide},
    theme,
};
use crate::{
    ai::{AiPlan, AiSettings, AuthMethod},
    ai_jobs::{AiRequest, AiResultGate, AiWorker, revoke_adopted_plan},
    engine::Engine,
    hotkeys::{HotkeyBindings, format_hotkey, parse_hotkey, settings_hotkeys},
    model::*,
    palette::{self, Context as PaletteContext, Entry as PaletteEntry, Function},
    ui_state::{KeyAction, Lifecycle, MenuTarget, PanelKey, UiState},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::Path,
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Gdi::*, Imaging::*},
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
const SEARCH: usize = 100;
const LIST: usize = 101;
const ADD: usize = 102;
const PREVIEW: usize = 103;
const SETTINGS: usize = 104;
const AI_FIND: usize = 105;
const GENERAL_SETTINGS: usize = 106;
const AI_SETTINGS: usize = 107;
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
}
struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}
enum Work {
    Capture(u32),
    Stop,
}
enum Done {
    Captured(Result<Option<ClipboardSnapshot>, String>),
    Preview(u64, Result<Image, String>),
}
struct PreviewQueue {
    job: Mutex<Option<(u64, StoredItem)>>,
    wake: Condvar,
    generation: AtomicU64,
    stop: AtomicBool,
}
impl PreviewQueue {
    fn replace(&self, id: u64, item: StoredItem) {
        self.generation.store(id, Ordering::Release);
        if let Ok(mut job) = self.job.lock() {
            *job = Some((id, item));
            self.wake.notify_one();
        }
    }
    fn cancel(&self, id: u64) {
        self.generation.store(id, Ordering::Release);
        if let Ok(mut job) = self.job.lock() {
            *job = None;
        }
    }
}
#[derive(Clone)]
struct RenderState {
    dpi: u32,
    theme: Theme,
    font: HFONT,
    brush: HBRUSH,
    items: Vec<PaletteEntry>,
    image: Option<Arc<Image>>,
}
impl RenderState {
    fn s(&self, n: i32) -> i32 {
        n * self.dpi as i32 / 96
    }
}
enum UiEffect {
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
    AiSettings {
        owner: HWND,
        settings: AiSettings,
    },
    AiPrompt {
        owner: HWND,
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
struct App {
    host: *const Host,
    effects: VecDeque<UiEffect>,
    hotkey_bindings: HotkeyBindings,
    hwnd: HWND,
    edit: HWND,
    list: HWND,
    footer: HWND,
    preview: HWND,
    ai_button: HWND,
    buttons: Vec<HWND>,
    font: HFONT,
    brush: HBRUSH,
    engine: Option<Engine>,
    state: UiState,
    settings: Settings,
    items: Vec<PaletteEntry>,
    saved_items: Vec<ItemSummary>,
    saved_ready: bool,
    query: String,
    result_scope: Scope,
    ai_worker: Option<AiWorker>,
    ai_gate: AiResultGate,
    ai_plan: Option<AiPlan>,
    scope: Scope,
    target: HWND,
    load_id: u64,
    pending_action: Option<(u64, Action)>,
    pending_paste: Option<(u64, StoredItem, Action, HWND)>,
    preview_id: u64,
    preview_open: bool,
    image: Option<Arc<Image>>,
    tx: mpsc::Sender<Work>,
    rx: mpsc::Receiver<Done>,
    capture_worker: Option<std::thread::JoinHandle<()>>,
    preview_queue: Arc<PreviewQueue>,
    preview_worker: Option<std::thread::JoinHandle<()>>,
    dpi: u32,
    suppress_sequence: Option<u32>,
    last_capture_sequence: u32,
}
unsafe fn post(handle: usize, msg: u32) {
    let _ = PostMessageW(Some(HWND(handle as *mut _)), msg, WPARAM(0), LPARAM(0));
}
unsafe fn decode(item: &StoredItem, id: u64, generation: &AtomicU64) -> Result<Image, String> {
    if generation.load(Ordering::Acquire) != id {
        return Err("预览已取消".into());
    }
    let rep = item
        .representations
        .iter()
        .find(|r| r.format == "PNG")
        .or_else(|| {
            item.representations
                .iter()
                .find(|r| r.format == "CF_DIBV5" || r.format == "CF_DIB")
        })
        .ok_or("该记录没有可解码的图片")?;
    let mut data = rep.data.clone();
    if rep.format != "PNG" {
        if data.len() < 40 {
            return Err("图片数据不完整".into());
        }
        let u32at = |n: usize| u32::from_le_bytes(data[n..n + 4].try_into().unwrap());
        let header = u32at(0) as usize;
        let bpp = u16::from_le_bytes(data[14..16].try_into().unwrap());
        let compression = u32at(16);
        let colors = u32at(32) as usize;
        let palette = if bpp <= 8 {
            if colors == 0 { 1usize << bpp } else { colors }
        } else {
            colors
        };
        let offset = 14usize
            .checked_add(header)
            .and_then(|v| v.checked_add(palette * 4))
            .and_then(|v| {
                v.checked_add(if header == 40 && compression == 3 {
                    12
                } else {
                    0
                })
            })
            .ok_or("图片尺寸溢出")?;
        if offset > data.len() + 14 {
            return Err("图片头无效".into());
        }
        let mut bmp = Vec::with_capacity(data.len() + 14);
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&((data.len() + 14) as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&(offset as u32).to_le_bytes());
        bmp.append(&mut data);
        data = bmp;
    }
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
    let stream = factory.CreateStream().map_err(|e| e.to_string())?;
    stream
        .InitializeFromMemory(&data)
        .map_err(|e| e.to_string())?;
    let decoder = factory
        .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
        .map_err(|e| e.to_string())?;
    let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
    let (mut width, mut height) = (0, 0);
    frame
        .GetSize(&mut width, &mut height)
        .map_err(|e| e.to_string())?;
    let bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or("图片尺寸溢出")?;
    if bytes > 16 * 1024 * 1024 || width == 0 || height == 0 {
        return Err("图片解码超过16 MiB预算；仍可复制原图".into());
    }
    let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
    converter
        .Initialize(
            &frame,
            &GUID_WICPixelFormat32bppBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(|e| e.to_string())?;
    if generation.load(Ordering::Acquire) != id {
        return Err("预览已取消".into());
    }
    let mut pixels = vec![0; bytes];
    converter
        .CopyPixels(std::ptr::null(), width * 4, &mut pixels)
        .map_err(|e| e.to_string())?;
    Ok(Image {
        width,
        height,
        pixels,
    })
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
            host: std::ptr::null(),
            effects: VecDeque::new(),
            hotkey_bindings: HotkeyBindings::default(),
            hwnd: HWND::default(),
            edit: HWND::default(),
            list: HWND::default(),
            footer: HWND::default(),
            preview: HWND::default(),
            ai_button: HWND::default(),
            buttons: Vec::new(),
            font: HFONT::default(),
            brush: HBRUSH::default(),
            engine: None,
            state: UiState::default(),
            settings: Settings::default(),
            items: Vec::new(),
            saved_items: Vec::new(),
            saved_ready: false,
            query: String::new(),
            result_scope: Scope::All,
            ai_worker: None,
            ai_gate: AiResultGate::default(),
            ai_plan: None,
            scope: Scope::All,
            target: HWND::default(),
            load_id: 0,
            pending_action: None,
            pending_paste: None,
            preview_id: 0,
            preview_open: false,
            image: None,
            tx,
            rx,
            capture_worker: None,
            preview_queue: Arc::new(PreviewQueue {
                job: Mutex::new(None),
                wake: Condvar::new(),
                generation: AtomicU64::new(0),
                stop: AtomicBool::new(false),
            }),
            preview_worker: None,
            dpi: 96,
            suppress_sequence: None,
            last_capture_sequence: 0,
        };
        let host = Box::new(Host {
            app: RefCell::new(app_data),
            render: RefCell::new(Rc::new(RenderState {
                dpi: 96,
                theme: Theme::System,
                font: HFONT::default(),
                brush: HBRUSH::default(),
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
        let preview_queue = app.preview_queue.clone();
        app.preview_worker = Some(std::thread::spawn(move || {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            loop {
                let mut slot = match preview_queue.job.lock() {
                    Ok(slot) => slot,
                    Err(_) => break,
                };
                while slot.is_none() && !preview_queue.stop.load(Ordering::Acquire) {
                    slot = match preview_queue.wake.wait(slot) {
                        Ok(slot) => slot,
                        Err(_) => return,
                    };
                }
                if preview_queue.stop.load(Ordering::Acquire) {
                    break;
                }
                let Some((id, item)) = slot.take() else {
                    continue;
                };
                drop(slot);
                let result = decode(&item, id, &preview_queue.generation);
                if id == preview_queue.generation.load(Ordering::Acquire) {
                    if done_tx.send(Done::Preview(id, result)).is_err() {
                        break;
                    }
                    post(handle, WORK_WAKE);
                }
            }
            CoUninitialize();
        }));
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
        app.preview_queue.stop.store(true, Ordering::Release);
        app.preview_queue.wake.notify_all();
        if let Some(worker) = app.preview_worker.take() {
            let _ = worker.join();
        }
        CoUninitialize();
        Ok(())
    }
}
impl App {
    fn sync_render(&self) {
        unsafe {
            if let Some(host) = self.host.as_ref() {
                *host.render.borrow_mut() = Rc::new(RenderState {
                    dpi: self.dpi,
                    theme: self.settings.theme,
                    font: self.font,
                    brush: self.brush,
                    items: self.items.clone(),
                    image: self.image.clone(),
                });
            }
        }
    }
    fn cancel_actions(&mut self) {
        self.load_id = self.load_id.wrapping_add(1);
        self.pending_action = None;
        self.pending_paste = None;
        self.state.target_changed();
    }
    unsafe fn stop_ai_request(&mut self) {
        if let Some(worker) = self.ai_worker.as_ref() {
            worker.cancel();
        }
        self.ai_gate.cancel();
        let _ = SetWindowTextW(self.ai_button, w!("按意思查找"));
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
    }
    unsafe fn begin_ai(&mut self) {
        if self.state.lifecycle != Lifecycle::Running
            || self.state.modal_open
            || !self.state.visible
        {
            return;
        }
        self.cancel_actions();
        if self.ai_gate.busy() {
            self.cancel_ai();
            self.search();
            self.status("已取消 AI 检索，保留本地检索");
            return;
        }
        if !self.settings.ai.enabled {
            self.ai_settings();
            return;
        }
        let query = dialogs::text(self.edit);
        if ["按意思查找", "AI检索", "AI搜索", "智能查找"].contains(&query.trim()) {
            self.effects
                .push_back(UiEffect::AiPrompt { owner: self.hwnd });
            return;
        }
        if query.trim().is_empty() || query.chars().count() > 2048 {
            self.status("请输入不超过 2048 字的检索描述");
            return;
        }
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
                let _ = SetWindowTextW(self.ai_button, w!("取消 AI"));
                self.status("AI 正在理解查询…普通结果仍可使用");
            }
            Err(message) => self.status(&message),
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
        let _ = SetWindowTextW(self.ai_button, w!("按意思查找"));
        match completion.result {
            Ok(plan) => {
                self.ai_plan = Some(plan);
                self.saved_items.clear();
                self.saved_ready = false;
                self.search();
            }
            Err(message) => self.status(&format!("{message} · 本地检索仍可使用")),
        }
    }
    unsafe fn clear_preview(&mut self) {
        self.load_id = self.load_id.wrapping_add(1);
        self.preview_id = self.load_id;
        self.image = None;
        self.preview_queue.cancel(self.preview_id);
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
        self.preview = self.control(w!("STATIC"), "", WINDOW_STYLE(0), 110)?;
        self.ai_button = self.control(w!("BUTTON"), "按意思查找", WS_TABSTOP, AI_FIND)?;
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
        if !self.brush.is_invalid() {
            let _ = DeleteObject(self.brush.into());
        }
        self.font = theme::font(self.dpi);
        self.brush = CreateSolidBrush(theme::palette(self.settings.theme).background);
        self.sync_render();
        theme::apply(self.hwnd, self.settings.theme);
        for h in [self.edit, self.list, self.footer, self.preview]
            .into_iter()
            .chain(self.buttons.iter().copied())
            .chain(std::iter::once(self.ai_button))
        {
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        let _ = ShowWindow(
            self.ai_button,
            if self.settings.ai.enabled {
                SW_SHOW
            } else {
                SW_HIDE
            },
        );
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
        let ai_width = if self.settings.ai.enabled {
            self.s(114)
        } else {
            0
        };
        mov(
            self.edit,
            edge,
            self.s(18),
            (w - self.s(126) - ai_width).max(self.s(80)),
            self.s(36),
        );
        mov(
            self.ai_button,
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
        mov(self.list, edge, self.s(110), list_width, h - self.s(165));
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
        self.search();
    }
    unsafe fn hide(&mut self) {
        self.cancel_ai();
        self.cancel_actions();
        self.clear_preview();
        self.state.hide();
        let _ = ShowWindow(self.hwnd, SW_HIDE);
    }
    unsafe fn search(&mut self) {
        if self.state.lifecycle != Lifecycle::Running {
            return;
        }
        self.cancel_actions();
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
        if query != self.query || self.scope != self.result_scope {
            self.cancel_ai();
            self.saved_items.clear();
            self.saved_ready = false;
        }
        self.query = query;
        self.result_scope = self.scope;
        let id = self.state.next_request();
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
        };
        self.items = if let Some(plan) = self.ai_plan.as_ref() {
            palette::ai_results(plan, self.saved_items.clone(), context)
        } else {
            palette::results(&self.query, self.saved_items.clone(), context)
        };
        self.state.replace_ids(
            self.items
                .iter()
                .map(|entry| entry.id().to_owned())
                .collect(),
        );
        self.sync_render();
        SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(self.list, LB_RESETCONTENT, None, None);
        for item in &self.items {
            let title = wide(item.title());
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
        SendMessageW(self.list, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = InvalidateRect(Some(self.list), None, true);
        self.selection();
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
        self.ai_done();
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
                Done::Preview(id, result) => {
                    if id == self.preview_id && self.preview_open && self.state.visible {
                        match result {
                            Ok(image) => {
                                self.image = Some(Arc::new(image));
                                self.sync_render();
                                let _ = SetWindowTextW(self.preview, w!(""));
                            }
                            Err(e) => {
                                let text = wide(&e);
                                let _ = SetWindowTextW(self.preview, PCWSTR(text.as_ptr()));
                            }
                        }
                        let _ = InvalidateRect(Some(self.preview), None, true);
                    }
                }
            }
        }
    }
    unsafe fn events(&mut self) {
        while let Some(event) = self.engine.as_ref().and_then(|e| e.try_recv().ok()) {
            match event {
                Event::Ready(settings) => {
                    self.settings = settings;
                    self.retheme();
                    if let Err(e) = self.replace_hotkeys(&self.settings.clone()) {
                        self.show(None);
                        self.effects.push_back(UiEffect::Message {
                            owner: self.hwnd,
                            message: e,
                        });
                    }
                    self.tray(NIM_MODIFY);
                    self.search();
                }
                Event::Results { request_id, items } => {
                    if self.state.is_current(request_id) {
                        self.saved_items = items;
                        self.saved_ready = true;
                        self.rebuild_results();
                    }
                }
                Event::Loaded { request_id, item } => {
                    if let Some(item) = item {
                        if self
                            .pending_action
                            .as_ref()
                            .is_some_and(|(id, _)| *id == request_id)
                        {
                            if GetForegroundWindow() == self.hwnd
                                && self.state.action_current(request_id)
                            {
                                if let Some((_, action)) = self.pending_action.take() {
                                    if matches!(action, Action::Open(_))
                                        && item.summary.kind == ItemKind::Clipboard
                                    {
                                        self.pending_paste =
                                            Some((request_id, item, action, self.target));
                                        self.release_paste();
                                    } else if self.state.finish_action(request_id) {
                                        self.perform(item, action);
                                    }
                                }
                            } else {
                                self.cancel_actions();
                            }
                        } else if self.state.visible
                            && self.state.lifecycle == Lifecycle::Running
                            && !self.state.modal_open
                            && self.preview_open
                            && request_id == self.preview_id
                        {
                            self.perform(item, Action::Preview);
                        }
                    }
                }
                Event::Changed => {
                    if self.state.visible {
                        self.search();
                    }
                }
                Event::Saved(_) => {
                    if self.state.lifecycle == Lifecycle::Running {
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
        self.cancel_actions();
        self.clear_preview();
        let i = SendMessageW(self.list, LB_GETCURSEL, None, None).0;
        if i >= 0 {
            self.state.select(i as usize);
        } else {
            self.state.select(usize::MAX);
            self.status(if self.saved_ready {
                "没有匹配结果 · 可换个关键词或添加收藏"
            } else {
                "正在检索保存内容…"
            });
            return;
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
        if self.preview_open && !function {
            self.request(Action::Preview);
        }
        if self.ai_gate.busy() {
            self.status("AI 正在理解查询…可点击取消 AI");
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
        if let Some(id) = self.state.selected_id().map(str::to_owned) {
            self.load_id = self.load_id.wrapping_add(1);
            if matches!(action, Action::Preview) {
                self.preview_id = self.load_id;
                self.image = None;
                self.preview_queue.cancel(self.preview_id);
                self.sync_render();
            } else {
                self.pending_paste = None;
                if !self.state.begin_action(self.load_id) {
                    return;
                }
                self.pending_action = Some((self.load_id, action));
            }
            self.send(Command::Load {
                id,
                request_id: self.load_id,
            });
        }
    }
    unsafe fn release_paste(&mut self) {
        let Some((request_id, _, _, target)) = self.pending_paste.as_ref() else {
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
            && let Some((_, item, action, _)) = self.pending_paste.take()
        {
            self.perform(item, action);
        }
    }
    unsafe fn perform(&mut self, item: StoredItem, action: Action) {
        match action {
            Action::Open(plain) => {
                if item.summary.kind == ItemKind::Clipboard {
                    let result = clipboard::restore(self.hwnd, &item, plain);
                    self.suppress_sequence = Some(clipboard::sequence_number());
                    match result {
                        Ok(()) => {
                            self.hide();
                            if let Err(e) = clipboard::paste_to(self.target) {
                                self.show(None);
                                self.status(&e);
                            }
                        }
                        Err(e) => self.status(&e),
                    }
                } else if let Err(e) = actions::open_target(&item) {
                    self.status(&e);
                    if item.target.as_ref().is_some_and(|p| !Path::new(p).exists()) {
                        self.perform(item, Action::Relink);
                    }
                    return;
                }
                self.send(Command::Used(item.summary.id));
            }
            Action::Copy(plain) => {
                if item.summary.kind != ItemKind::Clipboard {
                    self.perform(item, Action::CopyPath);
                    return;
                }
                let result = clipboard::restore(self.hwnd, &item, plain);
                self.suppress_sequence = Some(clipboard::sequence_number());
                match result {
                    Ok(()) => self.status("已复制"),
                    Err(e) => self.status(&e),
                }
            }
            Action::Pin => self.send(Command::Pin {
                id: item.summary.id,
                pinned: !item.summary.pinned,
            }),
            Action::Delete => self.send(Command::Delete(item.summary.id)),
            Action::Reveal => {
                if let Err(e) = actions::reveal_target(&item) {
                    self.status(&e);
                }
            }
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
                self.edit_favorite(Some(item), None);
            }
            Action::Relink => {
                self.effects.push_back(UiEffect::Picker {
                    owner: self.hwnd,
                    folder: item.summary.kind == ItemKind::Folder,
                    item: Some(item),
                });
            }
            Action::Preview => {
                self.image = None;
                let text = if item.summary.kind == ItemKind::Clipboard {
                    item.text.chars().take(6000).collect::<String>()
                } else {
                    let path = item.target.clone().unwrap_or_default();
                    let metadata = std::fs::metadata(&path);
                    format!(
                        "{}\n\n{}\n\n{}",
                        item.summary.title,
                        path,
                        match metadata {
                            Ok(m) => format!(
                                "大小：{} 字节\n标签：{}",
                                m.len(),
                                item.summary.tags.join("、")
                            ),
                            Err(_) => "目标已失效 · Ctrl+K → 重新定位".into(),
                        }
                    )
                };
                let t = wide(&text);
                let _ = SetWindowTextW(self.preview, PCWSTR(t.as_ptr()));
                if item
                    .representations
                    .iter()
                    .any(|r| r.format == "PNG" || r.format == "CF_DIB" || r.format == "CF_DIBV5")
                {
                    self.status("正在生成图片预览…");
                    self.preview_queue.replace(self.preview_id, item);
                }
            }
        }
    }
    unsafe fn edit_favorite(&mut self, item: Option<StoredItem>, target: Option<String>) {
        let kind = item.as_ref().map(|i| i.summary.kind).unwrap_or_else(|| {
            if target.as_ref().is_some_and(|p| Path::new(p).is_dir()) {
                ItemKind::Folder
            } else {
                ItemKind::Document
            }
        });
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
    unsafe fn settings(&mut self) {
        self.effects.push_back(UiEffect::Settings {
            owner: self.hwnd,
            settings: self.settings.clone(),
        });
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
                (AI_SETTINGS, "AI 检索设置…"),
            ],
            target: None,
        });
    }
    unsafe fn ai_settings(&mut self) {
        self.cancel_ai();
        self.effects.push_back(UiEffect::AiSettings {
            owner: self.hwnd,
            settings: self.settings.ai.clone(),
        });
    }
    unsafe fn apply_ai_settings(&mut self, mut values: Vec<String>) -> Result<(), String> {
        let password = ai_client::SecretString(std::mem::take(&mut values[4]));
        if self.state.lifecycle != Lifecycle::Running {
            return Ok(());
        }
        let mut next = self.settings.ai.clone();
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
        self.settings.ai = next;
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
        self.settings = settings.clone();
        self.retheme();
        self.tray(NIM_MODIFY);
        self.send(Command::SaveSettings(settings));
        self.search();
    }
    unsafe fn context(&mut self, x: i32, y: i32, tray: bool, add: bool) {
        let entries: Vec<(usize, &str)> = if tray {
            vec![
                (221, "打开面板"),
                (GENERAL_SETTINGS, "常规设置…"),
                (AI_SETTINGS, "AI 检索设置…"),
                (222, "暂停 / 恢复记录"),
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
            .and_then(PaletteEntry::saved)
            .is_some_and(|i| i.kind == ItemKind::Clipboard)
        {
            vec![
                (OPEN, "粘贴"),
                (COPY, "复制"),
                (PLAIN, "纯文本粘贴"),
                (PIN, "固定 / 取消固定"),
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
        if function != Function::Exit && function != Function::AiSearch {
            // Result commands return to browsing content. Scope and preview
            // commands read the cleared native Edit through their normal handler.
            let _ = SetWindowTextW(self.edit, w!(""));
        }
        match function {
            Function::Settings => self.settings(),
            Function::AiSettings => self.ai_settings(),
            Function::AiSearch => self.begin_ai(),
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
                self.clear_preview();
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
                self.show(None);
                if self.preview_open {
                    self.request(Action::Preview);
                }
            }
            SETTINGS => self.settings_menu(),
            GENERAL_SETTINGS => self.settings(),
            AI_SETTINGS => self.ai_settings(),
            AI_FIND => self.begin_ai(),
            300..=303 => {
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
                self.cancel_ai();
                self.cancel_actions();
                self.clear_preview();
                self.state.visible = true;
                let _ = ShowWindow(self.hwnd, SW_SHOWNA);
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
                    ENGINE_WAKE | WORK_WAKE | SHOW_EXISTING | WM_CLIPBOARDUPDATE | WM_HOTKEY
                    | WM_CLOSE | WM_ACTIVATE | WM_SIZE | WM_SETTINGCHANGE | WM_COMMAND
                    | WM_CONTEXTMENU | TRAY | WM_DROPFILES | WM_ENDSESSION | WM_DESTROY => {
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
            }
            return Some(LRESULT(0));
        }
        ENGINE_WAKE => {
            app.events();
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
                app.show((action == 1).then_some(Scope::Clipboard));
            }
            return Some(LRESULT(0));
        }
        WM_CLOSE => {
            app.hide();
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
        WM_SETTINGCHANGE => {
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
            for path in paths {
                app.edit_favorite(None, Some(path));
            }
            return Some(LRESULT(0));
        }
        WM_QUERYENDSESSION => return Some(LRESULT(1)),
        WM_DESTROY => {
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
            if !app.brush.is_invalid() {
                let _ = DeleteObject(app.brush.into());
            }
            PostQuitMessage(0);
            return Some(LRESULT(0));
        }
        _ => {}
    }
    None
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
                let p = theme::palette(render.theme);
                let selected = draw.itemState.0 & ODS_SELECTED.0 != 0;
                let brush = CreateSolidBrush(if selected { p.selected } else { p.surface });
                FillRect(draw.hDC, &draw.rcItem, brush);
                let _ = DeleteObject(brush.into());
                let _ = SetBkMode(draw.hDC, TRANSPARENT);
                let _ = SelectObject(draw.hDC, render.font.into());
                let kind = item.kind_label();
                let mut title = wide(&format!(
                    "{}{}",
                    if item.pinned() { "★ " } else { "" },
                    item.title()
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
                let mut subtitle = wide(&item.subtitle());
                rect.top += render.s(26);
                rect.bottom = rect.top + render.s(22);
                rect.right = draw.rcItem.right - render.s(14);
                DrawTextW(
                    draw.hDC,
                    &mut subtitle,
                    &mut rect,
                    DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
                );
                if selected {
                    let border = CreateSolidBrush(p.accent);
                    FrameRect(draw.hDC, &draw.rcItem, border);
                    let _ = DeleteObject(border.into());
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
            let p = theme::palette(render.theme);
            SetTextColor(dc, p.text);
            SetBkColor(dc, p.background);
            return Some(LRESULT(render.brush.0 as isize));
        }
        WM_ERASEBKGND => {
            let dc = HDC(wp.0 as *mut _);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            FillRect(dc, &r, render.brush);
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
        FillRect(dc, &r, render.brush);
        let ratio = (r.right as f64 / image.width as f64)
            .min(r.bottom as f64 / image.height as f64)
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
            (r.bottom - height) / 2,
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
            && (app.buttons.contains(&msg.hwnd) || msg.hwnd == app.ai_button)
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
                        continue;
                    }
                    app.stop_ai_request();
                    app.state.modal_open = !matches!(effect, UiEffect::Destroy(_));
                }
                effect
            };
            let Some(effect) = effect else {
                break;
            };
            match effect {
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
                    let result = dialogs::form(owner, "编辑收藏", &fields);
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
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if let Some(values) = result {
                        app.apply_settings(values);
                    }
                }
                UiEffect::AiSettings { owner, settings } => {
                    let result = dialogs::ai_settings_form(owner, &settings);
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    let error = result.and_then(|values| app.apply_ai_settings(values).err());
                    // Opening this form revoked its adopted plan. Cancel and validation
                    // failure also need a fresh ordinary query rather than derived rows.
                    app.search();
                    if let Some(message) = error {
                        app.effects.push_back(UiEffect::Message { owner, message });
                    }
                }
                UiEffect::AiPrompt { owner } => {
                    let result = dialogs::form(
                        owner,
                        "按意思查找",
                        &[("描述想找的内容或功能（仅此查询发送给 AI）", String::new())],
                    );
                    let mut app = self.app.borrow_mut();
                    app.state.modal_open = false;
                    if let Some(values) = result {
                        let text = wide(&values[0]);
                        let _ = SetWindowTextW(app.edit, PCWSTR(text.as_ptr()));
                        app.search();
                        app.begin_ai();
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
                                app.command(command);
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
        }
        self.effects_running.set(false);
    }
}
