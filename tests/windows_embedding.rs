//! Local WinHTTP/DPAPI fixtures; compile-only on non-Windows hosts.
#![cfg(windows)]
use quick_panel::{
    platform::windows::ai_client::{
        embed, protect_key, test_embedding_connection, validate_embedding_settings,
    },
    semantic::EmbeddingSettings,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const DUMMY_KEY: &str = "fixture-embedding-key";

// Reusing saved settings or batching user data would change the observed POST here.
#[test]
fn connection_test_posts_one_fixed_text_with_unsaved_disabled_config_and_reports_dimension() {
    let (endpoint, server) = serve(response(
        "200 OK",
        "",
        br#"{"data":[{"index":0,"embedding":[1,0,0]}]}"#,
    ));
    let mut config = settings(endpoint);
    config.enabled = false;
    config.model = "unsaved-embedding-model".into();
    let original = config.clone();
    assert_eq!(
        test_embedding_connection(&config, "unsaved-embedding-key".into(), false).unwrap(),
        3
    );
    assert_eq!(config, original);
    let request = server.join().unwrap();
    assert_eq!(request.line, "POST /v1/embeddings HTTP/1.1");
    assert_eq!(
        request.headers["authorization"],
        "Bearer unsaved-embedding-key"
    );
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "unsaved-embedding-model");
    assert_eq!(
        body["input"],
        serde_json::json!(["Quick Panel connection test"])
    );
    assert_eq!(body["encoding_format"], "float");
}

#[test]
fn connection_test_empty_password_uses_saved_key_but_clear_never_posts() {
    let (endpoint, server) = serve(response(
        "200 OK",
        "",
        br#"{"data":[{"index":0,"embedding":[1,0]}]}"#,
    ));
    assert_eq!(
        test_embedding_connection(&settings(endpoint), String::new(), false).unwrap(),
        2
    );
    assert_eq!(
        server.join().unwrap().headers["authorization"],
        "Bearer fixture-embedding-key"
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = settings(format!("http://{}", listener.local_addr().unwrap()));
    for password in ["", "unsaved-embedding-key"] {
        let error = test_embedding_connection(&config, password.into(), true).unwrap_err();
        assert!(!error.contains(DUMMY_KEY));
        assert!(!error.contains("unsaved-embedding-key"));
        assert!(matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
    }
}

#[test]
fn connection_test_rejects_invalid_vector_and_redacts_service_failure() {
    for (status, body) in [
        (
            "200 OK",
            &br#"{"data":[{"index":0,"embedding":[0,0]}]}"#[..],
        ),
        (
            "503 Service Unavailable",
            &b"private-provider-body unsaved-embedding-key"[..],
        ),
    ] {
        let (endpoint, server) = serve(response(status, "", body));
        let error =
            test_embedding_connection(&settings(endpoint), "unsaved-embedding-key".into(), false)
                .unwrap_err();
        assert!(!error.contains("private-provider-body"));
        assert!(!error.contains("unsaved-embedding-key"));
        server.join().unwrap();
    }
}
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
    let endpoint = format!("http://{}/v1/embeddings", listener.local_addr().unwrap());
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
        // Keep the listener until embed() returns; extra connections would be retries.
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

fn settings(endpoint: String) -> EmbeddingSettings {
    EmbeddingSettings {
        enabled: true,
        endpoint,
        model: "fixture-model".into(),
        encrypted_key: protect_key(DUMMY_KEY).unwrap(),
        ..Default::default()
    }
}
#[test]
fn validation_uses_dpapi_and_never_posts() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut config = settings(format!("http://{}", listener.local_addr().unwrap()));
    validate_embedding_settings(&config).unwrap();
    config.encrypted_key[0] ^= 0x80;
    assert!(validate_embedding_settings(&config).is_err());
    assert!(matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock));
}
#[test]
fn post_is_bearer_and_reorders_vectors_by_input_index() {
    let (endpoint, server) = serve(response(
        "200 OK",
        "",
        br#"{"data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#,
    ));
    let result = embed(&settings(endpoint), &["查询".into(), "saved text".into()]).unwrap();
    assert_eq!(result, [vec![1., 0.], vec![0., 1.]]);
    let request = server.join().unwrap();
    assert_eq!(request.line, "POST /v1/embeddings HTTP/1.1");
    assert_eq!(
        request.headers["authorization"],
        "Bearer fixture-embedding-key"
    );
    let json: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(json["input"], serde_json::json!(["查询", "saved text"]));
    assert_eq!(json["encoding_format"], "float");
    assert_eq!(json["model"], "fixture-model");
}
#[test]
fn embedding_accepts_response_above_claude_budget() {
    let mut body = br#"{"data":[{"index":0,"embedding":[1,0]}]}"#.to_vec();
    body.resize(96 * 1024, b' ');
    let (endpoint, server) = serve(response("200 OK", "", &body));
    assert_eq!(
        embed(&settings(endpoint), &["text".into()]).unwrap(),
        [vec![1., 0.]]
    );
    server.join().unwrap();
}
#[test]
fn oversize_declared_and_chunked_responses_stop_at_embedding_budget() {
    for chunked in [false, true] {
        let body = vec![b'x'; 2 * 1024 * 1024 + 1];
        let reply = if chunked {
            let mut reply =
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                    .to_vec();
            reply.extend_from_slice(format!("{:x}\r\n", body.len()).as_bytes());
            reply.extend(body);
            reply.extend_from_slice(b"\r\n0\r\n\r\n");
            reply
        } else {
            response("200 OK", "", &body)
        };
        let (endpoint, server) = serve(reply);
        assert!(
            embed(&settings(endpoint), &["text".into()])
                .unwrap_err()
                .contains("2 MiB")
        );
        server.join().unwrap();
    }
}
#[test]
fn service_error_and_redirect_are_sanitized_without_retry() {
    for status in [
        "401 Unauthorized",
        "429 Too Many Requests",
        "503 Service Unavailable",
        "302 Found",
    ] {
        let (endpoint, server) = serve(response(
            status,
            "Location: http://127.0.0.1:1/\r\n",
            b"sensitive provider error",
        ));
        let error = embed(&settings(endpoint), &["text".into()]).unwrap_err();
        assert!(!error.contains("sensitive"));
        assert!(!error.contains(DUMMY_KEY));
        server.join().unwrap();
    }
}
