use quick_panel::{embedding, model::*, semantic::*};

fn settings() -> EmbeddingSettings {
    EmbeddingSettings {
        enabled: true,
        model: "fixture-model".into(),
        encrypted_key: vec![7, 8, 9],
        ..Default::default()
    }
}
fn item(kind: ItemKind) -> StoredItem {
    StoredItem {
        summary: ItemSummary {
            id: "saved:one".into(),
            kind,
            title: "标题".into(),
            subtitle: "ignored subtitle".into(),
            alias: "别名".into(),
            tags: vec!["标签".into()],
            pinned: true,
            created_at_ms: 1,
            last_used_ms: 2,
            use_count: 3,
        },
        text: "正文".into(),
        representations: vec![],
        target: Some("C:\\项目".into()),
        open_with: None,
    }
}

#[test]
fn legacy_settings_get_disabled_embedding_and_keys_are_redacted() {
    let old: Settings = serde_json::from_str(r#"{"theme":"Dark","max_history":123}"#).unwrap();
    assert!(!old.embedding.enabled);
    assert_eq!(old.embedding.endpoint, "https://api.openai.com");
    assert_eq!(old.embedding.index_scope, IndexScope::PinnedAndFavorites);
    assert_eq!(old.max_history, 123);
    assert!(old.embedding.model.is_empty());
    assert!(old.embedding.encrypted_key.is_empty());
    let mut config = settings();
    config.encrypted_key = b"private-key-marker".to_vec();
    let debug = format!("{config:?}");
    assert!(!debug.contains("private-key-marker"));
    assert!(!debug.contains("112, 114, 105"));
    assert_eq!(
        serde_json::from_str::<EmbeddingSettings>("{}").unwrap(),
        EmbeddingSettings::default()
    );
}

#[test]
fn endpoint_normalizes_root_version_full_proxy_and_ipv6() {
    for (address, host, port, secure, path) in [
        (
            "https://API.Example.com",
            "api.example.com",
            443,
            true,
            "/v1/embeddings",
        ),
        (
            "https://api.example.com/v1/",
            "api.example.com",
            443,
            true,
            "/v1/embeddings",
        ),
        (
            "http://localhost:8080/v1/embeddings",
            "localhost",
            8080,
            false,
            "/v1/embeddings",
        ),
        (
            "https://proxy.example.com/provider/",
            "proxy.example.com",
            443,
            true,
            "/provider/v1/embeddings",
        ),
        (
            "https://proxy.example.com/provider/v1/embeddings/",
            "proxy.example.com",
            443,
            true,
            "/provider/v1/embeddings",
        ),
        ("http://[::1]:8080/v1", "::1", 8080, false, "/v1/embeddings"),
    ] {
        let endpoint = embedding::parse_endpoint(address).unwrap();
        assert_eq!(
            (
                endpoint.host.as_str(),
                endpoint.port,
                endpoint.secure,
                endpoint.path.as_str()
            ),
            (host, port, secure, path)
        );
    }
}

#[test]
fn endpoint_rejects_credentials_parameters_injection_and_relative_paths() {
    for address in [
        "https://user:secret@api.example.com",
        "https://api.example.com?secret=x",
        "https://api.example.com/#x",
        "https://api.example.com/\r\nX:secret",
        "https://api.example.com/%0d%0a",
        "https://api.example.com/%2e%2e/a",
        "https://api.example.com/a/../b",
        "https://api.example.com/\\x",
        "https://api.example.com:0",
        "file:///tmp/x",
    ] {
        let error = embedding::parse_endpoint(address).unwrap_err();
        assert!(!error.contains("secret"));
    }
}

#[test]
fn request_uses_bearer_and_float_array_without_leaking_debug() {
    let request = embedding::prepare(
        &settings(),
        "fixture-secret",
        &["第一条".into(), "second".into()],
    )
    .unwrap();
    assert_eq!(request.endpoint.path, "/v1/embeddings");
    assert_eq!(
        request.headers,
        [
            ("Content-Type".into(), "application/json".into()),
            ("Authorization".into(), "Bearer fixture-secret".into())
        ]
    );
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"model":"fixture-model", "input":["第一条","second"],"encoding_format":"float"})
    );
    let debug = format!("{request:?}");
    assert!(!debug.contains("fixture-secret"));
    assert!(!debug.contains("第一条"));
}

