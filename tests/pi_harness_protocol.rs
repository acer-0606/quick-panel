use quick_panel::pi_harness::parse_runtime_frame;
#[test]
fn protocol_rejects_unknown_operation_and_oversized_checkpoint() {
    assert!(parse_runtime_frame(br#"{"type":"shell","command":"x"}"#).is_err());
    let bytes = vec![b'x'; 256 * 1024 + 1];
    assert!(parse_runtime_frame(&bytes).is_err());
}
#[test]
fn protocol_rejects_unknown_ready_fields() {
    assert!(parse_runtime_frame(br#"{"type":"ready","protocol":1,"api_key":"secret"}"#).is_err());
}

use quick_panel::pi_harness::{
    Checkpoint, HostFrame, HostResult, MAX_FRAME_BYTES, ModelResponse, RuntimeFrame,
    parse_host_frame, read_runtime_frame,
};
use serde_json::json;
use std::io::{BufReader, Cursor};

#[test]
fn structured_null_result_is_not_confused_with_a_missing_value() {
    let frame = parse_host_frame(br#"{"type":"tool_result","request_id":"fixture","call_id":"test:1","result":{"ok":true,"value":null}}"#).unwrap();
    match frame {
        HostFrame::ToolResult { result, .. } => {
            assert_eq!(result.into_result().unwrap(), serde_json::Value::Null)
        }
        _ => panic!("wrong frame"),
    }
}
#[test]
fn streaming_reader_rejects_unterminated_oversize_without_consuming_unbounded_input() {
    let mut reader =
        BufReader::with_capacity(1024, Cursor::new(vec![b'x'; MAX_FRAME_BYTES + 9999]));
    assert!(read_runtime_frame(&mut reader).is_err());
    assert!(reader.get_ref().position() <= (MAX_FRAME_BYTES + 1024) as u64);
    assert!(read_runtime_frame(&mut Cursor::new(br#"{"type":"ready","protocol":1}"#)).is_err());
    assert!(read_runtime_frame(&mut Cursor::new(b"")).unwrap().is_none());
    assert!(matches!(
        read_runtime_frame(&mut Cursor::new(b"{\"type\":\"ready\",\"protocol\":1}\r\n")).unwrap(),
        Some(RuntimeFrame::Ready { protocol: 1 })
    ));
}
#[test]
fn host_and_runtime_discriminators_and_correlation_are_strict() {
    for raw in [
        r#"{"type":"ready","protocol":2}"#,
        r#"{"type":"tool","request_id":"fixture","call_id":"a","name":"shell","arguments":{}}"#,
        r#"{"type":"finish","request_id":"","call_id":"a"}"#,
    ] {
        assert!(parse_runtime_frame(raw.as_bytes()).is_err());
    }
    for result in [
        json!({"ok":false,"value":{}}),
        json!({"ok":true,"value":{},"error":{"code":"x","message":"x","retryable":false}}),
        json!({"ok":true}),
    ] {
        let raw =
            json!({"type":"tool_result","request_id":"fixture","call_id":"a","result":result});
        assert!(parse_host_frame(&serde_json::to_vec(&raw).unwrap()).is_err());
    }
}
#[test]
fn model_response_rejects_natural_language_operations_and_oversized_content() {
    assert!(
        ModelResponse {
            content: vec![json!({"type":"shell","command":"x"})],
            stop_reason: "end_turn".into()
        }
        .validate()
        .is_err()
    );
    assert!(
        ModelResponse {
            content: vec![json!({"type":"text","text":"x".repeat(64*1024)})],
            stop_reason: "end_turn".into()
        }
        .validate()
        .is_err()
    );
    let invalid = HostFrame::LlmResult {
        request_id: "fixture".into(),
        call_id: "a".into(),
        result: HostResult::success(ModelResponse {
            content: vec![],
            stop_reason: "surprise".into(),
        }),
    };
    assert!(invalid.validate().is_err());
}
#[test]
fn checkpoint_rejects_byte_and_counter_limits() {
    for cp in [
        Checkpoint {
            messages_json: "x".repeat(128 * 1024),
            rounds: 0,
            calls: 0,
            elapsed_ms: 0,
        },
        Checkpoint {
            messages_json: "[]".into(),
            rounds: 9,
            calls: 0,
            elapsed_ms: 0,
        },
        Checkpoint {
            messages_json: "[]".into(),
            rounds: 0,
            calls: 33,
            elapsed_ms: 0,
        },
        Checkpoint {
            messages_json: "[]".into(),
            rounds: 0,
            calls: 0,
            elapsed_ms: 120001,
        },
    ] {
        assert!(cp.validate().is_err());
    }
}
#[test]
fn shared_fixture_round_trips_both_directions_and_checkpoint_transcripts() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../agent-runtime/fixtures/protocol-v1.json")).unwrap();
    for frame in fixture["host"].as_array().unwrap() {
        let parsed = parse_host_frame(&serde_json::to_vec(frame).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), *frame);
    }
    for frame in fixture["runtime"].as_array().unwrap() {
        let parsed = parse_runtime_frame(&serde_json::to_vec(frame).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), *frame);
    }
    let cp = fixture["runtime"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["type"] == "checkpoint")
        .unwrap()["checkpoint"]
        .clone();
    let mut valid: Checkpoint = serde_json::from_value(cp).unwrap();
    valid.validate().unwrap();
    let mut m: Vec<serde_json::Value> = serde_json::from_str(&valid.messages_json).unwrap();
    m.extend((0..63).map(|_| json!({"role":"user","content":"x","timestamp":0})));
    valid.messages_json = serde_json::to_string(&m).unwrap();
    assert!(valid.validate().is_err());
}
#[test]
fn checkpoint_rejects_pending_or_orphaned_tools_and_preserves_counter_reservations() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../agent-runtime/fixtures/protocol-v1.json")).unwrap();
    let value = fixture["runtime"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|f| f["type"] == "checkpoint")
        .unwrap()["checkpoint"]
        .clone();
    let cp: Checkpoint = serde_json::from_value(value).unwrap();
    cp.validate().unwrap();
    let mut m: Vec<serde_json::Value> = serde_json::from_str(&cp.messages_json).unwrap();
    m.pop();
    let mut pending = cp.clone();
    pending.messages_json = serde_json::to_string(&m).unwrap();
    assert!(pending.validate().is_err());
    let mut m: Vec<serde_json::Value> = serde_json::from_str(&cp.messages_json).unwrap();
    m.last_mut().unwrap()["toolCallId"] = json!("orphan");
    let mut orphan = cp.clone();
    orphan.messages_json = serde_json::to_string(&m).unwrap();
    assert!(orphan.validate().is_err());
    let mut counters = cp.clone();
    counters.rounds = 0;
    assert!(counters.validate().is_err());
    counters = cp.clone();
    counters.calls = 0;
    assert!(counters.validate().is_err());
}
#[test]
fn request_result_and_receipt_budgets_are_enforced_on_serialization() {
    use quick_panel::pi_harness::{CompletionReceipt, ModelRequest, encode_host_frame};
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../agent-runtime/fixtures/protocol-v1.json")).unwrap();
    let v = &fixture["runtime"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["type"] == "llm")
        .unwrap()["request"];
    let mut request: ModelRequest = serde_json::from_value(v.clone()).unwrap();
    request.system = "x".repeat(192 * 1024);
    assert!(request.validate().is_err());
    let frame = HostFrame::ToolResult {
        request_id: "fixture".into(),
        call_id: "a".into(),
        result: HostResult::success(json!({"text":"x".repeat(16*1024)})),
    };
    assert!(encode_host_frame(&frame).is_err());
    let frame = HostFrame::FinishResult {
        request_id: "fixture".into(),
        call_id: "a".into(),
        result: HostResult::success(CompletionReceipt {
            accepted: true,
            submitted_indices: vec![0, 0],
        }),
    };
    assert!(encode_host_frame(&frame).is_err());
}
#[test]
fn signed_thinking_is_valid_structured_model_content() {
    let response = ModelResponse {
        content: vec![
            json!({"type":"thinking","thinking":"synthetic","signature":"signature-fixture"}),
            json!({"type":"redacted_thinking","data":"opaque-fixture"}),
        ],
        stop_reason: "end_turn".into(),
    };
    assert!(response.validate().is_ok());
}
