use quick_panel::{
    ai::{AiSettings, Effort},
    ai_models::ModelInfo,
    model::ItemKind,
    organizer::*,
    organizer_ui::*,
};

fn proposal() -> Proposal {
    Proposal {
        candidate: Candidate {
            source: Source::SavedItem { id: "one".into() },
            fingerprint: "fingerprint".into(),
            expected_version: "v1".into(),
            kind: ItemKind::Clipboard,
            title: "原文".into(),
            alias: String::new(),
            tags: vec![],
            context: "完整本地摘要".into(),
        },
        alias: Some("建议".into()),
        tags: vec!["资料".into()],
        reason: "上下文".into(),
    }
}

// Wrong field indexes would mix the effort choice with credential controls.
#[test]
fn ai_form_reads_manual_model_and_appended_effort_without_moving_key_fields() {
    let values = [
        "0",
        "https://example.com/v1/messages",
        " manual/model ",
        "1",
        "",
        "0",
        "4",
    ]
    .map(str::to_owned);
    let parsed = parse_ai_fields(&AiSettings::default(), &values).unwrap();
    assert_eq!(parsed.model, "manual/model");
    assert_eq!(parsed.effort, Effort::XHigh);
    assert_eq!(parsed.auth, quick_panel::ai::AuthMethod::Bearer);
    assert!(!parsed.enabled);
    assert!(parse_ai_fields(&AiSettings::default(), &values[..6]).is_err());
}

// Unknown capability must never be rejected just because a model isn't in the list.
#[test]
fn capability_guidance_only_rejects_service_reported_unsupported_effort() {
    let models = vec![ModelInfo {
        id: "known".into(),
        display_name: Some("Friendly label".into()),
        effort: Some(quick_panel::ai_models::EffortCapabilities {
            supported: Some(false),
            ..Default::default()
        }),
    }];
    assert!(effort_warning(&models, "known", Effort::Max).is_some());
    assert!(effort_warning(&models, "manual", Effort::Max).is_none());
    assert!(effort_warning(&models, "known", Effort::Default).is_none());
}

// Parsing should reject a mode typo and oversized local inputs before any actor/network work.
#[test]
fn organizer_form_is_validated_and_does_not_silently_enable() {
    let mut values = ["0", "1", "2", "60", "4", "按项目分类"].map(str::to_owned);
    let settings = parse_organizer_fields(&values).unwrap();
    assert!(!settings.enabled);
    assert_eq!(settings.mode, OrganizerMode::Automatic);
    assert_eq!(settings.scope, OrganizerScope::PathsOnly);
    values[3] = "29".into();
    assert!(parse_organizer_fields(&values).is_err());
    values[3] = "60".into();
    values[1] = "9".into();
    assert!(parse_organizer_fields(&values).is_err());
}

// Losing the pending snapshot ID or adopting any response would resurrect a closed page.
#[test]
fn snapshot_gate_keeps_one_request_and_preserves_selected_edits() {
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    assert!(ui.request_snapshot().is_none());
    assert!(!ui.adopt_snapshot(id + 1, Snapshot::default()));
    assert!(ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![proposal()],
            ..Default::default()
        }
    ));
    ui.select(Some(0));
    ui.edit("人工名称".into(), "人工, 标签".into());
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![proposal()],
            queued: 3,
            ..Default::default()
        },
    );
    assert_eq!(
        ui.editor_text().unwrap(),
        ("人工名称".into(), "人工, 标签".into())
    );
    ui.close();
    assert!(!ui.adopt_snapshot(id, Snapshot::default()));
}

// Disappearing Ready or an unrelated receipt must never turn a submission into durable success.
#[test]
fn review_requires_correlated_receipt_and_preserves_failed_submission() {
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![proposal()],
            ..Default::default()
        },
    );
    ui.check(0, true);
    let (decision_id, _) = ui.begin_decision(false).unwrap();
    assert!(ui.begin_decision(false).is_err());
    let request = ui.request_snapshot().unwrap();
    assert!(!ui.adopt_snapshot(request, Snapshot::default()));
    assert_eq!(ui.snapshot.proposals.len(), 1);
    assert!(!ui.receipt(decision_id + 1, Ok(()), false));
    assert!(ui.pending());
    assert!(ui.receipt(decision_id, Err("写入失败".into()), true));
    assert_eq!(ui.snapshot.proposals.len(), 1);
    assert!(ui.needs_save());
    let (save_id, decision) = ui.begin_decision(false).unwrap();
    assert!(matches!(decision, ReviewDecision::SavePending));
    ui.receipt(save_id, Ok(()), false);
    assert!(!ui.needs_save());
    assert!(ui.message.contains("已保存"));
}

