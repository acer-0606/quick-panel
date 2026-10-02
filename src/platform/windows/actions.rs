use crate::model::StoredItem;
use windows::{
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
        UI::{
            Shell::{
                Common::ITEMIDLIST, ILClone, ILFindLastID, ILRemoveLastID,
                SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW,
            },
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{PCWSTR, w},
};

fn wide(value: &str) -> Result<Vec<u16>, String> {
    if value.contains('\0') {
        return Err("路径中含有无效的终止字符".into());
    }
    Ok(value.encode_utf16().chain(std::iter::once(0)).collect())
}

fn target(item: &StoredItem) -> Result<&str, String> {
    let path = item
        .target
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or("条目没有目标路径")?;
    if path.contains('\0') {
        return Err("目标路径无效".into());
    }
    std::fs::metadata(path).map_err(|err| format!("无法访问目标路径：{err}"))?;
    Ok(path)
}

fn quote_argument(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        if character == '"' {
            result.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        result.push(character);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}

pub fn open_target(item: &StoredItem) -> Result<(), String> {
    let path = target(item)?;
    let application = item
        .open_with
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let file = wide(application.unwrap_or(path))?;
    let parameters = application
        .map(|_| wide(&quote_argument(path)))
        .transpose()?;
    // ShellExecute's documented result is an HINSTANCE-typed status: >32 means success.
    let status = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            parameters
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    }
    .0 as isize;
    if status > 32 {
        Ok(())
    } else {
        Err(format!("打开目标失败（Shell错误{status}）"))
    }
}

struct ComGuard(bool);
impl ComGuard {
    fn new() -> Result<Self, String> {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            return Ok(Self(true));
        }
        // The UI may already own a COM apartment; do not uninitialize its registration.
        if result == RPC_E_CHANGED_MODE {
            return Ok(Self(false));
        }
        Err(format!("初始化Shell接口失败：{result}"))
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}

pub fn reveal_target(item: &StoredItem) -> Result<(), String> {
    let path = wide(target(item)?)?;
    let _com = ComGuard::new()?;
    let mut pointer = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(path.as_ptr()), None, &mut pointer, 0, None) }
        .map_err(|err| format!("解析Shell目标失败：{err}"))?;
    if pointer.is_null() {
        return Err("Shell未返回有效目标".into());
    }
    let full = Pidl(pointer);
    let parent = Pidl(unsafe { ILClone(full.0) });
    if parent.0.is_null() {
        return Err("无法分配Shell路径内存".into());
    }
    let last = unsafe { ILFindLastID(full.0) };
    let result = if unsafe { ILRemoveLastID(Some(parent.0)) }.as_bool() {
        unsafe { SHOpenFolderAndSelectItems(parent.0, Some(&[last.cast_const()]), 0) }
    } else {
        // A namespace root has no parent: open that root directly.
        unsafe { SHOpenFolderAndSelectItems(full.0, None, 0) }
    };
    result.map_err(|err| format!("在资源管理器中定位失败：{err}"))
}
