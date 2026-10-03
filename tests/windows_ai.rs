//! Real Windows API integration tests. Cross-compilation does not execute them.
#![cfg(windows)]

use quick_panel::{
    ai::{AiSettings, AuthMethod, FunctionDescriptor, Period},
    model::Scope,
    platform::windows::ai_client::{interpret, protect_key, unprotect_key, validate_settings},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const DUMMY_KEY: &str = "fixture-api-key-not-a-real-credential";
const PLAN_RESPONSE: &str = r#"{"id":"msg_fixture","type":"message","role":"assistant","model":"fixture-model","content":[{"type":"text","text":"{\"queries\":[\"连接超时\",\"connection timeout\"],\"scope\":\"clipboard\",\"period\":\"yesterday\",\"functions\":[\"pause_capture\"]}"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}"#;

struct ObservedRequest {
    line: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

// This fixture accepts exactly one real HTTP request; it never contacts a service.
fn serve(response: Vec<u8>) -> (String, JoinHandle<ObservedRequest>) {
    serve_delayed(response, Duration::ZERO)
}

fn serve_delayed(response: Vec<u8>, delay: Duration) -> (String, JoinHandle<ObservedRequest>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "fixture received no request");
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = read_request(&mut stream);
        thread::sleep(delay);
        // Oversize-response tests may close the client before this write finishes.
        let _ = stream.write_all(&response);
        drop(stream);
        // Keep the listener until interpret() returns; extra connections would be retries.
        thread::sleep(Duration::from_millis(150));
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock),
            "one invocation must not send another request"
        );
        request
    });
    (endpoint, worker)
}

fn read_request(stream: &mut TcpStream) -> ObservedRequest {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "request closed before headers");
        bytes.extend_from_slice(&chunk[..count]);
        assert!(
            bytes.len() <= 128 * 1024,
            "fixture request unexpectedly large"
        );
        if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let header_text = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let mut lines = header_text.split("\r\n");
    let line = lines.next().unwrap().to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers.get("content-length").unwrap().parse().unwrap();
    assert!(length <= 128 * 1024);
    while bytes.len() - header_end < length {
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "request closed before body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    ObservedRequest {
        line,
        headers,
        body: bytes[header_end..header_end + length].to_vec(),
    }
}

fn response(status: &str, extra_headers: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n", body.len()).into_bytes();
    response.extend_from_slice(body);
    response
}

fn settings(endpoint: String, auth: AuthMethod) -> AiSettings {
    AiSettings {
        enabled: true,
        endpoint,
        model: "fixture-model".into(),
        auth,
        encrypted_key: protect_key(DUMMY_KEY).unwrap(),
    }
}

fn functions() -> Vec<FunctionDescriptor> {
    vec![FunctionDescriptor {
        id: "pause_capture".into(),
        title: "暂停记录".into(),
        description: "暂停剪贴板捕获".into(),
    }]
}

#[test]
fn dpapi_round_trip_and_tampered_ciphertext_rejection() {
    for secret in [DUMMY_KEY, "fixture-密钥-日本語"] {
        let mut encrypted = protect_key(secret).unwrap();
        assert_ne!(encrypted.as_slice(), secret.as_bytes());
        assert_eq!(unprotect_key(&encrypted).unwrap(), secret);
        let middle = encrypted.len() / 2;
        encrypted[middle] ^= 0x80;
        let error = unprotect_key(&encrypted).unwrap_err();
        assert!(!error.contains(secret));
    }
}

#[test]
fn dpapi_rejects_empty_and_unreasonably_large_credentials() {
    assert!(protect_key("").is_err());
    assert!(protect_key(&"x".repeat(64 * 1024)).is_err());
    assert!(unprotect_key(&[]).is_err());
    assert!(unprotect_key(&vec![0; 128 * 1024]).is_err());
}

#[test]
fn validating_enabled_settings_checks_protected_credentials_without_network() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut config = settings(
        format!("http://{}/v1/messages", listener.local_addr().unwrap()),
        AuthMethod::ApiKey,
    );
    assert!(validate_settings(&config).is_ok());
    config.model.clear();
    assert!(validate_settings(&config).is_err());
    assert!(matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
}

#[test]
fn validating_settings_rejects_invalid_auth_data_without_echoing_plaintext() {
    let mut config = settings("https://api.anthropic.com".into(), AuthMethod::Bearer);
    let dummy_invalid = "fixture invalid credential";
    config.encrypted_key = protect_key(dummy_invalid).unwrap();
    let error = validate_settings(&config).unwrap_err();
    assert!(!error.contains(dummy_invalid));
}