// A closed review keeps its one outstanding local read instead of issuing a second read on reopen.
#[test]
fn closing_does_not_release_an_outstanding_snapshot_slot() {
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.close();
    assert!(ui.request_snapshot().is_none());
    assert!(ui.adopt_snapshot(id, Snapshot::default()));
    assert!(ui.request_snapshot().is_some());
}

// A changed candidate version must not silently retarget an edit to the new source version.
#[test]
fn changed_source_snapshot_preserves_draft_and_original_version_for_stale_rejection() {
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![proposal()],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    ui.edit("编辑".into(), "".into());
    ui.check(0, true);
    let mut changed = proposal();
    changed.candidate.expected_version = "v2".into();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![changed],
            ..Default::default()
        },
    );
    assert!(ui.begin_decision(false).unwrap_err().contains("变化"));
    assert_eq!(ui.editor_text().unwrap().0, "编辑");
}

// Empty metadata or a blank endpoint must be reported locally when organizer opt-in is enabled.
#[test]
fn organizer_configuration_checks_are_independent_of_search_opt_in() {
    let mut ai = AiSettings {
        model: "manual".into(),
        encrypted_key: vec![1],
        ..Default::default()
    };
    assert!(organizer_config_warning(&ai).is_none());
    assert!(!ai.enabled);
    ai.endpoint.clear();
    assert!(organizer_config_warning(&ai).is_some());
}

// Fitting a tall form must keep actual DPI typography and expose the remainder through a scroll viewport.
#[test]
fn tall_native_forms_fit_monitor_without_reducing_dpi_or_hiding_fields() {
    for dpi in [96, 144, 192] {
        let fit = fit_form(1024, 768, dpi, 634);
        assert!(fit.width <= 1000 && fit.height <= 744);
        assert_eq!(fit.content_height, 554 * dpi as i32 / 96);
        assert_eq!(fit.viewport_height, fit.height - 80 * dpi as i32 / 96);
        assert_eq!(
            clamp_scroll(i32::MAX, fit.content_height, fit.viewport_height),
            (fit.content_height - fit.viewport_height).max(0)
        );
    }
}

// A late path result must never be attached to another selection, nor release a slot before its real local receipt.
#[test]
fn detail_gate_discards_old_selection_and_closed_page_receipts() {
    let mut a = proposal();
    a.candidate.kind = ItemKind::Folder;
    let mut b = a.clone();
    b.candidate.source = Source::SavedItem { id: "two".into() };
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![a, b],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    let (id, _, _) = ui.request_detail().unwrap();
    ui.select(Some(1));
    assert!(ui.request_detail().is_none());
    assert!(!ui.adopt_detail(id, Ok("C:/old".into())));
    assert!(ui.detail_target().is_none());
    let (id, _, _) = ui.request_detail().unwrap();
    ui.close();
    ui.open();
    assert!(!ui.adopt_detail(id, Ok("C:/new".into())));
    assert!(ui.detail_target().is_none());
}

// A source error must keep the edited row until the user explicitly refreshes local suggestions.
#[test]
fn stale_receipt_keeps_edits_until_an_explicit_local_refresh() {
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![proposal()],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    ui.check(0, true);
    ui.edit("保留编辑".into(), "".into());
    let (id, _) = ui.begin_decision(false).unwrap();
    ui.receipt(id, Err("来源已变化".into()), false);
    let id = ui.request_snapshot().unwrap();
    assert!(!ui.adopt_snapshot(id, Snapshot::default()));
    assert_eq!(ui.editor_text().unwrap().0, "保留编辑");
    ui.refresh();
    let id = ui.request_snapshot().unwrap();
    assert!(ui.adopt_snapshot(id, Snapshot::default()));
    assert!(ui.snapshot.proposals.is_empty());
}

