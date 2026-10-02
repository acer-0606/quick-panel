use crate::{
    clipboard_formats::{
        MAX_ITEM_BYTES, SUPPORTED_FORMATS, decode_hdrop, decode_unicode_text, encode_unicode_text,
        validate_representations,
    },
    model::{ClipboardSnapshot, Representation, StoredItem},
};
use std::{
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND, SetLastError, WIN32_ERROR,
        },
        System::{
            DataExchange::*,
            Memory::*,
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        UI::{
            Input::KeyboardAndMouse::*,
            Shell::{DragQueryFileW, HDROP},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, PWSTR},
};

// These standard formats are HGLOBAL-backed. Bitmap/metafile/private pointer formats are excluded.
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const CF_HDROP: u32 = 15;

struct ClipboardGuard;
impl ClipboardGuard {
    fn open(owner: HWND) -> Result<Self, String> {
        let mut error = String::new();
        for attempt in 0..5 {
            match unsafe { OpenClipboard(Some(owner)) } {
                Ok(()) => return Ok(Self),
                Err(err) => error = err.to_string(),
            }
            if attempt < 4 {
                thread::sleep(Duration::from_millis(10 << attempt));
            }
        }
        Err(format!("剪贴板暂时被占用：{error}"))
    }
}
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

struct LockedGlobal(HGLOBAL, *mut core::ffi::c_void);
impl LockedGlobal {
    fn new(handle: HGLOBAL) -> Result<Self, String> {
        let pointer = unsafe { GlobalLock(handle) };
        if pointer.is_null() {
            return Err(format!(
                "无法锁定剪贴板内存：{}",
                windows::core::Error::from_thread()
            ));
        }
        Ok(Self(handle, pointer))
    }
}
impl Drop for LockedGlobal {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(self.0);
        }
    }
}

struct OwnedGlobal(Option<HGLOBAL>);
impl OwnedGlobal {
    fn copy_from(bytes: &[u8]) -> Result<Self, String> {
        let global = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) }
            .map_err(|err| format!("无法分配剪贴板内存：{err}"))?;
        let owned = Self(Some(global));
        {
            let lock = LockedGlobal::new(global)?;
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), lock.1.cast(), bytes.len());
            }
        }
        Ok(owned)
    }
    fn transfer(&mut self, format: u32) -> Result<(), String> {
        let global = self.0.ok_or("剪贴板内存所有权已转移")?;
        unsafe { SetClipboardData(format, Some(HANDLE(global.0))) }
            .map_err(|err| format!("恢复剪贴板格式失败：{err}"))?;
        // Only successful SetClipboardData transfers ownership to Windows.
        self.0 = None;
        Ok(())
    }
}
impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        if let Some(global) = self.0.take() {
            unsafe {
                let _ = GlobalFree(Some(global));
            }
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
fn registered_format(name: &str) -> Result<u32, String> {
    let name = wide(name);
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) };
    if id == 0 {
        Err(format!(
            "注册剪贴板格式失败：{}",
            windows::core::Error::from_thread()
        ))
    } else {
        Ok(id)
    }
}
fn format_id(name: &str) -> Result<u32, String> {
    match name {
        "CF_UNICODETEXT" => Ok(CF_UNICODETEXT),
        "CF_DIB" => Ok(CF_DIB),
        "CF_DIBV5" => Ok(CF_DIBV5),
        "CF_HDROP" => Ok(CF_HDROP),
        "PNG" | "HTML Format" | "Rich Text Format" => registered_format(name),
        _ => Err("不支持恢复该剪贴板格式".into()),
    }
}

