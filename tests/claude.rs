use quick_panel::ai::{AiSettings, AuthMethod, Effort, FunctionDescriptor, Period};
use quick_panel::claude::{parse_endpoint, parse_response, prepare};
use quick_panel::model::Scope;
use serde_json::{Value, json};

fn settings() -> AiSettings {
    AiSettings {
        enabled: true,
        endpoint: "https://api.anthropic.com".into(),
        model: "claude-sonnet-test".into(),
        auth: AuthMethod::ApiKey,
        effort: Effort::Default,
        encrypted_key: b"not-a-plaintext-key".to_vec(),
    }
}

fn functions() -> Vec<FunctionDescriptor> {
    vec![FunctionDescriptor {
        id: "pause_capture".into(),
        title: "暂停记录".into(),
        description: "暂停捕获新的剪贴板内容".into(),
    }]
}

fn plan_response(plan: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": "msg_fixture", "type": "message", "role": "assistant",
        "model": "claude-sonnet-test",
        "content": [{"type":"text", "text":plan.to_string()}],
        "stop_reason": "end_turn", "stop_sequence": null,
        "usage": {"input_tokens":100,"output_tokens":40}
    }))
    .unwrap()
}

fn basic_plan() -> Value {
    json!({"queries":["连接超时","connection timeout"],"scope":"clipboard",
        "period":"yesterday","functions":[]})
}

#[test]
fn endpoint_normalizes_roots_versions_and_custom_proxy_prefixes() {
    for (input, host, port, path, secure) in [
        (
            "https://api.anthropic.com",
            "api.anthropic.com",
            443,
            "/v1/messages",
            true,
        ),
        (
            "https://api.anthropic.com/v1/",
            "api.anthropic.com",
            443,
            "/v1/messages",
            true,
        ),
        (
            "https://api.anthropic.com/v1/messages",
            "api.anthropic.com",
            443,
            "/v1/messages",
            true,
        ),
        (
            "https://proxy.example:8443/claude",
            "proxy.example",
            8443,
            "/claude/v1/messages",
            true,
        ),
        (
            "https://proxy.example/base/v1",
            "proxy.example",
            443,
            "/base/v1/messages",
            true,
        ),
        (
            "http://127.0.0.1:12345/v1/messages",
            "127.0.0.1",
            12345,
            "/v1/messages",
            false,
        ),
        (
            "http://[::1]:2345/proxy/",
            "::1",
            2345,
            "/proxy/v1/messages",
            false,
        ),
    ] {
        let endpoint = parse_endpoint(input).unwrap();
        assert_eq!(
            (
                endpoint.host.as_str(),
                endpoint.port,
                endpoint.path.as_str(),
                endpoint.secure
            ),
            (host, port, path, secure),
            "{input}"
        );
    }
}

#[test]
fn endpoint_rejects_ambiguous_credentials_queries_fragments_and_injection() {
    for input in [
        "ftp://example.com",
        "https://",
        "https://user:pass@example.com",
        "https://example.com/#secret",
        "https://example.com?key=secret",
        "https://example.com\r\nX-Test: value",
        "https://example.com/hello world",
        "https://example.com:0",
        "https://example.com:65536",
        "https://example.com:abc",
        "https://example.com:/",
        "https://::1/v1",
        "https://[not-ip]/",
        "https://example.com\\other",
        "https://example.com/a/../b",
        "https://example.com/%0d%0aHeader",
        "https://example.com/%ZZ",
        "https://exa_mple.com",
    ] {
        assert!(parse_endpoint(input).is_err(), "accepted {input:?}");
    }
    assert!(parse_endpoint(&format!("https://example.com/{}", "a".repeat(2048))).is_err());
}

#[test]
fn request_uses_messages_wire_shape_and_only_query_scope_public_catalogue() {
    let request = prepare(
        &settings(),
        "sk-fixture-key",
        "昨天复制的连接错误",
        Scope::Clipboard,
        &functions(),
    )
    .unwrap();
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "x-api-key" && value == "sk-fixture-key")
    );
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "anthropic-version" && value == "2023-06-01")
    );
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["model"], "claude-sonnet-test");
    assert_eq!(body["max_tokens"], 768);
    assert_eq!(body["stream"], false);
    assert!(body["system"].as_str().unwrap().contains("queries"));
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let payload: Value = serde_json::from_str(messages[0]["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        payload,
        json!({"query":"昨天复制的连接错误","scope":"clipboard", "functions":[{
        "id":"pause_capture","title":"暂停记录","description":"暂停捕获新的剪贴板内容"}]})
    );
    let wire = String::from_utf8(request.body).unwrap();
    assert!(!wire.contains("sk-fixture-key"));
    assert!(!wire.contains("not-a-plaintext-key"));
}

