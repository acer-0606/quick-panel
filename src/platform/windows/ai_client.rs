//! On-demand Claude-compatible HTTP and current-user Windows secret protection.
//!
//! Call `interpret` from the bounded AI worker, never from the UI or SQLite actor.
use crate::{
    ai::{AiPlan, AiSettings, FunctionDescriptor},
    ai_models::{self, ModelInfo, ModelListing},
    claude::{HttpRequest, parse_response, prepare},
    connection_test::{CredentialSource, credential_source},
    model::{Scope, Settings},
    organizer::{self, Proposal, Work},
};
use std::{
    ffi::c_void,
    ptr,
    sync::atomic::{Ordering, compiler_fence},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Networking::WinHttp::*,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    },
    core::{PCWSTR, w},
};

const MAX_KEY_BYTES: usize = 8192;
const MAX_PROTECTED_KEY_BYTES: usize = 16 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_HEADER_BYTES: u32 = 16 * 1024;
const REQUEST_BUDGET: Duration = Duration::from_secs(30);
const RESOLVE_TIMEOUT_MS: i32 = 4000;
const CONNECT_TIMEOUT_MS: i32 = 4000;
const IO_TIMEOUT_MS: i32 = 8000;
// Stable application separation, not an independently secret password.
const KEY_ENTROPY: &[u8] = b"quick-panel:claude-api-key:v1";
const CONNECTION_TEST_TEXT: &str = "Quick Panel connection test";

fn clear_bytes(bytes: &mut [u8]) {
    for byte in bytes {
        // Keep sensitive-data erasure observable to the compiler before deallocation.
        unsafe { ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

struct ProtectedOutput(CRYPT_INTEGER_BLOB);
impl ProtectedOutput {
    fn bytes(&self) -> &[u8] {
        if self.0.pbData.is_null() || self.0.cbData == 0 {
            &[]
        } else {
            // DPAPI owns an allocation of exactly cbData bytes until LocalFree.
            unsafe { std::slice::from_raw_parts(self.0.pbData, self.0.cbData as usize) }
        }
    }
}
impl Drop for ProtectedOutput {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            unsafe {
                clear_bytes(std::slice::from_raw_parts_mut(
                    self.0.pbData,
                    self.0.cbData as usize,
                ));
                let _ = LocalFree(Some(HLOCAL(self.0.pbData.cast())));
            }
        }
    }
}

fn input_blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        // CryptProtectData/CryptUnprotectData treat pDataIn as input only.
        pbData: bytes.as_ptr().cast_mut(),
    }
}

/// Protect a credential for the current Windows user; the returned bytes may be saved.
pub fn protect_key(key: &str) -> Result<Vec<u8>, String> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err("API Key 不能为空或超过 8192 字节".into());
    }
    let input = input_blob(key.as_bytes());
    let entropy = input_blob(KEY_ENTROPY);
    let mut output = ProtectedOutput(CRYPT_INTEGER_BLOB::default());
    unsafe {
        CryptProtectData(
            &input,
            w!("Quick Panel API credential"),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.0,
        )
    }
    .map_err(|_| "无法使用当前 Windows 用户保护 API Key".to_owned())?;
    let bytes = output.bytes();
    if bytes.is_empty() || bytes.len() > MAX_PROTECTED_KEY_BYTES {
        return Err("保护 API Key 时返回了无效数据".into());
    }
    Ok(bytes.to_vec())
}

/// Recover a credential only in the current user context; never print the result.
pub fn unprotect_key(encrypted: &[u8]) -> Result<String, String> {
    if encrypted.is_empty() || encrypted.len() > MAX_PROTECTED_KEY_BYTES {
        return Err("尚未配置 API Key，或受保护的密钥数据无效".into());
    }
    let input = input_blob(encrypted);
    let entropy = input_blob(KEY_ENTROPY);
    let mut output = ProtectedOutput(CRYPT_INTEGER_BLOB::default());
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output.0,
        )
    }
    .map_err(|_| "无法读取 API Key，请在当前 Windows 用户下重新配置".to_owned())?;
    let bytes = output.bytes();
    if bytes.is_empty() || bytes.len() > MAX_KEY_BYTES {
        return Err("受保护的 API Key 数据无效，请重新配置".into());
    }
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "受保护的 API Key 数据无效，请重新配置".into())
}

pub(crate) struct SecretString(pub(crate) String);
impl Drop for SecretString {
    fn drop(&mut self) {
        // No read follows this erasure; a zero byte is valid UTF-8 as well.
        unsafe { clear_bytes(self.0.as_mut_vec()) };
    }
}