fn sensitive_content() -> Result<bool, String> {
    // Microsoft's history exclusion marker plus widely used password-manager viewer marker.
    for name in [
        "ExcludeClipboardContentFromMonitorProcessing",
        "Clipboard Viewer Ignore",
    ] {
        if unsafe { IsClipboardFormatAvailable(registered_format(name)?) }.is_ok() {
            return Ok(true);
        }
    }
    // This official DWORD controls inclusion in local clipboard history; malformed markers fail closed.
    let format = registered_format("CanIncludeInClipboardHistory")?;
    if unsafe { IsClipboardFormatAvailable(format) }.is_ok() {
        let handle = unsafe { GetClipboardData(format) }.map_err(|err| err.to_string())?;
        let global = HGLOBAL(handle.0);
        if unsafe { GlobalSize(global) } < 4 {
            return Ok(true);
        }
        let lock = LockedGlobal::new(global)?;
        let value = unsafe { std::ptr::read_unaligned(lock.1.cast::<u32>()) };
        if value == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn source_app() -> String {
    let Ok(owner) = (unsafe { GetClipboardOwner() }) else {
        return String::new();
    };
    let mut process_id = 0;
    unsafe {
        GetWindowThreadProcessId(owner, Some(&mut process_id));
    }
    if process_id == 0 {
        return String::new();
    }
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
    else {
        return String::new();
    };
    struct ProcessGuard(HANDLE);
    impl Drop for ProcessGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    let process = ProcessGuard(handle);
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    if unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .is_err()
    {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
        .rsplit('\\')
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn ansi_hdrop_text(handle: HANDLE, byte_limit: usize) -> Result<String, String> {
    let drop = HDROP(handle.0);
    let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
    if count as usize > byte_limit / 2 {
        return Err("文件列表超过大小上限".into());
    }
    let mut result = String::new();
    for index in 0..count {
        let length = unsafe { DragQueryFileW(drop, index, None) } as usize;
        if length == 0 || length.saturating_add(1).saturating_mul(2) > byte_limit {
            return Err("文件路径长度无效或超过大小上限".into());
        }
        let mut buffer = vec![0u16; length + 1];
        let written = unsafe { DragQueryFileW(drop, index, Some(&mut buffer)) } as usize;
        if written != length {
            return Err("读取文件路径失败".into());
        }
        let path = String::from_utf16(&buffer[..written]).map_err(|_| "文件路径含无效UTF-16")?;
        if result.len().saturating_add(path.len()).saturating_add(1) > byte_limit {
            return Err("文件列表超过大小上限".into());
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&path);
    }
    Ok(result)
}

pub fn capture(owner: HWND, max_bytes: usize) -> Result<Option<ClipboardSnapshot>, String> {
    let limit = max_bytes.min(MAX_ITEM_BYTES);
    let _clipboard = ClipboardGuard::open(owner)?;
    let starting_sequence = sequence_number();
    if sensitive_content()? {
        return Ok(None);
    }
    let source_app = source_app();
    let mut representations = Vec::new();
    let mut total = 0usize;
    let mut text = String::new();
    // Prefer descriptive formats on restore; keep all supported representations byte-for-byte.
    for name in [
        "HTML Format",
        "Rich Text Format",
        "PNG",
        "CF_DIBV5",
        "CF_DIB",
        "CF_HDROP",
        "CF_UNICODETEXT",
    ] {
        debug_assert!(SUPPORTED_FORMATS.contains(&name));
        let format = format_id(name)?;
        if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
            continue;
        }
        let handle = unsafe { GetClipboardData(format) }
            .map_err(|err| format!("读取剪贴板格式{name}失败：{err}"))?;
        let global = HGLOBAL(handle.0);
        let length = unsafe { GlobalSize(global) };
        if length == 0 {
            return Err(format!("剪贴板格式{name}不是可复制的全局内存"));
        }
        total = total.checked_add(length).ok_or("剪贴板数据大小溢出")?;
        if total > limit {
            return Err("剪贴板内容超过8 MiB或配置上限，未记录".into());
        }
        let mut data = Vec::new();
        data.try_reserve_exact(length)
            .map_err(|_| "无法为剪贴板数据分配内存")?;
        {
            let lock = LockedGlobal::new(global)?;
            let bytes = unsafe { std::slice::from_raw_parts(lock.1.cast::<u8>(), length) };
            data.extend_from_slice(bytes);
        }
        let representation = Representation {
            format: name.to_owned(),
            data,
        };
        // Validate DROPFILES bounds/terminators before passing the foreign HGLOBAL to the Shell decoder.
        if name == "CF_HDROP" {
            validate_representations(std::slice::from_ref(&representation), limit)?;
        }
        let data = &representation.data;
        if name == "CF_UNICODETEXT" {
            text = decode_unicode_text(data)?;
        } else if name == "CF_HDROP" {
            text = if u32::from_le_bytes(
                data.get(16..20)
                    .ok_or("DROPFILES头不完整")?
                    .try_into()
                    .unwrap(),
            ) == 0
            {
                ansi_hdrop_text(handle, limit.saturating_sub(total))?
            } else {
                decode_hdrop(data)?.join("\n")
            };
        }
        representations.push(representation);
    }
    if representations.is_empty() {
        return Ok(None);
    }
    validate_representations(&representations, limit)?;
    if total
        .checked_add(text.len())
        .is_none_or(|bytes| bytes > limit)
    {
        return Err("剪贴板内容及检索文本超过8 MiB或配置上限，未记录".into());
    }
    if sequence_number() != starting_sequence {
        return Err("读取期间剪贴板内容发生变化，未记录本次数据".into());
    }
    let captured_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    Ok(Some(ClipboardSnapshot {
        text,
        representations,
        source_app,
        captured_at_ms,
    }))
}

pub fn restore(owner: HWND, item: &StoredItem, plain_text: bool) -> Result<(), String> {
    if !unsafe { IsWindow(Some(owner)) }.as_bool() {
        return Err("剪贴板所属窗口已失效".into());
    }
    let item_bytes = item
        .representations
        .iter()
        .try_fold(item.text.len(), |total, representation| {
            total.checked_add(representation.data.len())
        });
    if item_bytes.is_none_or(|bytes| bytes > MAX_ITEM_BYTES) {
        return Err("条目内容超过剪贴板大小上限".into());
    }
    let fallback;
    let representations = if plain_text || item.representations.is_empty() {
        if item.text.is_empty() {
            return Err("此条目没有可粘贴的纯文本".into());
        }
        if item
            .text
            .encode_utf16()
            .count()
            .saturating_add(1)
            .saturating_mul(2)
            > MAX_ITEM_BYTES
        {
            return Err("文本超过剪贴板大小上限".into());
        }
        fallback = vec![Representation {
            format: "CF_UNICODETEXT".into(),
            data: encode_unicode_text(&item.text),
        }];
        &fallback
    } else {
        &item.representations
    };
    validate_representations(representations, MAX_ITEM_BYTES)?;
    // Allocate before EmptyClipboard so allocation errors leave the existing clipboard intact.
    let mut allocations = Vec::with_capacity(representations.len());
    for representation in representations {
        allocations.push((
            format_id(&representation.format)?,
            OwnedGlobal::copy_from(&representation.data)?,
        ));
    }
    let _clipboard = ClipboardGuard::open(owner)?;
    unsafe { EmptyClipboard() }.map_err(|err| format!("清空剪贴板失败：{err}"))?;
    for (format, allocation) in &mut allocations {
        allocation.transfer(*format)?;
    }
    Ok(())
}

pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

fn key_input(key: VIRTUAL_KEY, release: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: if release {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }
}

pub fn paste_to(target: HWND) -> Result<(), String> {
    if !unsafe { IsWindow(Some(target)) }.as_bool() {
        return Err("先前窗口已关闭，内容已复制，可手动粘贴".into());
    }
    for key in [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN] {
        if unsafe { GetAsyncKeyState(key.0 as i32) } < 0 {
            return Err("请松开修饰键后粘贴，内容已复制".into());
        }
    }
    if unsafe { GetForegroundWindow() } != target
        && !unsafe { SetForegroundWindow(target) }.as_bool()
    {
        return Err("系统未允许恢复先前窗口焦点，内容已复制，可手动粘贴".into());
    }
    if !unsafe { IsWindow(Some(target)) }.as_bool() || unsafe { GetForegroundWindow() } != target {
        return Err("目标窗口未获得焦点，内容已复制，可手动粘贴".into());
    }
    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VK_V, false),
        key_input(VK_V, true),
        key_input(VK_CONTROL, true),
    ];
    unsafe {
        SetLastError(WIN32_ERROR(0));
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        let error = unsafe { GetLastError() }.0;
        if sent > 0 {
            // Release only keys this call pressed; physical modifiers were checked above.
            let releases = [key_input(VK_V, true), key_input(VK_CONTROL, true)];
            unsafe {
                SendInput(&releases, std::mem::size_of::<INPUT>() as i32);
            }
        }
        return Err(format!(
            "自动粘贴未完成（{sent}/4，错误{error}）。Windows可能因UIPI权限级别阻止输入；内容已复制，可手动粘贴"
        ));
    }
    Ok(())
}
