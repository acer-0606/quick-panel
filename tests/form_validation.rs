use quick_panel::{
    ai::{AiSettings, Effort},
    ai_models::{EffortCapabilities, ModelInfo},
    organizer_ui::validate_ai_form,
};
#[test]
fn disabling_ai_does_not_require_a_supported_effort() {
    let saved = AiSettings {
        enabled: true,
        encrypted_key: vec![1],
        ..Default::default()
    };
    let mut fields = [
        "0",
        "https://example.com/v1/messages",
        "known",
        "0",
        "",
        "0",
        "5",
    ]
    .map(str::to_owned);
    let models = vec![ModelInfo {
        id: "known".into(),
        display_name: None,
        effort: Some(EffortCapabilities {
            supported: Some(false),
            ..Default::default()
        }),
    }];
    let next = validate_ai_form(&saved, &fields, &models).unwrap();
    assert!(!next.enabled);
    assert_eq!(next.effort, Effort::Max);
    assert_eq!(next.encrypted_key, saved.encrypted_key);
    fields[0] = "1".into();
    assert!(validate_ai_form(&saved, &fields, &models).is_err());
}

use quick_panel::form_validation::{FormError, FormStatus, Submission, Validation, validate};
#[test]
fn local_errors_identify_fields_without_normalizing_the_draft() {
    for (kind, fields, bad) in [
        (
            Validation::General,
            vec!["0", "  Ctrl+  ", "Alt+C", "0", "", "0"],
            1,
        ),
        (
            Validation::Paths,
            vec!["1", "0", "  relative/path  ", "0"],
            2,
        ),
        (Validation::Favorite, vec!["  ", "C:\\a", "", "", ""], 0),
        (
            Validation::Embedding(Default::default()),
            vec!["1", "ftp://bad", "model", "synthetic-key", "0", "0"],
            1,
        ),
    ] {
        let fields: Vec<String> = fields.into_iter().map(str::to_owned).collect();
        let raw = fields.clone();
        let error = validate(&kind, &fields, &[]).unwrap_err();
        assert_eq!(error.field, bad);
        assert_eq!(fields, raw);
    }
}
#[test]
fn runtime_poll_keeps_config_and_save_error_visible() {
    let mut status = FormStatus {
        config: "缺少API与路径依赖".into(),
        error: Some(FormError::new(3, "间隔无效")),
        test: String::new(),
    };
    status.runtime_health("Pi就绪".into());
    assert!(status.text().contains("缺少API与路径依赖"));
    assert!(status.text().contains("间隔无效"));
    assert!(status.text().contains("Pi就绪"));
}
#[test]
fn failed_native_submit_keeps_the_same_raw_draft_open_for_retry() {
    let fields = vec!["  Ctrl+Alt+K  ".to_owned(), "原始草稿".into()];
    let mut submission = Submission::default();
    submission.attempt(&fields, |raw| {
        assert_eq!(raw[0], "  Ctrl+Alt+K  ");
        Err(FormError::new(0, "OS注册失败"))
    });
    assert!(!submission.accepted);
    assert_eq!(submission.error.as_ref().unwrap().field, 0);
    submission.attempt(&fields, |raw| {
        assert_eq!(raw[0], "  Ctrl+Alt+K  ");
        assert_eq!(raw[1], "原始草稿");
        Ok(())
    });
    assert!(submission.accepted);
    assert!(submission.error.is_none());
}

#[test]
fn embedding_rejects_each_invalid_credential_field_and_preserves_existing_key() {
    use quick_panel::{form_validation::parse_embedding_fields, semantic::EmbeddingSettings};
    let saved = EmbeddingSettings {
        encrypted_key: vec![7, 8],
        ..Default::default()
    };
    let raw = ["1", " https://example.com/v1 ", " model ", "", "0", "1"].map(str::to_owned);
    let next = parse_embedding_fields(&saved, &raw).unwrap();
    assert!(next.enabled);
    assert_eq!(next.model, "model");
    assert_eq!(next.encrypted_key, vec![7, 8]);
    for (field, bad) in [
        (1, ""),
        (2, ""),
        (2, "model\nname"),
        (3, "bad key"),
        (5, "9"),
    ] {
        let mut draft = raw.clone();
        draft[field] = bad.into();
        assert_eq!(
            parse_embedding_fields(&saved, &draft).unwrap_err().field,
            field
        );
        assert_eq!(draft[field], bad);
    }
    let mut clear = raw.clone();
    clear[4] = "1".into();
    clear[3] = "synthetic-key".into();
    assert_eq!(parse_embedding_fields(&saved, &clear).unwrap_err().field, 4);
    clear[3].clear();
    let next = parse_embedding_fields(&saved, &clear).unwrap();
    assert!(!next.enabled);
    assert!(next.encrypted_key.is_empty());
    assert_eq!(
        parse_embedding_fields(&EmbeddingSettings::default(), &raw)
            .unwrap_err()
            .field,
        3
    );
    let mut fresh = raw.clone();
    fresh[3] = "synthetic-key".into();
    assert!(parse_embedding_fields(&EmbeddingSettings::default(), &fresh).is_ok());
}