struct SensitiveRequest(HttpRequest);
impl Drop for SensitiveRequest {
    fn drop(&mut self) {
        for (_, value) in &mut self.0.headers {
            unsafe { clear_bytes(value.as_mut_vec()) };
        }
        clear_bytes(&mut self.0.body);
    }
}

struct HeaderBuffer(Vec<u16>);
impl HeaderBuffer {
    fn new(name: &str, value: &str) -> Self {
        Self(
            name.encode_utf16()
                .chain(": ".encode_utf16())
                .chain(value.encode_utf16())
                .chain("\r\n".encode_utf16())
                .collect(),
        )
    }
}
impl Drop for HeaderBuffer {
    fn drop(&mut self) {
        for character in &mut self.0 {
            unsafe { ptr::write_volatile(character, 0) };
        }
        compiler_fence(Ordering::SeqCst);
    }
}

struct InternetHandle(*mut c_void);
impl InternetHandle {
    fn new(pointer: *mut c_void) -> Result<Self, String> {
        if pointer.is_null() {
            Err("无法初始化 AI 网络连接".into())
        } else {
            Ok(Self(pointer))
        }
    }

    fn set_u32(&self, option: u32, value: u32) -> Result<(), String> {
        unsafe {
            WinHttpSetOption(
                Some(self.0.cast_const()),
                option,
                Some(&value.to_ne_bytes()),
            )
        }
        .map_err(|_| "无法设置 AI 网络请求选项".into())
    }

    fn timeouts(&self, deadline: Instant) -> Result<(), String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("AI 请求超时，请稍后重试".into());
        }
        // 0 means infinite in WinHTTP, so even the final sub-millisecond interval is 1 ms.
        let cap = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        unsafe {
            WinHttpSetTimeouts(
                self.0,
                RESOLVE_TIMEOUT_MS.min(cap),
                CONNECT_TIMEOUT_MS.min(cap),
                IO_TIMEOUT_MS.min(cap),
                IO_TIMEOUT_MS.min(cap),
            )
        }
        .map_err(|_| "无法设置 AI 请求超时".into())
    }
}
impl Drop for InternetHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn query_number(handle: &InternetHandle, kind: u32) -> Result<u32, String> {
    let mut number = 0u32;
    let mut length = size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            handle.0,
            kind | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut number as *mut u32).cast()),
            &mut length,
            ptr::null_mut(),
        )
    }
    .map_err(|_| "AI 返回了无效的 HTTP 响应".to_owned())?;
    Ok(number)
}

fn exchange(
    request: &HttpRequest,
    response_limit: usize,
    oversize: &str,
) -> Result<(u16, Vec<u8>), String> {
    exchange_at(
        request,
        response_limit,
        oversize,
        HttpMethod::Post,
        Instant::now() + REQUEST_BUDGET,
    )
}

enum HttpMethod {
    Get,
    Post,
}

