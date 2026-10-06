use quick_panel::{
    ai::{AiSettings, Effort},
    ai_models::{self, ModelListing},
};
use serde_json::{Value, json};

fn page(value: Value) -> ai_models::ModelPage {
    ai_models::parse_page(200, &serde_json::to_vec(&value).unwrap()).unwrap()
}

#[test]
fn model_endpoint_replaces_messages_resource_and_keeps_proxy_prefix() {
    let settings = AiSettings {
        endpoint: "https://proxy.example/claude/v1/messages/".into(),
        ..AiSettings::default()
    };
    let request = ai_models::prepare(&settings, "entered-key", None).unwrap();
    assert_eq!(request.endpoint.path, "/claude/v1/models?limit=100");
    assert!(request.body.is_empty());
    assert!(
        request
            .headers
            .iter()
            .any(|(name, value)| name == "x-api-key" && value == "entered-key")
    );
    let next = ai_models::prepare(&settings, "entered-key", Some("model+a/b")).unwrap();
    assert_eq!(
        next.endpoint.path,
        "/claude/v1/models?limit=100&after_id=model%2Ba%2Fb"
    );
    for cursor in ["https://evil.example", "bad\nheader", "", "带空格"] {
        assert!(ai_models::prepare(&settings, "entered-key", Some(cursor)).is_err());
    }
    for key in ["", "invalid key", "key\r\nheader", "秘密"] {
        assert!(ai_models::prepare(&settings, key, None).is_err());
    }
}

#[test]
fn official_capabilities_remain_known_and_gateway_missing_capabilities_remain_unknown() {
    let result = page(json!({"data":[
        {"id":"claude-fixture","display_name":"Claude fixture","type":"model","created_at":"2026-10-06",
         "capabilities":{"effort":{"supported":true,"low":{"supported":true},"medium":{"supported":true},"high":{"supported":true},"xhigh":{"supported":false},"max":{"supported":false}}}},
        {"id":"gateway-model"}],"has_more":false,"first_id":"claude-fixture","last_id":"gateway-model"}));
    assert_eq!(
        result.models[0].display_name.as_deref(),
        Some("Claude fixture")
    );
    assert_eq!(result.models[0].supports_effort(Effort::High), Some(true));
    assert_eq!(result.models[0].supports_effort(Effort::XHigh), Some(false));
    assert_eq!(result.models[1].supports_effort(Effort::High), None);
    assert_eq!(result.models[0].supports_effort(Effort::Default), None);
    assert!(result.next_cursor.is_none());
    let no_effort =
        page(json!({"data":[{"id":"plain","capabilities":{"effort":{"supported":false}}}]}));
    assert_eq!(
        no_effort.models[0].supports_effort(Effort::Low),
        Some(false)
    );
}

#[test]
fn parser_rejects_malformed_duplicate_models_bad_cursors_and_oversized_pages() {
    for value in [
        json!({"data":[{"id":""}]}),
        json!({"data":[{"id":"bad id"}]}),
        json!({"data":[{"id":"model\u{202e}evil"}]}),
        json!({"data":[{"id":"https://evil.example"}]}),
        json!({"data":[{"id":"x"},{"id":"x"}]}),
        json!({"data":[{"id":"x"}],"has_more":true}),
        json!({"data":[],"has_more":true,"last_id":"x"}),
        json!({"data":[{"id":"x"}],"has_more":true,"last_id":"other"}),
        json!({"data":[{"id":"x","capabilities":{"effort":{"high":{"supported":"yes"}}}}]}),
    ] {
        assert!(
            ai_models::parse_page(200, &serde_json::to_vec(&value).unwrap()).is_err(),
            "{value}"
        );
    }
    assert!(ai_models::parse_page(200, &vec![b' '; 262145]).is_err());
    assert!(
        !ai_models::parse_page(403, b"private server key")
            .unwrap_err()
            .contains("private")
    );
}

#[test]
fn listing_bounds_pages_models_and_cross_page_duplicates_without_partial_success() {
    let mut list = ModelListing::default();
    assert_eq!(
        list.push_page(page(
            json!({"data":[{"id":"a"}],"has_more":true,"last_id":"a"})
        ))
        .unwrap(),
        Some("a".into())
    );
    assert!(list.push_page(page(json!({"data":[{"id":"a"}]}))).is_err());
    let mut list = ModelListing::default();
    for index in 0..4 {
        let id = format!("model-{index}");
        list.push_page(page(
            json!({"data":[{"id":id}],"has_more":true,"last_id":id}),
        ))
        .unwrap();
    }
    assert!(
        list.push_page(page(
            json!({"data":[{"id":"fifth"}],"has_more":true,"last_id":"fifth"})
        ))
        .is_err()
    );
    let models = (0..200)
        .map(|index| json!({"id":format!("m-{index}")}))
        .collect::<Vec<_>>();
    let mut list = ModelListing::default();
    list.push_page(page(json!({"data":models}))).unwrap();
    assert_eq!(list.models().len(), 200);
    assert!(
        list.push_page(page(json!({"data":[{"id":"overflow"}]})))
            .is_err()
    );
}