#[test]
fn winhttp_sends_real_messages_post_with_api_key_and_returns_valid_plan() {
    let (endpoint, server) = serve(response("200 OK", "", PLAN_RESPONSE.as_bytes()));
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "之前那个连接超时",
        Scope::All,
        &functions(),
    );
    let request = server.join().unwrap();
    let plan = result.unwrap();
    assert_eq!(plan.queries, ["连接超时", "connection timeout"]);
    assert_eq!(plan.scope, Some(Scope::Clipboard));
    assert_eq!(plan.period, Period::Yesterday);
    assert_eq!(plan.functions, ["pause_capture"]);
    assert_eq!(request.line, "POST /v1/messages HTTP/1.1");
    assert_eq!(
        request.headers.get("x-api-key").map(String::as_str),
        Some(DUMMY_KEY)
    );
    assert_eq!(
        request.headers.get("anthropic-version").map(String::as_str),
        Some("2023-06-01")
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert!(!request.headers.contains_key("authorization"));
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "fixture-model");
    assert_eq!(body["stream"], false);
    assert!(body["max_tokens"].as_u64().unwrap() > 0);
    assert!(body.get("system").is_some());
    assert_eq!(body["messages"][0]["role"], "user");
    assert!(
        serde_json::to_string(&body["messages"])
            .unwrap()
            .contains("之前那个连接超时")
    );
}

#[test]
fn winhttp_bearer_configuration_changes_only_authentication_header() {
    let (endpoint, server) = serve(response("200 OK", "", PLAN_RESPONSE.as_bytes()));
    let result = interpret(
        &settings(endpoint, AuthMethod::Bearer),
        "连接超时",
        Scope::Clipboard,
        &functions(),
    );
    let request = server.join().unwrap();
    assert!(result.is_ok());
    assert_eq!(
        request.headers.get("authorization"),
        Some(&format!("Bearer {DUMMY_KEY}"))
    );
    assert!(!request.headers.contains_key("x-api-key"));
    assert_eq!(request.line, "POST /v1/messages HTTP/1.1");
}

#[test]
fn winhttp_never_follows_redirect_with_a_credential() {
    let destination = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    destination.set_nonblocking(true).unwrap();
    let location = format!(
        "Location: http://{}/collect\r\n",
        destination.local_addr().unwrap()
    );
    let (endpoint, server) = serve(response("302 Found", &location, b""));
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "query",
        Scope::All,
        &functions(),
    );
    server.join().unwrap();
    assert!(result.is_err());
    assert!(matches!(destination.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
}

#[test]
fn winhttp_rejects_oversize_response_without_returning_server_text() {
    let oversized = vec![b'x'; 64 * 1024 + 1];
    let (endpoint, server) = serve(response("200 OK", "", &oversized));
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "query",
        Scope::All,
        &functions(),
    );
    server.join().unwrap();
    let error = result.unwrap_err();
    assert!(!error.contains(&"x".repeat(32)));
}

#[test]
fn winhttp_rejects_chunked_response_after_total_body_limit() {
    let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n".to_vec();
    for _ in 0..17 {
        raw.extend_from_slice(b"1000\r\n");
        raw.extend_from_slice(&[b'x'; 4096]);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");
    let (endpoint, server) = serve(raw);
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "query",
        Scope::All,
        &functions(),
    );
    server.join().unwrap();
    assert!(result.is_err());
}

#[test]
fn winhttp_reports_http_error_without_server_body_or_retry() {
    let private_body = b"server-private-diagnostic fixture-api-key-not-a-real-credential";
    let (endpoint, server) = serve(response("429 Too Many Requests", "", private_body));
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "query",
        Scope::All,
        &functions(),
    );
    server.join().unwrap();
    let error = result.unwrap_err();
    assert!(!error.contains("server-private-diagnostic"));
    assert!(!error.contains(DUMMY_KEY));
}

#[test]
fn winhttp_rejects_truncated_response_without_waiting_for_unbounded_data() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 65536\r\nConnection: close\r\n\r\n{\"content\":".to_vec();
    let (endpoint, server) = serve(raw);
    let started = Instant::now();
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "query",
        Scope::All,
        &functions(),
    );
    server.join().unwrap();
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn winhttp_receive_timeout_stops_a_silent_server() {
    let (endpoint, server) = serve_delayed(
        response("200 OK", "", PLAN_RESPONSE.as_bytes()),
        Duration::from_secs(10),
    );
    let started = Instant::now();
    let result = interpret(
        &settings(endpoint, AuthMethod::ApiKey),
        "query",
        Scope::All,
        &functions(),
    );
    let elapsed = started.elapsed();
    server.join().unwrap();
    assert!(result.is_err());
    assert!(
        elapsed < Duration::from_secs(10),
        "receive timeout did not stop waiting"
    );
}

#[test]
fn disabled_configuration_does_not_send_a_request() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut settings = settings(
        format!("http://{}/v1/messages", listener.local_addr().unwrap()),
        AuthMethod::ApiKey,
    );
    settings.enabled = false;
    assert!(interpret(&settings, "query", Scope::All, &functions()).is_err());
    assert!(matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
}