fn exchange_at(
    request: &HttpRequest,
    response_limit: usize,
    oversize: &str,
    method: HttpMethod,
    deadline: Instant,
) -> Result<(u16, Vec<u8>), String> {
    let endpoint = &request.endpoint;
    // A loopback endpoint must stay local, even if the current user configured a proxy.
    let access = if endpoint.host.eq_ignore_ascii_case("localhost")
        || endpoint
            .host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        WINHTTP_ACCESS_TYPE_NO_PROXY
    } else {
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
    };
    let session = InternetHandle::new(unsafe {
        WinHttpOpen(
            w!("QuickPanel/0.1"),
            access,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    session.timeouts(deadline)?;
    // Do not spend five connect timeout intervals trying each address by default.
    session.set_u32(WINHTTP_OPTION_CONNECT_RETRIES, 1)?;
    let host = wide(&endpoint.host);
    let connection = InternetHandle::new(unsafe {
        WinHttpConnect(session.0, PCWSTR(host.as_ptr()), endpoint.port, 0)
    })?;
    let path = wide(&endpoint.path);
    let flags = if endpoint.secure {
        WINHTTP_FLAG_SECURE
    } else {
        WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    let handle = InternetHandle::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            match method {
                HttpMethod::Get => w!("GET"),
                HttpMethod::Post => w!("POST"),
            },
            PCWSTR(path.as_ptr()),
            w!("HTTP/1.1"),
            PCWSTR::null(),
            ptr::null(),
            flags,
        )
    })?;
    // Keep TLS verification at the system defaults; never set SECURITY_FLAG_IGNORE_*.
    handle.set_u32(
        WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
    )?;
    handle.set_u32(
        WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE,
        MAX_RESPONSE_HEADER_BYTES,
    )?;
    for (name, value) in &request.headers {
        let buffer = HeaderBuffer::new(name, value);
        unsafe {
            WinHttpAddRequestHeaders(
                handle.0,
                &buffer.0,
                WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
            )
        }
        .map_err(|_| "无法设置 AI 请求头".to_owned())?;
    }
    let length = u32::try_from(request.body.len()).map_err(|_| "AI 请求内容过大".to_owned())?;
    handle.timeouts(deadline)?;
    // Exactly one attempt per explicit request/page, without resend/status retries.
    unsafe {
        WinHttpSendRequest(
            handle.0,
            None,
            if request.body.is_empty() {
                None
            } else {
                Some(request.body.as_ptr().cast())
            },
            length,
            length,
            0,
        )
    }
    .map_err(|_| "AI 请求发送失败，请检查网络、API 地址和凭据".to_owned())?;
    handle.timeouts(deadline)?;
    unsafe { WinHttpReceiveResponse(handle.0, ptr::null_mut()) }
        .map_err(|_| "未能接收 AI 响应，请检查网络或稍后重试".to_owned())?;
    let status = query_number(&handle, WINHTTP_QUERY_STATUS_CODE)?;
    let status = u16::try_from(status).map_err(|_| "AI 返回了无效的 HTTP 状态".to_owned())?;
    if !(200..300).contains(&status) {
        // HTTP status is enough for our redacted error; never drain/display an error body.
        return Ok((status, Vec::new()));
    }
    let declared_size = query_number(&handle, WINHTTP_QUERY_CONTENT_LENGTH).ok();
    if declared_size.is_some_and(|size| size as usize > response_limit) {
        return Err(oversize.into());
    }
    let mut body = Vec::with_capacity(declared_size.unwrap_or(0) as usize);
    let mut buffer = [0u8; 8192];
    loop {
        handle.timeouts(deadline)?;
        let mut count = 0u32;
        // Read one extra byte at the exact limit so a longer chunked response is rejected.
        let wanted = buffer.len().min(response_limit + 1 - body.len());
        unsafe {
            WinHttpReadData(
                handle.0,
                buffer.as_mut_ptr().cast(),
                wanted as u32,
                &mut count,
            )
        }
        .map_err(|_| "AI 响应读取失败或超时，请稍后重试".to_owned())?;
        if count == 0 {
            break;
        }
        if count as usize > wanted || body.len() + count as usize > response_limit {
            return Err(oversize.into());
        }
        body.extend_from_slice(&buffer[..count as usize]);
    }
    if declared_size.is_some_and(|size| size as usize != body.len()) {
        return Err("AI 响应不完整，请稍后重试".into());
    }
    Ok((status, body))
}

/// Validate an enabled configuration locally; no connection or request is sent.
pub fn validate_settings(settings: &AiSettings) -> Result<(), String> {
    let key = SecretString(unprotect_key(&settings.encrypted_key)?);
    let _request = SensitiveRequest(prepare(settings, &key.0, "检查配置", Scope::All, &[])?);
    Ok(())
}

/// Interpret one explicitly requested query. Only a validated plan leaves this function.
pub fn interpret(
    settings: &AiSettings,
    query: &str,
    scope: Scope,
    functions: &[FunctionDescriptor],
) -> Result<AiPlan, String> {
    if !settings.enabled {
        return Err("AI 检索尚未启用".into());
    }
    let key = SecretString(unprotect_key(&settings.encrypted_key)?);
    interpret_with_key(settings, key, query, scope, functions)
}

/// One bounded metadata-only POST using the actor's exact revocable snapshot.
/// Search enablement is independent; the organizer switch is required.
pub fn organize(settings: &Settings, work: &Work) -> Result<Vec<Proposal>, String> {
    if !settings.organizer.enabled || !work.organizer_settings.enabled {
        return Err("AI 管家尚未启用".into());
    }
    if settings.ai != work.ai_settings || settings.organizer != work.organizer_settings {
        return Err("管家配置已变化，请使用新的整理任务".into());
    }
    let key = SecretString(unprotect_key(&work.ai_settings.encrypted_key)?);
    let request = SensitiveRequest(organizer::prepare(
        &work.ai_settings,
        &key.0,
        &work.organizer_settings,
        &work.candidates,
    )?);
    drop(key);
    let (status, body) = exchange(
        &request.0,
        organizer::MAX_RESPONSE_BYTES,
        "整理响应超过 64 KiB，已停止读取",
    )?;
    organizer::parse_response(status, &body, &work.candidates)
}

/// Explicit model discovery using entered or saved credentials, without saving
/// configuration, requiring a model ID, or sending a Messages inference request.
pub fn list_models(
    settings: &AiSettings,
    entered_key: String,
    clear_key: bool,
) -> Result<Vec<ModelInfo>, String> {
    list_models_with_key(settings, SecretString(entered_key), clear_key)
}