fn unselected_draft_survives(dismiss: bool, retry: bool) {
    let a = proposal();
    let mut b = proposal();
    b.candidate.source = Source::SavedItem { id: "two".into() };
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![a.clone(), b],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    ui.edit("人工 A".into(), "草稿标签".into());
    ui.select(Some(1));
    ui.check(1, true);
    let (id, _) = ui.begin_decision(dismiss).unwrap();
    if retry {
        ui.receipt(id, Err("磁盘失败".into()), true);
        ui.select(Some(0));
        ui.check(0, true);
        let (id, decision) = ui.begin_decision(dismiss).unwrap();
        assert!(matches!(decision, ReviewDecision::SavePending));
        ui.receipt(id, Ok(()), false);
    } else {
        ui.receipt(id, Ok(()), false);
    }
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![a.clone()],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    if retry {
        assert!(
            ui.checked(0),
            "SavePending discarded an unrelated checkbox selection"
        );
    }
    assert_eq!(
        ui.editor_text().unwrap(),
        ("人工 A".into(), "草稿标签".into())
    );
    let mut changed = a;
    changed.candidate.expected_version = "v2".into();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![changed],
            ..Default::default()
        },
    );
    ui.check(0, true);
    assert!(
        ui.begin_decision(false).unwrap_err().contains("变化"),
        "unsubmitted draft lost its version guard"
    );
}
// Saving/ignoring B must not erase A's draft or its source version.
#[test]
fn successful_selected_receipt_keeps_unselected_draft_and_version() {
    for dismiss in [false, true] {
        unselected_draft_survives(dismiss, false);
    }
}
// A SavePending barrier must use the original submission set, not the current selection or all drafts.
#[test]
fn failed_selected_receipt_and_save_pending_keep_unselected_draft_and_version() {
    for dismiss in [false, true] {
        unselected_draft_survives(dismiss, true);
    }
}

// Local snapshot/detail refresh must preserve the raw delimiter/whitespace; normalization belongs only to a submitted proposal.
#[test]
fn editor_preserves_raw_tail_delimiter_and_whitespace_until_submission() {
    let mut p = proposal();
    p.candidate.kind = ItemKind::Folder;
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![p.clone()],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    ui.edit(" 名称  ".into(), " 项目, ".into());
    assert_eq!(
        ui.editor_text().unwrap(),
        (" 名称  ".into(), " 项目, ".into())
    );
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![p],
            queued: 4,
            ..Default::default()
        },
    );
    let (id, _, _) = ui.request_detail().unwrap();
    assert!(ui.adopt_detail(id, Ok("C:/原路径".into())));
    assert_eq!(
        ui.editor_text().unwrap(),
        (" 名称  ".into(), " 项目, ".into())
    );
    ui.check(0, true);
    let (_, decision) = ui.begin_decision(false).unwrap();
    let ReviewDecision::Apply(proposals) = decision else {
        panic!("wrong decision")
    };
    assert_eq!(proposals[0].alias.as_deref(), Some("名称"));
    assert_eq!(proposals[0].tags, vec!["项目"]);
    assert_eq!(ui.editor_text().unwrap().1, " 项目, ");
}

// The native editor's production gate must skip same-source refreshes and postpone necessary replacements until IME composition ends.
#[test]
fn editor_refresh_gate_preserves_same_identity_and_defers_ime_replacement() {
    let a = proposal().candidate;
    let mut b = a.clone();
    b.source = Source::SavedItem { id: "two".into() };
    let mut gate = EditorRefresh::default();
    assert!(gate.replace(Some(&a), false));
    assert!(!gate.replace(Some(&a), false));
    assert!(!gate.replace(Some(&a), true));
    assert!(!gate.replace(Some(&b), true));
    assert!(gate.deferred());
    assert!(gate.replace(Some(&b), false));
    assert!(!gate.deferred());
    assert!(!gate.replace(Some(&b), false));
    b.expected_version = "v2".into();
    assert!(!gate.replace(Some(&b), true));
    assert!(gate.replace(Some(&b), false));
    assert!(!gate.replace(None, true));
    assert!(gate.replace(None, false));
}

// Composition finishing after a source-version refresh still belongs to the editor's displayed version.
#[test]
fn deferred_editor_input_does_not_rebind_an_old_draft_to_a_new_version() {
    let mut p = proposal();
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![p.clone()],
            ..Default::default()
        },
    );
    ui.select(Some(0));
    let source = p.candidate.source.clone();
    p.candidate.expected_version = "v2".into();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![p],
            ..Default::default()
        },
    );
    ui.edit_for(source, "v1".into(), " 组合输入 ".into(), "项目,".into());
    assert_eq!(ui.editor_text().unwrap().1, "项目,");
    ui.check(0, true);
    assert!(ui.begin_decision(false).unwrap_err().contains("变化"));
}

// IME is active between Start/End even before GCS_COMPSTR contains its first character.
#[test]
fn editor_refresh_defers_empty_ime_composition_until_all_editors_end() {
    let a = proposal().candidate;
    let mut b = a.clone();
    b.expected_version = "v2".into();
    let mut gate = EditorRefresh::default();
    assert!(gate.replace(Some(&a), false));
    gate.composition(0, true);
    assert!(!gate.replace(Some(&b), false));
    gate.composition(1, true);
    gate.composition(0, false);
    assert!(!gate.replace(Some(&b), false));
    gate.composition(1, false);
    assert!(gate.replace(Some(&b), false));
}