#[test]
fn bearer_auth_replaces_api_key_and_debug_redacts_credentials_and_query() {
    let mut config = settings();
    config.auth = AuthMethod::Bearer;
    let request = prepare(
        &config,
        "fixture-sensitive-key",
        "fixture-sensitive-query",
        Scope::All,
        &functions(),
    )
    .unwrap();
    assert!(request.headers.iter().any(|(name, value)| name == "Authorization" && value == "Bearer fixture-sensitive-key"));
    assert!(!request.headers.iter().any(|(name, _)| name == "x-api-key"));
    let debug = format!("{request:?}");
    assert!(!debug.contains("fixture-sensitive-key"));
    assert!(!debug.contains("fixture-sensitive-query"));
}

#[test]
fn request_rejects_disabled_config_and_malformed_or_oversized_input() {
    let mut config = settings();
    config.enabled = false;
    assert!(prepare(&config, "fixture-key", "检索", Scope::All, &[]).is_err());
    config.enabled = true;
    for key in [
        "",
        " ",
        "key\r\nInjected: yes",
        "key\0",
        "key value",
        "秘密",
    ] {
        assert!(prepare(&config, key, "检索", Scope::All, &[]).is_err());
    }
    assert!(prepare(&config, &"a".repeat(4097), "检索", Scope::All, &[]).is_err());
    for query in [
        "".to_owned(),
        "  ".to_owned(),
        "a\0b".to_owned(),
        "检".repeat(2049),
    ] {
        assert!(prepare(&config, "fixture-key", &query, Scope::All, &[]).is_err());
    }
    config.model.clear();
    assert!(prepare(&config, "fixture-key", "检索", Scope::All, &[]).is_err());
    config.model = "a".repeat(257);
    assert!(prepare(&config, "fixture-key", "检索", Scope::All, &[]).is_err());
    config.model = "model\nother".into();
    assert!(prepare(&config, "fixture-key", "检索", Scope::All, &[]).is_err());
}

#[test]
fn request_bounds_and_validates_public_catalogue() {
    let invalid = FunctionDescriptor {
        id: "arbitrary/path".into(),
        title: "功能".into(),
        description: "描述".into(),
    };
    assert!(prepare(&settings(), "fixture-key", "查找", Scope::All, &[invalid]).is_err());
    let repeated = functions().into_iter().cycle().take(65).collect::<Vec<_>>();
    assert!(prepare(&settings(), "fixture-key", "查找", Scope::All, &repeated).is_err());
    let mut oversized = functions();
    oversized[0].description = "a".repeat(513);
    assert!(prepare(&settings(), "fixture-key", "查找", Scope::All, &oversized).is_err());
    let valid = prepare(
        &settings(),
        "fixture-key",
        &"检".repeat(2048),
        Scope::All,
        &functions(),
    )
    .unwrap();
    assert!(valid.body.len() <= 65536);
    let large_but_individually_valid = (0..64)
        .map(|index| FunctionDescriptor {
            id: format!("function_{index}"),
            title: "名".repeat(128),
            description: "描".repeat(512),
        })
        .collect::<Vec<_>>();
    assert!(
        prepare(
            &settings(),
            "fixture-key",
            "查找",
            Scope::All,
            &large_but_individually_valid
        )
        .is_err()
    );
}

#[test]
fn request_accepts_internal_palette_function_ids_and_validates_their_return() {
    let catalogue = [FunctionDescriptor {
        id: "function:pause".into(),
        title: "暂停记录".into(),
        description: "暂停捕获新的剪贴板内容".into(),
    }];
    prepare(
        &settings(),
        "fixture-key",
        "暂停记录",
        Scope::All,
        &catalogue,
    )
    .unwrap();
    let plan = json!({"queries":[],"scope":null,"period":"any","functions":["function:pause"]});
    assert_eq!(
        parse_response(200, &plan_response(plan), &catalogue)
            .unwrap()
            .functions,
        ["function:pause"]
    );
}