pub(crate) fn list_models_with_key(
    settings: &AiSettings,
    entered_key: SecretString,
    clear_key: bool,
) -> Result<Vec<ModelInfo>, String> {
    let key = connection_test_key(entered_key, clear_key, &settings.encrypted_key)?;
    let deadline = Instant::now() + REQUEST_BUDGET;
    let mut listing = ModelListing::default();
    let mut cursor: Option<String> = None;
    loop {
        let request = SensitiveRequest(ai_models::prepare(settings, &key.0, cursor.as_deref())?);
        let (status, body) = exchange_at(
            &request.0,
            ai_models::MAX_RESPONSE_BYTES,
            "模型列表响应超过 256 KiB，已停止读取",
            HttpMethod::Get,
            deadline,
        )?;
        cursor = listing.push_page(ai_models::parse_page(status, &body)?)?;
        if cursor.is_none() {
            return Ok(listing.into_models());
        }
    }
}

fn interpret_with_key(
    settings: &AiSettings,
    key: SecretString,
    query: &str,
    scope: Scope,
    functions: &[FunctionDescriptor],
) -> Result<AiPlan, String> {
    let request = SensitiveRequest(prepare(settings, &key.0, query, scope, functions)?);
    drop(key);
    let (status, body) = exchange(
        &request.0,
        MAX_RESPONSE_BYTES,
        "AI 响应超过 64 KiB，已停止读取",
    )?;
    parse_response(status, &body, functions)
}

/// Local-only validation of the independent embedding credential and protocol.
pub fn validate_embedding_settings(
    settings: &crate::semantic::EmbeddingSettings,
) -> Result<(), String> {
    let key = SecretString(unprotect_key(&settings.encrypted_key)?);
    let _request = SensitiveRequest(crate::embedding::prepare(
        settings,
        &key.0,
        &["检查配置".into()],
    )?);
    Ok(())
}
/// One explicit bounded request; run only on the independent embedding worker.
pub fn embed(
    settings: &crate::semantic::EmbeddingSettings,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    let key = SecretString(unprotect_key(&settings.encrypted_key)?);
    embed_with_key(settings, key, inputs)
}

fn embed_with_key(
    settings: &crate::semantic::EmbeddingSettings,
    key: SecretString,
    inputs: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    let request = SensitiveRequest(crate::embedding::prepare(settings, &key.0, inputs)?);
    drop(key);
    let (status, body) = exchange(
        &request.0,
        crate::embedding::MAX_RESPONSE_BYTES,
        "Embedding API 响应超过 2 MiB，已停止读取",
    )?;
    crate::embedding::parse_response(status, &body, inputs.len())
}

fn connection_test_key(
    entered: SecretString,
    clear_key: bool,
    saved: &[u8],
) -> Result<SecretString, String> {
    match credential_source(!entered.0.is_empty(), clear_key, !saved.is_empty())? {
        CredentialSource::Entered => Ok(entered),
        CredentialSource::Saved => Ok(SecretString(unprotect_key(saved)?)),
    }
}

/// One explicit real Messages POST with built-in text and an empty function catalogue.
/// The configuration remains untouched, and a disabled feature can still be tested.
pub fn test_ai_connection(
    settings: &AiSettings,
    entered_key: String,
    clear_key: bool,
) -> Result<(), String> {
    test_ai_connection_with_key(settings, SecretString(entered_key), clear_key)
}

pub(crate) fn test_ai_connection_with_key(
    settings: &AiSettings,
    entered_key: SecretString,
    clear_key: bool,
) -> Result<(), String> {
    let key = connection_test_key(entered_key, clear_key, &settings.encrypted_key)?;
    let mut test_settings = settings.clone();
    test_settings.enabled = true;
    interpret_with_key(&test_settings, key, CONNECTION_TEST_TEXT, Scope::All, &[]).map(|_| ())
}

/// One explicit real Embeddings POST containing only built-in text; returns its dimension.
pub fn test_embedding_connection(
    settings: &crate::semantic::EmbeddingSettings,
    entered_key: String,
    clear_key: bool,
) -> Result<usize, String> {
    test_embedding_connection_with_key(settings, SecretString(entered_key), clear_key)
}

pub(crate) fn test_embedding_connection_with_key(
    settings: &crate::semantic::EmbeddingSettings,
    entered_key: SecretString,
    clear_key: bool,
) -> Result<usize, String> {
    let key = connection_test_key(entered_key, clear_key, &settings.encrypted_key)?;
    let mut test_settings = settings.clone();
    test_settings.enabled = true;
    let vectors = embed_with_key(&test_settings, key, &[CONNECTION_TEST_TEXT.into()])?;
    Ok(vectors[0].len())
}
