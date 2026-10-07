//! WinEvent callbacks only coalesce notifications. All inspection stays on the MTA.
use super::{account_target::PasswordBounds, theme};
use crate::model::Theme;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::w,
};
static DIRTY: AtomicBool = AtomicBool::new(false);
static LAYOUT: AtomicBool = AtomicBool::new(false);
static TARGET: AtomicUsize = AtomicUsize::new(0);
static FOCUS: AtomicUsize = AtomicUsize::new(0);
static OWNER: AtomicUsize = AtomicUsize::new(0);
pub const HINT_CLICK: u32 = WM_APP + 94;
pub const CANCEL_TARGET: u32 = WM_APP + 95;
struct Guard {
    control: super::account_worker::AccountControl,
    epoch: u64,
    target: u64,
    own_pid: u32,
}
static GUARD: Mutex<Option<Guard>> = Mutex::new(None);
unsafe extern "system" fn event(
    _: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        let guard = GUARD.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(guard) = guard.as_ref() {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            if pid != guard.own_pid && hwnd.0 as u64 != guard.target {
                guard.control.invalidate(guard.epoch);
                let owner = OWNER.load(Ordering::Acquire);
                if owner != 0 {
                    unsafe {
                        let _ = PostMessageW(
                            Some(HWND(owner as *mut _)),
                            CANCEL_TARGET,
                            WPARAM(guard.control.generation() as usize),
                            LPARAM(0),
                        );
                    }
                }
            }
        }
    }
    if event != EVENT_SYSTEM_FOREGROUND && event != EVENT_OBJECT_FOCUS {
        if TARGET.load(Ordering::Relaxed) != hwnd.0 as usize
            && FOCUS.load(Ordering::Relaxed) != hwnd.0 as usize
        {
            return;
        }
        LAYOUT.store(true, Ordering::Release);
    }
    DIRTY.store(true, Ordering::Release);
}
struct Style {
    colors: theme::Colors,
    font: HFONT,
}
impl Drop for Style {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                (*(lp.0 as *const CREATESTRUCTW)).lpCreateParams as isize,
            );
        }
        match msg {
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONUP => {
                let owner = OWNER.load(Ordering::Acquire);
                if owner != 0 {
                    let _ = PostMessageW(
                        Some(HWND(owner as *mut _)),
                        HINT_CLICK,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                return LRESULT(0);
            }
            WM_PAINT => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Style;
                if !ptr.is_null() {
                    let style = &*ptr;
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(dc, &rect, style.colors.surface);
                    SetBkMode(dc, TRANSPARENT);
                    SetTextColor(dc, style.colors.palette.text);
                    let old = SelectObject(dc, style.font.into());
                    let mut label: Vec<u16> = "点击填入账号".encode_utf16().collect();
                    DrawTextW(
                        dc,
                        &mut label,
                        &mut rect,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                    SelectObject(dc, old);
                    let _ = EndPaint(hwnd, &paint);
                    return LRESULT(0);
                }
            }
            _ => (),
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
pub struct Hint {
    hooks: Vec<HWINEVENTHOOK>,
    hwnd: HWND,
    style: Box<Style>,
}
impl Default for Hint {
    fn default() -> Self {
        Self {
            hooks: vec![],
            hwnd: HWND::default(),
            style: Box::new(Style {
                colors: theme::Colors::new(Theme::System),
                font: unsafe { theme::font(96) },
            }),
        }
    }
}
impl Hint {
    pub fn guard(
        &self,
        control: Option<super::account_worker::AccountControl>,
        epoch: u64,
        target: Option<crate::account_fill::WindowIdentity>,
    ) {
        *GUARD.lock().unwrap_or_else(|p| p.into_inner()) =
            control.zip(target).map(|(control, target)| Guard {
                control,
                epoch,
                target: target.handle,
                own_pid: unsafe { windows::Win32::System::Threading::GetCurrentProcessId() },
            });
    }
    pub fn observe(&mut self, owner: HWND, enabled: bool) {
        unsafe {
            if !enabled {
                self.stop();
                return;
            }
            if !self.hooks.is_empty() {
                return;
            }
            OWNER.store(owner.0 as usize, Ordering::Release);
            for (first, last) in [
                (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
                (EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS),
                (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
                (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            ] {
                let hook = SetWinEventHook(
                    first,
                    last,
                    None,
                    Some(event),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                );
                if !hook.is_invalid() {
                    self.hooks.push(hook);
                }
            }
            DIRTY.store(true, Ordering::Release);
        }
    }
    pub fn changed(&self) -> Option<bool> {
        (!self.hooks.is_empty() && DIRTY.swap(false, Ordering::AcqRel))
            .then(|| LAYOUT.swap(false, Ordering::AcqRel))
    }
    pub fn target(&self, handle: u64, focus: usize) {
        TARGET.store(handle as usize, Ordering::Release);
        FOCUS.store(focus, Ordering::Release);
    }
    pub fn hide(&self) {
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
    pub fn show(&mut self, bounds: PasswordBounds, appearance: Theme) {
        unsafe {
            self.style.colors = theme::Colors::new(appearance);
            if self.hwnd.is_invalid() {
                let class = w!("QuickPanelAccountHint");
                let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: instance.unwrap_or_default().into(),
                    lpszClassName: class,
                    hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
                    ..Default::default()
                });
                self.hwnd = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                    class,
                    w!("点击填入账号"),
                    WS_POPUP | WS_BORDER,
                    0,
                    0,
                    150,
                    30,
                    None,
                    None,
                    instance.map(Into::into),
                    Some((&*self.style as *const Style).cast()),
                )
                .unwrap_or_default();
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(
                MonitorFromPoint(
                    POINT {
                        x: bounds.left,
                        y: bounds.bottom,
                    },
                    MONITOR_DEFAULTTONEAREST,
                ),
                &mut info,
            );
            let monitor = MonitorFromPoint(
                POINT {
                    x: bounds.left,
                    y: bounds.bottom,
                },
                MONITOR_DEFAULTTONEAREST,
            );
            let (mut dpi, mut dpi_y) = (96, 96);
            let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(
                monitor,
                windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
                &mut dpi,
                &mut dpi_y,
            );
            let dpi = dpi.max(96);
            let _ = DeleteObject(self.style.font.into());
            self.style.font = theme::font(dpi);
            let width = 150 * dpi as i32 / 96;
            let height = 32 * dpi as i32 / 96;
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                bounds.left.clamp(
                    info.rcWork.left,
                    (info.rcWork.right - width).max(info.rcWork.left),
                ),
                bounds.bottom.clamp(
                    info.rcWork.top,
                    (info.rcWork.bottom - height).max(info.rcWork.top),
                ),
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }
    pub fn stop(&mut self) {
        self.hide();
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
        DIRTY.store(false, Ordering::Release);
        LAYOUT.store(false, Ordering::Release);
        TARGET.store(0, Ordering::Release);
        FOCUS.store(0, Ordering::Release);
        *GUARD.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}
impl Drop for Hint {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
