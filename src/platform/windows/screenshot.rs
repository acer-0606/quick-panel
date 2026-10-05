//! User-invoked physical-coordinate selection; only the confirmed rectangle is captured.
#![allow(unsafe_op_in_unsafe_fn)]
use crate::screenshot::{Rect, ScreenshotTicket, pixel_bytes, selection};
use std::cell::{Cell, RefCell};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::DwmFlush, Gdi::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::w,
};

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}
struct Overlay {
    hwnd: Cell<HWND>,
    desktop: Rect,
    start: Cell<Option<(i32, i32)>>,
    end: Cell<(i32, i32)>,
    done: Cell<bool>,
    result: RefCell<Option<Result<Option<Rect>, String>>>,
}
impl Overlay {
    unsafe fn finish(&self, result: Result<Option<Rect>, String>) {
        if self.done.replace(true) {
            return;
        }
        // Persist the terminal result before ReleaseCapture reenters WM_CAPTURECHANGED.
        *self.result.borrow_mut() = Some(result);
        if GetCapture() == self.hwnd.get() {
            let _ = ReleaseCapture();
        }
    }
}
struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.0)).as_bool() {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
struct DpiContext(DPI_AWARENESS_CONTEXT);
impl Drop for DpiContext {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
unsafe fn desktop() -> Result<Rect, String> {
    let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    if width <= 0 || height <= 0 {
        return Err("无法取得桌面范围".into());
    }
    Ok(Rect {
        left,
        top,
        right: left.checked_add(width).ok_or("桌面坐标溢出")?,
        bottom: top.checked_add(height).ok_or("桌面坐标溢出")?,
    })
}
unsafe fn cursor() -> Result<(i32, i32), String> {
    let mut point = POINT::default();
    GetCursorPos(&mut point).map_err(|e| e.to_string())?;
    Ok((point.x, point.y))
}

/// Called without a borrowed App. Its nested pump permits shutdown/cancellation.
pub fn select_and_capture(
    ticket: &ScreenshotTicket,
    target: HWND,
) -> Result<Option<Pixels>, String> {
    unsafe {
        if !ticket.current() {
            return Ok(None);
        }
        let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if previous.0.is_null() {
            return Err("无法设置截图 DPI 上下文".into());
        }
        let _dpi = DpiContext(previous);
        let bounds = desktop()?;
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = w!("QuickPanelScreenshotSelection");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_CROSS).map_err(|e| e.to_string())?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let state = Box::new(Overlay {
            hwnd: Cell::new(HWND::default()),
            desktop: bounds,
            start: Cell::new(None),
            end: Cell::new((bounds.left, bounds.top)),
            done: Cell::new(false),
            result: RefCell::new(None),
        });
        let (width, height) = bounds.dimensions()?;
        let window = Window(
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                class,
                w!("拖动选择截图区域 · Esc / 右键取消"),
                WS_POPUP,
                bounds.left,
                bounds.top,
                i32::try_from(width).map_err(|_| "桌面宽度溢出")?,
                i32::try_from(height).map_err(|_| "桌面高度溢出")?,
                None,
                None,
                Some(instance.into()),
                Some((&*state as *const Overlay).cast()),
            )
            .map_err(|e| e.to_string())?,
        );
        SetLayeredWindowAttributes(window.0, COLORREF(0), 90, LWA_ALPHA)
            .map_err(|e| e.to_string())?;
        let _ = ShowWindow(window.0, SW_SHOW);
        if GetForegroundWindow() != window.0 && !SetForegroundWindow(window.0).as_bool() {
            return Err("系统未允许截图选区获得焦点，请重试".into());
        }
        let _ = SetFocus(Some(window.0));
        // Idle selection has no screen capture thread and no screenshot bitmap cache.
        while !state.done.get() && ticket.current() {
            let mut message = MSG::default();
            let result = GetMessageW(&mut message, None, 0, 0);
            if result.0 == -1 {
                return Err(windows::core::Error::from_thread().to_string());
            }
            if result.0 == 0 {
                PostQuitMessage(message.wParam.0 as i32);
                state.finish(Ok(None));
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if !state.done.get() {
            state.finish(Ok(None));
        }
        let restore_target = GetForegroundWindow() == window.0;
        let _ = ShowWindow(window.0, SW_HIDE);
        drop(window);
        // Do not reopen the panel; return keyboard focus to the previously selected app.
        if restore_target && IsWindow(Some(target)).as_bool() {
            let _ = SetForegroundWindow(target);
        }
        let selected = state.result.borrow_mut().take().unwrap_or(Ok(None))?;
        if !ticket.current() {
            return Ok(None);
        }
        if desktop()? != bounds {
            return Ok(None);
        }
        let Some(rect) = selected else {
            return Ok(None);
        };
        DwmFlush().map_err(|e| format!("无法等待选区层隐藏：{e}"))?;
        capture_pixels(rect).map(Some)
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Overlay;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let state = &*ptr;
    if msg == WM_NCCREATE {
        state.hwnd.set(hwnd);
    }
    match msg {
        WM_LBUTTONDOWN if !state.done.get() => match cursor() {
            Ok(point) => {
                state.start.set(Some(point));
                state.end.set(point);
                SetCapture(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            Err(error) => state.finish(Err(error)),
        },
        WM_MOUSEMOVE if state.start.get().is_some() && !state.done.get() => match cursor() {
            Ok(point) => {
                state.end.set(point);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            Err(error) => state.finish(Err(error)),
        },
        WM_LBUTTONUP if !state.done.get() => {
            if let Some(start) = state.start.get() {
                state.finish(cursor().and_then(|end| selection(start, end, state.desktop)));
            }
        }
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => state.finish(Ok(None)),
        WM_RBUTTONDOWN | WM_DISPLAYCHANGE | WM_CANCELMODE | WM_CLOSE => state.finish(Ok(None)),
        WM_CAPTURECHANGED if state.start.get().is_some() && !state.done.get() => {
            state.finish(Ok(None))
        }
        WM_ACTIVATE if wp.0 & 0xffff == WA_INACTIVE as usize && !state.done.get() => {
            state.finish(Ok(None))
        }
        WM_DPICHANGED => {
            // The layer always spans the original physical virtual desktop, even across mixed DPI.
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            FillRect(dc, &client, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            if let Some(start) = state.start.get() {
                let end = state.end.get();
                let rectangle = RECT {
                    left: start.0.min(end.0).saturating_sub(state.desktop.left),
                    top: start.1.min(end.1).saturating_sub(state.desktop.top),
                    right: start.0.max(end.0).saturating_sub(state.desktop.left),
                    bottom: start.1.max(end.1).saturating_sub(state.desktop.top),
                };
                FrameRect(dc, &rectangle, HBRUSH(GetStockObject(WHITE_BRUSH).0));
            }
            let _ = EndPaint(hwnd, &paint);
        }
        WM_NCDESTROY => {
            state.finish(Ok(None));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

struct ScreenDc(HDC);
impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            ReleaseDC(None, self.0);
        }
    }
}
struct MemoryDc(HDC);
impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}
struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}
struct Selected(HDC, HGDIOBJ);
impl Drop for Selected {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.0, self.1);
        }
    }
}

unsafe fn capture_pixels(rect: Rect) -> Result<Pixels, String> {
    let (width, height) = rect.dimensions()?;
    let bytes = pixel_bytes(width, height)?;
    let screen = ScreenDc(GetDC(None));
    if screen.0.is_invalid() {
        return Err("无法取得屏幕设备上下文".into());
    }
    let memory = MemoryDc(CreateCompatibleDC(Some(screen.0)));
    if memory.0.is_invalid() {
        return Err("无法创建截图设备上下文".into());
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: bytes as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = Bitmap(
        CreateDIBSection(Some(screen.0), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .map_err(|e| format!("无法分配截图位图：{e}"))?,
    );
    if bits.is_null() {
        return Err("截图位图没有像素缓冲区".into());
    }
    let previous = SelectObject(memory.0, bitmap.0.into());
    if previous.is_invalid() || previous.0 as isize == -1 {
        return Err("无法选择截图位图".into());
    }
    let _selected = Selected(memory.0, previous);
    BitBlt(
        memory.0,
        0,
        0,
        width as i32,
        height as i32,
        Some(screen.0),
        rect.left,
        rect.top,
        SRCCOPY | CAPTUREBLT,
    )
    .map_err(|e| format!("无法读取选中屏幕区域：{e}"))?;
    if !GdiFlush().as_bool() {
        return Err("截图像素同步失败".into());
    }
    let mut bgra = Vec::new();
    bgra.try_reserve_exact(bytes)
        .map_err(|_| "无法为截图分配内存")?;
    bgra.extend_from_slice(std::slice::from_raw_parts(bits.cast::<u8>(), bytes));
    Ok(Pixels {
        width,
        height,
        bgra,
    })
}