#[test]
fn request_admission_bounds_inputs_models_keys_and_settings() {
    let mut disabled = settings();
    disabled.enabled = false;
    assert!(embedding::prepare(&disabled, "fixture", &["x".into()]).is_err());
    for inputs in [
        vec![],
        vec!["x".into(); 17],
        vec![" ".into()],
        vec!["中".repeat(4097)],
        vec!["x\0y".into()],
    ] {
        assert!(embedding::prepare(&settings(), "fixture", &inputs).is_err());
    }
    assert!(embedding::prepare(&settings(), "fixture", &vec!["中".repeat(4096); 16]).is_ok());
    for key in ["", "secret\r\nHeader: x", "secret key"] {
        assert!(embedding::prepare(&settings(), key, &["x".into()]).is_err());
    }
    for model in ["".into(), " ".into(), "x\0y".into(), "m".repeat(257)] {
        let mut config = settings();
        config.model = model;
        assert!(embedding::prepare(&config, "fixture", &["x".into()]).is_err());
    }
}

#[test]
fn response_reorders_by_index_and_accepts_success_status() {
    let reply = br#"{"data":[{"index":1,"embedding":[0,1]},{"index":0,"embedding":[1,0]}]}"#;
    assert_eq!(
        embedding::parse_response(200, reply, 2).unwrap(),
        vec![vec![1.0, 0.0], vec![0.0, 1.0]]
    );
    assert_eq!(
        embedding::parse_response(201, br#"{"data":[{"index":0,"embedding":[3,4]}]}"#, 1).unwrap(),
        vec![vec![3.0, 4.0]]
    );
}

#[test]
fn response_rejects_incomplete_indexes_and_invalid_vectors_without_body() {
    for reply in [
        r#"{"secret":"private-marker","data":[{"index":0,"embedding":[1,0]},{"index":0,"embedding":[0,1]}]}"#,
        r#"{"data":[{"index":0,"embedding":[1,0]}]}"#,
        r#"{"data":[{"index":0,"embedding":[1,0]},{"index":2,"embedding":[0,1]}]}"#,
        r#"{"data":[{"index":0,"embedding":[1,0]},{"index":1,"embedding":[1]}]}"#,
        r#"{"data":[{"index":0,"embedding":[0,0]},{"index":1,"embedding":[0,1]}]}"#,
        r#"{"data":[{"index":0,"embedding":[]},{"index":1,"embedding":[1]}]}"#,
        r#"{"data":[{"index":0,"embedding":[1e40]},{"index":1,"embedding":[1]}]}"#,
        r#"{"data":[{"index":0,"embedding":[NaN]},{"index":1,"embedding":[1]}]}"#,
        r#"{"data":[{"embedding":[1]},{"index":1,"embedding":[1]}]}"#,
        r#"{"data":[{"index":-1,"embedding":[1]},{"index":1,"embedding":[1]}]}"#,
    ] {
        assert!(
            !embedding::parse_response(200, reply.as_bytes(), 2)
                .unwrap_err()
                .contains("private-marker")
        );
    }
    for status in [301, 400, 401, 403, 429, 503] {
        assert!(
            !embedding::parse_response(status, b"private-marker", 1)
                .unwrap_err()
                .contains("private-marker")
        );
    }
    assert!(embedding::parse_response(200, &vec![b' '; 2 * 1024 * 1024 + 1], 1).is_err());
    assert!(embedding::parse_response(200, b"{}", 0).is_err());
    assert!(embedding::parse_response(200, b"{}", 17).is_err());
}

#[test]
fn profiles_normalize_addresses_and_ignore_credentials_enable_and_scope() {
    let config = settings();
    let profile = profile_id(&config).unwrap();
    assert_eq!(profile.len(), 64);
    let mut same = config.clone();
    same.endpoint = "https://API.OPENAI.COM:443/v1/embeddings/".into();
    same.enabled = false;
    same.encrypted_key = vec![99];
    same.index_scope = IndexScope::AllText;
    assert_eq!(profile_id(&same).unwrap(), profile);
    same.model = "another-model".into();
    assert_ne!(profile_id(&same).unwrap(), profile);
    same = config.clone();
    same.endpoint = "https://api.openai.com/proxy".into();
    assert_ne!(profile_id(&same).unwrap(), profile);
    same = config;
    same.model = "".into();
    assert!(profile_id(&same).is_err());
}

#[test]
fn candidates_include_saved_text_metadata_but_exclude_statistics_and_binary_images() {
    let mut text = item(ItemKind::Clipboard);
    text.target = None;
    let original = candidate(&text, IndexScope::PinnedAndFavorites).unwrap();
    assert_eq!(original.text, "标题\n别名\n标签\n正文");
    text.summary.use_count = 999;
    text.summary.last_used_ms = 999;
    text.summary.created_at_ms = 999;
    text.summary.pinned = false;
    text.summary.subtitle = "new presentation".into();
    assert_eq!(
        candidate(&text, IndexScope::AllText).unwrap().text_hash,
        original.text_hash
    );
    assert!(candidate(&text, IndexScope::PinnedAndFavorites).is_none());
    text.summary.alias = "新别名".into();
    assert_ne!(
        candidate(&text, IndexScope::AllText).unwrap().text_hash,
        original.text_hash
    );
    text.text.clear();
    text.representations = vec![Representation {
        format: "PNG".into(),
        data: vec![1, 2, 3],
    }];
    assert!(candidate(&text, IndexScope::AllText).is_none());
    let mut favorite = item(ItemKind::Document);
    favorite.summary.pinned = false;
    favorite.text = "not document contents".into();
    assert_eq!(
        candidate(&favorite, IndexScope::PinnedAndFavorites)
            .unwrap()
            .text,
        "标题\n别名\n标签\nC:\\项目"
    );
}

#[test]
fn candidate_bounds_unicode_before_hashing_and_skips_blank_text() {
    let mut text = item(ItemKind::Clipboard);
    text.text = "中".repeat(8000);
    let bounded = candidate(&text, IndexScope::AllText).unwrap();
    assert_eq!(bounded.text.chars().count(), 4096);
    text.text.push('尾');
    assert_eq!(
        candidate(&text, IndexScope::AllText).unwrap().text_hash,
        bounded.text_hash
    );
    text.text = " \n\t".into();
    assert!(candidate(&text, IndexScope::AllText).is_none());
}

#[test]
fn vector_validation_and_cosine_handle_bounds_nonfinite_and_large_coordinates() {
    assert_eq!(
        validate_vectors(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap(),
        2
    );
    for vectors in [
        vec![],
        vec![vec![]],
        vec![vec![0.0, 0.0]],
        vec![vec![f32::NAN]],
        vec![vec![f32::INFINITY]],
        vec![vec![1.0], vec![1.0, 2.0]],
        vec![vec![1.0; 4097]],
    ] {
        assert!(validate_vectors(&vectors).is_err());
    }
    assert_eq!(validate_vectors(&[vec![1.0; 4096]]).unwrap(), 4096);
    assert_eq!(cosine_distance(&[1.0, 0.0], &[0.0, 1.0]).unwrap(), 1.0);
    assert_eq!(cosine_distance(&[1.0, 0.0], &[-1.0, 0.0]).unwrap(), 2.0);
    assert_eq!(
        cosine_distance(&[f32::MAX, f32::MAX], &[f32::MAX, f32::MAX]).unwrap(),
        0.0
    );
    assert!(cosine_distance(&[1.0], &[1.0, 0.0]).is_err());
    assert!(cosine_distance(&[0.0], &[1.0]).is_err());
}