#[test]
fn response_turns_real_text_json_into_only_validated_queries_filters_and_functions() {
    let parsed = parse_response(200, &plan_response(basic_plan()), &functions()).unwrap();
    assert_eq!(parsed.queries, ["连接超时", "connection timeout"]);
    assert_eq!(parsed.scope, Some(Scope::Clipboard));
    assert_eq!(parsed.period, Period::Yesterday);
    assert!(parsed.functions.is_empty());
    let function = json!({"queries":[],"scope":null,"period":"any","functions":["pause_capture"]});
    assert_eq!(
        parse_response(200, &plan_response(function), &functions())
            .unwrap()
            .functions,
        ["pause_capture"]
    );
    let browsing = json!({"queries":[],"scope":"folder","period":"last_7_days","functions":[]});
    let parsed = parse_response(200, &plan_response(browsing), &[]).unwrap();
    assert_eq!(parsed.scope, Some(Scope::Folder));
    assert_eq!(parsed.period, Period::Last7Days);
}

#[test]
fn response_rejects_unknown_fields_scopes_periods_functions_and_non_search_actions() {
    for extra in [
        json!({"path":"C:\\Users\\secret"}),
        json!({"action":"delete"}),
        json!({"items":[]}),
    ] {
        let mut plan = basic_plan();
        plan.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(parse_response(200, &plan_response(plan), &functions()).is_err());
    }
    for (field, value) in [
        ("scope", json!("files")),
        ("period", json!("forever")),
        ("functions", json!(["run_shell"])),
        ("queries", json!("text")),
    ] {
        let mut plan = basic_plan();
        plan[field] = value;
        assert!(parse_response(200, &plan_response(plan), &functions()).is_err());
    }
}

#[test]
fn response_bounds_queries_and_rejects_empty_or_control_character_terms() {
    for queries in [
        json!(["x", "x", "x", "x", "x", "x", "x"]),
        json!(["a".repeat(65)]),
        json!([""]),
        json!(["  "]),
        json!(["x\0y"]),
        json!(["x\ny"]),
    ] {
        let mut plan = basic_plan();
        plan["queries"] = queries;
        assert!(parse_response(200, &plan_response(plan), &functions()).is_err());
    }
    let mut plan = basic_plan();
    plan["functions"] = json!([
        "pause_capture",
        "pause_capture",
        "pause_capture",
        "pause_capture"
    ]);
    assert!(parse_response(200, &plan_response(plan), &functions()).is_err());
    let meaningless = json!({"queries":[],"scope":null,"period":"any","functions":[]});
    assert!(parse_response(200, &plan_response(meaningless), &functions()).is_err());
}

#[test]
fn response_rejects_truncation_refusal_tool_use_and_unbounded_or_non_json_content() {
    for stop in [
        "max_tokens",
        "refusal",
        "tool_use",
        "pause_turn",
        "stop_sequence",
        "",
    ] {
        let mut response: Value = serde_json::from_slice(&plan_response(basic_plan())).unwrap();
        response["stop_reason"] = json!(stop);
        assert!(
            parse_response(200, &serde_json::to_vec(&response).unwrap(), &functions()).is_err()
        );
    }
    let mut response: Value = serde_json::from_slice(&plan_response(basic_plan())).unwrap();
    response["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool_use","id":"x","name":"run_shell","input":{}}));
    assert!(parse_response(200, &serde_json::to_vec(&response).unwrap(), &functions()).is_err());
    response["content"] = json!([{"type":"text","text":"This is not a query plan"}]);
    assert!(parse_response(200, &serde_json::to_vec(&response).unwrap(), &functions()).is_err());
    let mut refusal: Value = serde_json::from_slice(&plan_response(basic_plan())).unwrap();
    refusal["stop_details"] =
        json!({"type":"refusal","category":"test","explanation":"not for display"});
    assert!(parse_response(200, &serde_json::to_vec(&refusal).unwrap(), &functions()).is_err());
    assert!(parse_response(200, &vec![b' '; 65537], &[]).is_err());
    assert!(parse_response(200, b"\xff", &[]).is_err());
}