#[test]
fn accepted_submission_cannot_commit_twice() {
    let mut submission = Submission::default();
    let mut commits = 0;
    for _ in 0..2 {
        submission.attempt(&[], |_| {
            commits += 1;
            Ok(())
        });
    }
    assert_eq!(commits, 1);
}

#[test]
fn disabled_ai_still_identifies_malformed_local_fields() {
    let saved = AiSettings::default();
    let raw = ["0", "https://example.com", "known", "0", "", "0", "5"].map(str::to_owned);
    for (field, bad) in [
        (1, "ftp://bad"),
        (2, "bad\nmodel"),
        (3, "9"),
        (4, "bad key"),
        (6, "9"),
    ] {
        let mut fields = raw.clone();
        fields[field] = bad.into();
        assert_eq!(
            validate(&Validation::Ai(saved.clone()), &fields, &[])
                .unwrap_err()
                .field,
            field
        );
    }
}

#[test]
fn metadata_validation_preserves_raw_fields_and_reports_the_invalid_editor() {
    use quick_panel::model::{ItemKind, ItemSummary};
    let summary = ItemSummary {
        id: "synthetic".into(),
        kind: ItemKind::Clipboard,
        title: "saved".into(),
        subtitle: String::new(),
        alias: String::new(),
        tags: vec![],
        pinned: true,
        created_at_ms: 0,
        last_used_ms: 0,
        use_count: 0,
    };
    let kind = Validation::Metadata(summary);
    let fields = ["  title  ", " alias ", " first, second, "].map(str::to_owned);
    assert!(validate(&kind, &fields, &[]).is_ok());
    for (index, invalid) in [
        (0, " ".to_owned()),
        (1, "x".repeat(129)),
        (2, "x".repeat(65)),
    ] {
        let mut raw = fields.clone();
        raw[index] = invalid.clone();
        assert_eq!(validate(&kind, &raw, &[]).unwrap_err().field, index);
        assert_eq!(raw[index], invalid);
        assert_eq!(raw[(index + 1) % 3], fields[(index + 1) % 3]);
    }
}

#[test]
fn unsupported_effort_test_reports_into_the_same_form_after_releasing_model_borrow() {
    use quick_panel::form_validation::report_effort_warning;
    use std::cell::RefCell;
    let form = RefCell::new((
        vec![ModelInfo {
            id: "known".into(),
            display_name: None,
            effort: Some(EffortCapabilities {
                supported: Some(false),
                ..Default::default()
            }),
        }],
        FormStatus {
            config: "保留配置依赖".into(),
            error: Some(FormError::new(4, "保留保存错误")),
            test: String::new(),
        },
        "  原始草稿  ".to_owned(),
    ));
    let rejected = report_effort_warning(
        &form,
        |form| &form.0,
        "known",
        Effort::Max,
        |message| form.borrow_mut().1.runtime_health(message.to_owned()),
    );
    assert!(rejected);
    {
        let form = form.borrow();
        assert!(form.1.test.contains("不支持"));
        assert_eq!(form.1.config, "保留配置依赖");
        assert_eq!(form.1.error.as_ref().unwrap().field, 4);
        assert_eq!(form.2, "  原始草稿  ");
    }
    assert!(!report_effort_warning(
        &form,
        |form| &form.0,
        "known",
        Effort::Default,
        |_| panic!("默认强度不应报告错误")
    ));
    assert!(!report_effort_warning(
        &form,
        |form| &form.0,
        "unknown",
        Effort::Max,
        |_| panic!("未知能力不应阻断测试")
    ));
}
