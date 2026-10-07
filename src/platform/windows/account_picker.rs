//! Temporary GUI-thread native hooks capture only point/cancel notifications.
//! No UIA, screenshots or keyboard recording. Read-only instructions share this lifetime.
#[path = "account_instruction.rs"]
mod instruction;
use super::account_target::{capture_window, foreground_matches, hwnd, window_identity};
use crate::{account_fill::WindowIdentity, account_fill_execution::FillError};
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::{Arc, Mutex},
};
use windows::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickRole {
    Password,
    Username,
    Target,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickPoint {
    pub window: WindowIdentity,
    pub x: i32,
    pub y: i32,
    pub role: PickRole,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickEvent {
    Waiting,
    Cancelled,
    Selected(PickPoint),
}
#[derive(Clone, Copy, Default)]
enum Notice {
    #[default]
    Waiting,
    Cancel,
    Point(i32, i32),
}
#[derive(Default)]
struct Notices {
    notice: Notice,
    pending_point: Option<(i32, i32)>,
    right: bool,
    escape: bool,
}
struct Signal(Mutex<Notices>);
static ACTIVE: Mutex<Option<Arc<Signal>>> = Mutex::new(None);
pub struct Picker {
    owner: u64,
    target: Option<WindowIdentity>,
    role: PickRole,
    own_pid: u32,
    done: bool,
    signal: Arc<Signal>,
    instruction: Option<instruction::Instruction>,
    mouse: Option<HHOOK>,
    keyboard: Option<HHOOK>,
    _gui_thread: PhantomData<Rc<()>>,
}
/// Hide the account window and restore a known target first. Call on the GUI
/// message-pump thread, then poll from its timer. Unknown target is bound on click.
pub fn pick(
    owner: u64,
    target: Option<WindowIdentity>,
    role: PickRole,
    appearance: crate::model::Theme,
) -> Result<Picker, FillError> {
    unsafe {
        let own = window_identity(owner)?;
        if let Some(target) = target
            && !foreground_matches(target)
        {
            return Err(FillError::TargetChanged);
        }
        let signal = Arc::new(Signal(Mutex::new(Notices::default())));
        {
            let mut active = ACTIVE.lock().map_err(|_| FillError::Cancelled)?;
            if active.is_some() {
                return Err(FillError::Busy);
            }
            *active = Some(signal.clone());
        }
        let mut picker = Picker {
            owner,
            target,
            role,
            own_pid: own.pid,
            done: false,
            signal,
            instruction: None,
            mouse: None,
            keyboard: None,
            _gui_thread: PhantomData,
        };
        let module = GetModuleHandleW(None).map_err(|_| FillError::QueryFailed)?;
        picker.mouse = Some(
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(HINSTANCE(module.0)), 0)
                .map_err(|_| FillError::QueryFailed)?,
        );
        picker.keyboard = Some(
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                Some(HINSTANCE(module.0)),
                0,
            )
            .map_err(|_| FillError::QueryFailed)?,
        );
        picker.instruction = Some(instruction::Instruction::show(role, appearance)?);
        Ok(picker)
    }
}
fn notify(update: impl FnOnce(&mut Notices)) {
    if let Ok(active) = ACTIVE.lock()
        && let Some(signal) = active.as_ref()
        && let Ok(mut current) = signal.0.lock()
    {
        update(&mut current);
    }
}
unsafe extern "system" fn mouse_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            match message.0 as u32 {
                WM_LBUTTONDOWN => {
                    let event = &*(data.0 as *const MSLLHOOKSTRUCT);
                    notify(|n| {
                        if matches!(n.notice, Notice::Waiting) && n.pending_point.is_none() {
                            n.pending_point = Some((event.pt.x, event.pt.y));
                        }
                    });
                    return LRESULT(1);
                }
                WM_LBUTTONUP => {
                    notify(|n| {
                        if let Some((x, y)) = n.pending_point.take()
                            && matches!(n.notice, Notice::Waiting)
                        {
                            n.notice = Notice::Point(x, y);
                        }
                    });
                    return LRESULT(1);
                }
                WM_RBUTTONDOWN => {
                    notify(|n| {
                        n.right = true;
                        n.notice = Notice::Cancel;
                    });
                    return LRESULT(1);
                }
                WM_RBUTTONUP => {
                    notify(|n| {
                        n.right = false;
                        n.pending_point = None;
                    });
                    return LRESULT(1);
                }
                _ => {}
            }
        }
        CallNextHookEx(None, code, message, data)
    }
}
unsafe extern "system" fn keyboard_hook(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && (*(data.0 as *const KBDLLHOOKSTRUCT)).vkCode == VK_ESCAPE.0 as u32 {
            match message.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    notify(|n| {
                        n.escape = true;
                        n.notice = Notice::Cancel;
                    });
                    return LRESULT(1);
                }
                WM_KEYUP | WM_SYSKEYUP => {
                    notify(|n| {
                        n.escape = false;
                        n.pending_point = None;
                    });
                    return LRESULT(1);
                }
                _ => {}
            }
        }
        CallNextHookEx(None, code, message, data)
    }
}
impl Picker {
    fn release(&mut self) {
        self.instruction = None;
        unsafe {
            if let Some(hook) = self.mouse.take() {
                let _ = UnhookWindowsHookEx(hook);
            }
            if let Some(hook) = self.keyboard.take() {
                let _ = UnhookWindowsHookEx(hook);
            }
            if let Ok(mut active) = ACTIVE.lock()
                && active
                    .as_ref()
                    .is_some_and(|s| Arc::ptr_eq(s, &self.signal))
            {
                *active = None;
            }
        }
    }
    pub fn cancel(&mut self) -> PickEvent {
        self.done = true;
        self.release();
        PickEvent::Cancelled
    }
    pub fn poll(&mut self) -> PickEvent {
        unsafe {
            if self.done || !IsWindow(Some(hwnd(self.owner))).as_bool() {
                return self.cancel();
            }
            if self
                .target
                .is_some_and(|target| !foreground_matches(target))
            {
                return self.cancel();
            }
            if let Some(instruction) = &mut self.instruction {
                instruction.position();
            }
            let notice = self
                .signal
                .0
                .lock()
                .map(|mut n| {
                    if n.right || n.escape || n.pending_point.is_some() {
                        Notice::Waiting
                    } else {
                        std::mem::take(&mut n.notice)
                    }
                })
                .unwrap_or(Notice::Cancel);
            match notice {
                Notice::Waiting => PickEvent::Waiting,
                Notice::Cancel => self.cancel(),
                Notice::Point(x, y) => {
                    let selected = GetAncestor(WindowFromPhysicalPoint(POINT { x, y }), GA_ROOT);
                    let Ok(window) = capture_window(selected.0 as usize as u64, self.own_pid)
                    else {
                        return self.cancel();
                    };
                    if self.target.is_some_and(|target| target != window)
                        || GetAncestor(WindowFromPhysicalPoint(POINT { x, y }), GA_ROOT)
                            != hwnd(window.handle)
                    {
                        return self.cancel();
                    }
                    self.done = true;
                    self.release();
                    PickEvent::Selected(PickPoint {
                        window,
                        x,
                        y,
                        role: self.role,
                    })
                }
            }
        }
    }
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.release()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;
    #[test]
    fn production_hooks_pair_release_reenter_and_cleanup_instruction() {
        unsafe {
            let target = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("synthetic-picker"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                80,
                80,
                400,
                200,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            assert!(SetForegroundWindow(target).as_bool());
            let identity = window_identity(target.0 as u64).unwrap();
            let begin = |role| {
                let mut picker = pick(
                    target.0 as u64,
                    Some(identity),
                    role,
                    crate::model::Theme::System,
                )
                .unwrap();
                // Synthetic fixture is in-process; production forbids selecting own app.
                // Only the fixture's excluded PID differs; callbacks/poll/window validation remain real.
                picker.own_pid = 0;
                picker
            };
            let mouse = MSLLHOOKSTRUCT {
                pt: POINT { x: 150, y: 150 },
                ..Default::default()
            };
            let key = KBDLLHOOKSTRUCT {
                vkCode: VK_ESCAPE.0 as u32,
                ..Default::default()
            };
            let send_mouse = |m| {
                mouse_hook(
                    0,
                    WPARAM(m as usize),
                    LPARAM((&mouse as *const MSLLHOOKSTRUCT) as isize),
                )
            };
            for cancel in [0, 1, 2, 0] {
                let role = match cancel {
                    1 => PickRole::Username,
                    2 => PickRole::Target,
                    _ => PickRole::Password,
                };
                let mut picker = begin(role);
                let instruction = picker.instruction.as_ref().unwrap().hwnd_for_test();
                assert!(IsWindowVisible(instruction).as_bool());
                let mut label = [0u16; 80];
                let length = GetWindowTextW(instruction, &mut label);
                let label = String::from_utf16_lossy(&label[..length as usize]);
                assert!(label.contains(match role {
                    PickRole::Password => "密码框",
                    PickRole::Username => "用户名框",
                    PickRole::Target => "区分目标",
                }));
                assert!(label.contains("Esc / 右键取消"));
                assert_eq!(GetForegroundWindow(), target);
                assert_ne!(
                    GetWindowLongW(instruction, GWL_EXSTYLE) as u32 & WS_EX_NOACTIVATE.0,
                    0
                );
                assert_eq!(
                    SendMessageW(instruction, WM_MOUSEACTIVATE, None, None),
                    LRESULT(MA_NOACTIVATE as isize)
                );
                match cancel {
                    0 => {
                        assert_eq!(send_mouse(WM_LBUTTONDOWN), LRESULT(1));
                        assert_eq!(picker.poll(), PickEvent::Waiting);
                        assert_eq!(send_mouse(WM_LBUTTONUP), LRESULT(1));
                    }
                    1 => {
                        assert_eq!(send_mouse(WM_RBUTTONDOWN), LRESULT(1));
                        assert_eq!(picker.poll(), PickEvent::Waiting);
                        assert_eq!(send_mouse(WM_RBUTTONUP), LRESULT(1));
                    }
                    _ => {
                        assert_eq!(
                            keyboard_hook(
                                0,
                                WPARAM(WM_KEYDOWN as usize),
                                LPARAM((&key as *const KBDLLHOOKSTRUCT) as isize)
                            ),
                            LRESULT(1)
                        );
                        assert_eq!(picker.poll(), PickEvent::Waiting);
                        assert_eq!(
                            keyboard_hook(
                                0,
                                WPARAM(WM_KEYUP as usize),
                                LPARAM((&key as *const KBDLLHOOKSTRUCT) as isize)
                            ),
                            LRESULT(1)
                        );
                    }
                }
                if cancel == 0 {
                    assert!(
                        matches!(picker.poll(),PickEvent::Selected(PickPoint {window,..}) if window==identity)
                    );
                } else {
                    assert_eq!(picker.poll(), PickEvent::Cancelled);
                }
                assert_eq!(picker.poll(), PickEvent::Cancelled);
                assert!(ACTIVE.lock().unwrap().is_none());
                assert!(picker.mouse.is_none() && picker.keyboard.is_none());
                assert!(!IsWindow(Some(instruction)).as_bool());
            }
            let picker = begin(PickRole::Password);
            let instruction = picker.instruction.as_ref().unwrap().hwnd_for_test();
            drop(picker);
            assert!(ACTIVE.lock().unwrap().is_none());
            assert!(!IsWindow(Some(instruction)).as_bool());
            let mut picker = begin(PickRole::Password);
            let instruction = picker.instruction.as_ref().unwrap().hwnd_for_test();
            let _ = DestroyWindow(target);
            assert_eq!(picker.poll(), PickEvent::Cancelled);
            assert!(ACTIVE.lock().unwrap().is_none());
            assert!(!IsWindow(Some(instruction)).as_bool());
        }
    }
}