#[test]
fn response_size_limit_rejects_otherwise_valid_envelopes_and_padded_plans() {
    let mut response: Value = serde_json::from_slice(&plan_response(basic_plan())).unwrap();
    response["unused_api_metadata"] = json!("a".repeat(65536));
    assert!(parse_response(200, &serde_json::to_vec(&response).unwrap(), &functions()).is_err());
    response
        .as_object_mut()
        .unwrap()
        .remove("unused_api_metadata");
    response["content"][0]["text"] = json!(format!("{}{}", basic_plan(), " ".repeat(16384)));
    assert!(parse_response(200, &serde_json::to_vec(&response).unwrap(), &functions()).is_err());
}

#[test]
fn response_accepts_split_text_blocks_and_boundary_length_terms() {
    let term = "检".repeat(64);
    let plan = json!({"queries":[term],"scope":"document","period":"last_30_days","functions":[]});
    let text = plan.to_string();
    let midpoint = text.find("queries").unwrap();
    let response = json!({"content":[{"type":"text","text":&text[..midpoint]},
        {"type":"text","text":&text[midpoint..]}],"stop_reason":"end_turn"});
    let parsed = parse_response(200, &serde_json::to_vec(&response).unwrap(), &[]).unwrap();
    assert_eq!(parsed.queries, ["检".repeat(64)]);
    assert_eq!(parsed.scope, Some(Scope::Document));
    assert_eq!(parsed.period, Period::Last30Days);
}

#[test]
fn response_rejects_duplicate_plan_fields_and_returns_unique_trimmed_terms() {
    let malformed = br#"{"content":[{"type":"text","text":"{\"queries\":[\"safe\"],\"queries\":[\"replacement\"],\"scope\":null,\"period\":\"any\",\"functions\":[]}"}],"stop_reason":"end_turn"}"#;
    assert!(parse_response(200, malformed, &functions()).is_err());
    let plan = json!({"queries":[" 超时 ","超时","timeout"],"scope":null,"period":"any",
        "functions":["pause_capture","pause_capture"]});
    let parsed = parse_response(200, &plan_response(plan), &functions()).unwrap();
    assert_eq!(parsed.queries, ["超时", "timeout"]);
    assert_eq!(parsed.functions, ["pause_capture"]);
}

#[test]
fn status_errors_are_useful_without_echoing_untrusted_server_body() {
    for (status, useful) in [
        (401, "认证"),
        (404, "地址"),
        (429, "频"),
        (500, "服务"),
        (503, "服务"),
    ] {
        let error = parse_response(
            status,
            b"sk-sensitive response with raw filesystem path",
            &[],
        )
        .unwrap_err();
        assert!(error.contains(useful), "{status}: {error}");
        assert!(!error.contains("sensitive"));
        assert!(!error.contains("filesystem"));
    }
    assert!(parse_response(302, b"secret redirect", &[]).is_err());
    let error = parse_response(200, b"raw-sensitive-response", &[]).unwrap_err();
    assert!(!error.contains("raw-sensitive-response"));
}

#[test]
fn explicit_effort_uses_output_config_and_default_is_omitted() {
    for (effort, expected) in [
        (Effort::Default, None),
        (Effort::Low, Some("low")),
        (Effort::Medium, Some("medium")),
        (Effort::High, Some("high")),
        (Effort::XHigh, Some("xhigh")),
        (Effort::Max, Some("max")),
    ] {
        let config = AiSettings {
            effort,
            ..settings()
        };
        let request = prepare(&config, "fixture-key", "查找", Scope::All, &[]).unwrap();
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["output_config"]["effort"].as_str(), expected);
        if expected.is_none() {
            assert!(body.get("output_config").is_none());
        }
    }
}

#[test]
fn search_accepts_bounded_thinking_but_keeps_tool_and_size_rejection() {
    let mut response: Value = serde_json::from_slice(&plan_response(basic_plan())).unwrap();
    response["content"].as_array_mut().unwrap().insert(
        0,
        json!({"type":"thinking","thinking":"private reasoning","signature":"signature"}),
    );
    assert_eq!(
        parse_response(200, &serde_json::to_vec(&response).unwrap(), &[])
            .unwrap()
            .queries,
        ["连接超时", "connection timeout"]
    );
    response["content"][0]["thinking"] = json!("a".repeat(16385));
    assert!(parse_response(200, &serde_json::to_vec(&response).unwrap(), &[]).is_err());
}
