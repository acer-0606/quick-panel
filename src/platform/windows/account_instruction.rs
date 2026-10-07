//! Read-only teaching instructions. No click authorization or observation state.
use super::PickRole;
use crate::{account_fill_execution::FillError, model::Theme, platform::windows::theme};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
struct Style {
    colors: theme::Colors,
    font: HFONT,
    label: Vec<u16>,
}
impl Drop for Style {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.font.into());
        }
    }
}
pub(super) struct Instruction {
    hwnd: HWND,
    style: Box<Style>,
    dpi: u32,
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
            WM_NCHITTEST => return LRESULT(HTTRANSPARENT as isize),
            WM_PAINT => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Style;
                if !ptr.is_null() {
                    let style = &mut *ptr;
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(dc, &rect, style.colors.surface);
                    SetBkMode(dc, TRANSPARENT);
                    SetTextColor(dc, style.colors.palette.text);
                    let old = SelectObject(dc, style.font.into());
                    DrawTextW(
                        dc,
                        &mut style.label,
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
impl Instruction {
    #[cfg(test)]
    pub(super) fn hwnd_for_test(&self) -> HWND {
        self.hwnd
    }
    pub(super) fn show(role: PickRole, appearance: Theme) -> Result<Self, FillError> {
        let label = match role {
            PickRole::Password => "请选择密码框 · Esc / 右键取消",
            PickRole::Username => "请选择用户名框 · Esc / 右键取消",
            PickRole::Target => "请选择区分目标 · Esc / 右键取消",
        };
        let title: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
        let mut instruction = Self {
            hwnd: HWND::default(),
            dpi: 96,
            style: Box::new(Style {
                colors: theme::Colors::new(appearance),
                font: unsafe { theme::font(96) },
                label: label.encode_utf16().collect(),
            }),
        };
        unsafe {
            let class = w!("QuickPanelAccountInstruction");
            let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.unwrap_or_default().into(),
                lpszClassName: class,
                ..Default::default()
            });
            instruction.hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT,
                class,
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_BORDER | WS_DISABLED,
                0,
                0,
                1,
                1,
                None,
                None,
                instance.map(Into::into),
                Some((&mut *instruction.style as *mut Style).cast()),
            )
            .map_err(|_| FillError::QueryFailed)?;
        }
        instruction.position();
        Ok(instruction)
    }
    pub(super) fn position(&mut self) {
        unsafe {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            let (mut dpi, mut dy) = (96, 96);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
            let dpi = dpi.max(96);
            if self.dpi != dpi {
                let _ = DeleteObject(self.style.font.into());
                self.style.font = theme::font(dpi);
                self.dpi = dpi;
            }
            let pad = 16 * dpi as i32 / 96;
            let width = (340 * dpi as i32 / 96).min(info.rcWork.right - info.rcWork.left);
            let height = 40 * dpi as i32 / 96;
            // Follow the cursor's monitor and use the opposite edge: never sit over the point being selected.
            let y = if point.y < (info.rcWork.top + info.rcWork.bottom) / 2 {
                info.rcWork.bottom - height - pad
            } else {
                info.rcWork.top + pad
            };
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                info.rcWork.left + (info.rcWork.right - info.rcWork.left - width) / 2,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}
impl Drop for Instruction {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}
