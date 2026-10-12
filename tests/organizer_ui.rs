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

// A successful decision must not hide why background work later stops.
#[test]
fn successful_receipt_keeps_live_paused_cooldown_capacity_and_queue_visible() {
    for (snapshot, expected) in [
        (
            Snapshot {
                paused: true,
                ..Default::default()
            },
            "已暂停",
        ),
        (
            Snapshot {
                failures: 1,
                retry_at_ms: Some(99),
                ..Default::default()
            },
            "冷却",
        ),
        (
            Snapshot {
                saturated: true,
                ..Default::default()
            },
            "容量已满",
        ),
        (
            Snapshot {
                queued: 7,
                ..Default::default()
            },
            "待处理 7",
        ),
        (
            Snapshot {
                in_flight: true,
                queued: 3,
                ..Default::default()
            },
            "后台正在整理",
        ),
    ] {
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
        let (id, _) = ui.begin_decision(false).unwrap();
        ui.receipt(id, Ok(()), false);
        let id = ui.request_snapshot().unwrap();
        assert!(ui.adopt_snapshot(id, snapshot));
        assert!(
            ui.status(true).contains(expected),
            "live state hidden: {}",
            ui.status(true)
        );
    }
}

// Background status must not obscure an uncommitted save or release its draft guard.
#[test]
fn pending_save_failure_stays_visible_and_keeps_raw_drafts() {
    let mut ui = ReviewState::default();
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![proposal()],
            paused: true,
            ..Default::default()
        },
    );
    ui.select(Some(0));
    ui.edit(" 草稿 ".into(), "标签, ".into());
    ui.check(0, true);
    let (id, _) = ui.begin_decision(false).unwrap();
    ui.receipt(id, Err("磁盘故障".into()), true);
    assert!(ui.status(true).contains("未保存：磁盘故障"));
    assert!(ui.status(true).contains("重试保存"));
    let id = ui.request_snapshot().unwrap();
    assert!(!ui.adopt_snapshot(id, Snapshot::default()));
    assert!(ui.needs_save());
    assert_eq!(
        ui.editor_text().unwrap(),
        (" 草稿 ".into(), "标签, ".into())
    );
}

// A detail/count-only revision must leave a scrolled ListView's rows intact.
#[test]
fn list_refresh_keeps_rows_for_detail_status_and_checkbox_updates() {
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
    ui.edit(" 原稿 ".into(), "项目, ".into());
    let mut list = ListRefresh::default();
    assert!(list.replace(&ui.list_rows(), 0, 0).rebuild);
    let id = ui.request_snapshot().unwrap();
    ui.adopt_snapshot(
        id,
        Snapshot {
            proposals: vec![p],
            queued: 7,
            ..Default::default()
        },
    );
    ui.check(0, true);
    assert!(!list.replace(&ui.list_rows(), 0, -3).rebuild);
    let (id, _, _) = ui.request_detail().unwrap();
    assert!(ui.adopt_detail(id, Ok("C:/完整路径".into())));
    assert!(!list.replace(&ui.list_rows(), 0, -3).rebuild);
    assert_eq!(ui.selected_index(), Some(0));
    assert!(ui.checked(0));
    assert_eq!(
        ui.editor_text().unwrap(),
        (" 原稿 ".into(), "项目, ".into())
    );
}

// Rebuilding must anchor the stable source, or the next/previous survivor when it is removed.
#[test]
fn list_refresh_restores_source_anchor_and_pixel_offset_after_insertions_or_deletions() {
    let row = |id: &str| ReviewRow {
        source: Source::SavedItem { id: id.into() },
        title: id.into(),
        alias: String::new(),
        tags: String::new(),
    };
    let mut list = ListRefresh::default();
    list.replace(&[row("a"), row("b"), row("c"), row("d")], 0, 0);
    let update = list.replace(&[row("x"), row("a"), row("b"), row("c"), row("d")], 2, -6);
    assert!(update.rebuild);
    assert_eq!(update.scroll, Some((3, -6)));
    let update = list.replace(&[row("x"), row("a"), row("b"), row("d")], 3, -6);
    assert_eq!(update.scroll, Some((3, -6)));
    let update = list.replace(&[row("x"), row("a"), row("b")], 3, -6);
    assert_eq!(update.scroll, Some((2, -6)));
    let mut rows = vec![row("x"), row("a"), row("b")];
    rows[2].alias = "修改后的显示别名".into();
    assert_eq!(list.replace(&rows, 2, -2).scroll, Some((2, -2)));
    assert_eq!(list.replace(&[], 2, -2).scroll, None);
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
    let mut values = ["0", "1", "2", "60", "4", "按项目分类", "1", "4"].map(str::to_owned);
    let settings = parse_organizer_fields(&values).unwrap();
    assert!(!settings.enabled);
    assert_eq!(settings.mode, OrganizerMode::Automatic);
    assert_eq!(settings.scope, OrganizerScope::PathsOnly);
    values[6] = "0".into();
    values[7] = "7".into();
    let changed = parse_organizer_fields(&values).unwrap();
    assert!(!changed.memory_enabled);
    assert_eq!(changed.max_rounds, 7);
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
    ai.model = "m".repeat(257);
    assert!(organizer_config_warning(&ai).is_some());
    ai.model = "manual".into();
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

#[test]
fn memory_receipt_keeps_same_input_retry_and_rejects_stale_receipts() {
    use quick_panel::{
        harness_tools::{MemoryEdit, MemoryScope},
        model::Command,
    };
    let mut state = MemoryUiState::default();
    let id = memory_request_id();
    let command = Command::HarnessMemoryEdit {
        request_id: id,
        input: MemoryEdit {
            id: Some(5),
            scope: MemoryScope::Global,
            text: "保留项目名".into(),
            enabled: true,
            confirmed: true,
        },
    };
    state.submit(command).unwrap();
    assert!(!state.receipt(id + 1, Ok(()), false));
    assert!(state.saving);
    assert!(state.receipt(id, Err("磁盘不可写".into()), true));
    assert!(!state.message.contains("已保存"));
    let Command::HarnessMemoryEdit { request_id, input } = state.retry().unwrap() else {
        panic!()
    };
    assert_eq!(request_id, id);
    assert_eq!(input.text, "保留项目名");
    assert!(state.receipt(id, Ok(()), false));
    assert!(state.pending.is_none());
    let independent: std::collections::HashSet<_> = (0..256).map(|_| memory_request_id()).collect();
    assert_eq!(independent.len(), 256);
}
#[test]
fn memory_multiline_input_is_explicitly_normalized_without_losing_raw_draft() {
    let raw = "  保留项目名\r\n\t不要用日期  ";
    assert_eq!(memory_text(raw), "保留项目名   不要用日期");
    assert_eq!(raw, "  保留项目名\r\n\t不要用日期  ");
}
#[test]
fn feedback_captures_displayed_version_and_alias_before_new_snapshot() {
    let mut snapshot = Snapshot {
        proposals: vec![proposal()],
        ..Default::default()
    };
    let displayed = displayed_feedback(&snapshot);
    let old_alias = snapshot.proposals[0].alias.clone();
    let old_version = snapshot.proposals[0].candidate.expected_version.clone();
    snapshot.proposals[0].alias = Some("新建议".into());
    snapshot.proposals[0].candidate.expected_version = "new-version".into();
    assert_eq!(displayed[0].1.rejected_alias, old_alias);
    assert_eq!(displayed[0].1.expected_version, old_version);
    assert_ne!(displayed, displayed_feedback(&snapshot));
}
#[test]
fn attention_rows_remain_non_applicable_and_status_explains_local_recovery() {
    let mut state = ReviewState::default();
    state
        .snapshot
        .attention
        .push(quick_panel::harness_tools::Attention {
            title: "合成来源".into(),
            source: Source::SavedItem {
                id: "synthetic".into(),
            },
            expected_version: "seen".into(),
            code: "round_limit".into(),
            message: "已达到模型轮次上限".into(),
        });
    state.select(Some(0));
    state.check(0, true);
    assert!(state.selected().is_none());
    assert!(state.begin_decision(false).is_err());
    assert_eq!(state.feedback_target().unwrap().1.rejected_alias, None);
    assert!(state.status(true).contains("需处理"));
    let detail = state.attention_detail().unwrap();
    for part in [
        "合成来源",
        "synthetic",
        "round_limit",
        "已达到模型轮次上限",
        "重试",
        "反馈并重做",
    ] {
        assert!(detail.contains(part), "{detail}");
    }
}
#[test]
fn memory_small_screen_layout_scrolls_every_field_without_overlap_or_footer_obstruction() {
    for dpi in [96, 144, 192] {
        for (width, height) in [(1000, 620), (800, 500), (640, 360)] {
            for feedback in [false, true] {
                let layout = memory_layout(width, height, dpi, feedback);
                for (index, field) in layout
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.height > 0)
                {
                    assert!(
                        field.x >= 0 && field.right() <= width,
                        "horizontal bounds dpi={dpi} field={index}"
                    );
                    for other in layout
                        .fields
                        .iter()
                        .skip(index + 1)
                        .filter(|r| r.height > 0)
                    {
                        assert!(
                            field.right() <= other.x
                                || other.right() <= field.x
                                || field.bottom() <= other.y
                                || other.bottom() <= field.y,
                            "content overlap dpi={dpi} width={width} height={height}: {field:?} / {other:?}"
                        );
                    }
                    let scroll = memory_focus_scroll(&layout, index, 0);
                    assert!(
                        field.y - scroll >= 0 && field.bottom() - scroll <= layout.viewport_height,
                        "field cannot be revealed: dpi={dpi} field={index}"
                    );
                }
                for (i, button) in layout.buttons.iter().enumerate() {
                    assert!(
                        button.y >= layout.viewport_height
                            && button.bottom() <= height
                            && button.right() <= width
                            && button.x >= 0,
                        "footer must stay outside viewport"
                    );
                    for other in layout.buttons.iter().skip(i + 1) {
                        assert!(
                            button.right() <= other.x
                                || other.right() <= button.x
                                || button.bottom() <= other.y
                                || other.bottom() <= button.y,
                            "footer overlap"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn memory_focus_scroll_reveals_reverse_tab_targets_and_clamps_after_resize() {
    for dpi in [96, 144, 192] {
        let layout = memory_layout(640, 360, dpi, false);
        let mut position = layout.content_height;
        for index in (0..7).rev() {
            position = memory_focus_scroll(&layout, index, position);
            let field = layout.fields[index];
            assert!(
                position >= 0
                    && position <= (layout.content_height - layout.viewport_height).max(0)
            );
            assert!(field.y - position >= 0 && field.bottom() - position <= layout.viewport_height);
        }
        let larger = memory_layout(1200, 900, dpi, false);
        let position = memory_focus_scroll(&larger, 6, layout.content_height);
        assert!(position <= (larger.content_height - larger.viewport_height).max(0));
    }
}

#[test]
fn memory_save_retry_does_not_cancel_a_new_session() {
    use quick_panel::model::Command;
    for success in [false, true] {
        let mut state = MemoryUiState::default();
        let mut session = OrganizerSession::default();
        session.begin(41, 41);
        state
            .submit(Command::HarnessMemoryDelete {
                request_id: 7,
                ids: vec![1],
            })
            .unwrap();
        state.bind_work(&session);
        assert_eq!(
            state.receipt_for_session(8, Ok(()), false, &mut session),
            None
        );
        assert_eq!(
            state.receipt_for_session(7, Err("参数".into()), false, &mut session),
            Some(false)
        );
        state.retry().unwrap();
        state.bind_work(&session);
        assert_eq!(
            state.receipt_for_session(7, Err("磁盘".into()), true, &mut session),
            Some(true)
        );
        session.begin(42, 42);
        state.retry().unwrap();
        state.bind_work(&session);
        assert_eq!(
            state.receipt_for_session(
                7,
                if success {
                    Ok(())
                } else {
                    Err("磁盘".into())
                },
                !success,
                &mut session
            ),
            Some(false)
        );
        assert_eq!(session.finish(42), Some(42));
    }
}
#[test]
fn memory_first_admission_cannot_cancel_a_different_session() {
    use quick_panel::model::Command;
    let mut state = MemoryUiState::default();
    let mut session = OrganizerSession::default();
    session.begin(41, 41);
    state
        .submit(Command::HarnessMemoryDelete {
            request_id: 7,
            ids: vec![1],
        })
        .unwrap();
    state.bind_work(&session);
    session.begin(42, 42);
    assert_eq!(
        state.receipt_for_session(7, Ok(()), false, &mut session),
        Some(false)
    );
    assert_eq!(session.finish(42), Some(42));
}
#[test]
fn memory_empty_ime_composition_keeps_escape_enter_tab_with_ime_until_end() {
    let mut policy = MemoryInputPolicy::default();
    assert!(policy.dialog_keys(false));
    policy.composition(true);
    for _key in [27, 13, 9] {
        assert!(
            !policy.dialog_keys(false),
            "empty active composition must bypass dialog navigation"
        );
    }
    policy.composition(false);
    assert!(policy.dialog_keys(false));
    assert!(!policy.dialog_keys(true));
}
#[test]
fn memory_feedback_limit_is_256_but_rule_limit_is_512_and_raw_survives() {
    let raw = format!("{}\r\n\t末", "原".repeat(252));
    assert_eq!(
        memory_text_input(&raw, true).unwrap(),
        format!("{}   末", "原".repeat(252))
    );
    assert!(raw.contains("\r\n\t"));
    let too_long = "原".repeat(257);
    assert!(
        memory_text_input(&too_long, true)
            .unwrap_err()
            .contains("256")
    );
    assert_eq!(too_long.chars().count(), 257);
    assert!(memory_text_input(&"规".repeat(512), false).is_ok());
    assert!(memory_text_input(&"规".repeat(513), false).is_err());
}

#[test]
fn memory_scroll_explicitly_repositions_partially_clipped_children() {
    let layout = memory_layout(640, 360, 96, false);
    let positions = memory_scrolled_fields(&layout, 30);
    assert_eq!(positions[0].y, -20); // originally y=10, partially above viewport
    assert_eq!(positions[1].y, 38); // originally y=68
    assert_eq!(positions[6].y, 462); // originally y=492, below viewport
    for (physical, original) in positions.iter().zip(layout.fields) {
        assert_eq!(
            (physical.x, physical.width, physical.height),
            (original.x, original.width, original.height)
        );
    }
    assert_eq!(memory_scrolled_fields(&layout, -99), layout.fields);
    let clamped = memory_scrolled_fields(&layout, i32::MAX);
    assert_eq!(clamped[6].bottom(), layout.viewport_height - 24);
}

#[test]
fn clarity_manual_alias_error_names_field_and_retains_raw_input() {
    let mut ui = ReviewState::default();
    ui.snapshot.proposals = vec![proposal()];
    ui.select(Some(0));
    ui.check(0, true);
    let raw = format!(" {} ", "名".repeat(49));
    ui.edit(raw.clone(), "资料".into());
    let error = ui.begin_decision(false).unwrap_err();
    assert!(error.contains("别名最多48字符"), "{error}");
    assert_eq!(ui.editor_text().unwrap().0, raw);
    assert!(!ui.pending());
}
#[test]
fn clarity_manual_tags_error_names_count_and_item_limits() {
    let mut ui = ReviewState::default();
    ui.snapshot.proposals = vec![proposal()];
    ui.select(Some(0));
    ui.check(0, true);
    ui.edit("名称".into(), "一,二,三,四,五".into());
    let error = ui.begin_decision(false).unwrap_err();
    assert!(error.contains("新增标签最多4个"), "{error}");
    ui.edit("名称".into(), "标".repeat(25));
    let error = ui.begin_decision(false).unwrap_err();
    assert!(error.contains("每个新增标签最多24字符"), "{error}");
}

#[test]
fn clarity_memory_detail_discloses_rejected_alias_tags_scope_and_disabled_effect() {
    use quick_panel::harness_tools::*;
    let record = MemoryRecord {
        id: 1,
        scope: MemoryScope::Global,
        kind: MemoryKind::Rejected,
        origin: MemoryOrigin::Human,
        proposed_by_ai: false,
        text: "保留标识".into(),
        alias: Some("被拒名称完整".into()),
        tags: vec!["项目".into()],
        evidence_ids: vec![],
        confirmed: true,
        enabled: true,
        created_at_ms: 0,
    };
    let snapshot = MemorySnapshot {
        organizer_enabled: true,
        memory_enabled: false,
        records: vec![record.clone()],
        ..Default::default()
    };
    assert!(!snapshot.record_effective(&record));
    assert!(snapshot.availability().contains("整理记忆已关闭"));
    let detail = memory_detail(&record, &snapshot);
    for part in ["拒绝示例", "被拒名称完整", "项目", "全局", "当前不生效"] {
        assert!(detail.contains(part), "{detail}");
    }
    let impact = memory_scope_impact(&record, &MemoryScope::Global, &snapshot);
    for part in [
        "所选范围：全局",
        "被拒别名：被拒名称完整",
        "保留标识",
        "关联标签：项目",
    ] {
        assert!(impact.contains(part), "{impact}");
    }
    for organizer_enabled in [false, true] {
        for memory_enabled in [false, true] {
            let projection = MemorySnapshot {
                organizer_enabled,
                memory_enabled,
                ..snapshot.clone()
            };
            assert_eq!(
                projection.feedback_available(),
                organizer_enabled && memory_enabled
            );
            assert_eq!(
                projection.record_effective(&record),
                organizer_enabled && memory_enabled
            );
            assert_eq!(projection.records, snapshot.records);
        }
    }
}
#[test]
fn clarity_feedback_intro_explains_immediate_activation_and_has_no_confirm_fields() {
    let p = proposal();
    let target = displayed_feedback(&Snapshot {
        proposals: vec![p],
        ..Default::default()
    })
    .remove(0)
    .1;
    let intro = feedback_intro("原文", &target);
    for part in ["保存即启用", "建议", "范围", "拒绝"] {
        assert!(intro.contains(part), "{intro}");
    }
    let feedback = memory_layout(640, 360, 96, true);
    assert_eq!(feedback.fields[5].height, 0);
    assert_eq!(feedback.fields[6].height, 0);
    let management = memory_layout(640, 360, 96, false);
    assert!(management.fields[5].height > 0 && management.fields[6].height > 0);
}

#[test]
fn clarity_settings_navigation_keeps_raw_feedback_bound_to_exact_seen_target() {
    use quick_panel::{harness_tools::FeedbackScope, model::Command};
    let target = displayed_feedback(&Snapshot {
        proposals: vec![proposal()],
        ..Default::default()
    })
    .remove(0)
    .1;
    let raw = "  不要换名\r\n保留标识  ";
    let mut state = MemoryUiState::default();
    state.navigation_draft = Some(MemoryNavigationDraft {
        target: Some(target.clone()),
        record: None,
        raw_input: raw.into(),
        scope_choice: 2,
        enabled: false,
        confirmed: false,
    });
    let id = state.request_snapshot().unwrap();
    let Command::HarnessMemorySnapshot { request_id } = id else {
        panic!()
    };
    state.adopt(request_id, Default::default());
    let draft = state.navigation_draft_for(Some(&target)).unwrap();
    assert_eq!(draft.raw_input, raw);
    assert_eq!(draft.scope_choice, 2);
    assert!(state.navigation_draft_for(None).is_none());
    let mut advanced = target.clone();
    advanced.rejected_alias = Some("另一个所见别名".into());
    assert!(state.navigation_draft_for(Some(&advanced)).is_none());
    state
        .submit(Command::HarnessFeedback {
            request_id: 5,
            sources: vec![target.clone()],
            reason: "保留标识".into(),
            scope: FeedbackScope::Global,
        })
        .unwrap();
    assert!(state.navigation_draft_for(Some(&target)).is_none());
    assert!(state.receipt(5, Ok(()), false));
    assert!(state.navigation_draft.is_none());
}

#[test]
fn failed_feedback_with_disabled_parent_keeps_settings_and_original_retry_reachable() {
    use quick_panel::{
        harness_tools::{FeedbackScope, MemorySnapshot},
        model::Command,
    };
    let target = displayed_feedback(&Snapshot {
        proposals: vec![proposal()],
        ..Default::default()
    })
    .remove(0)
    .1;
    let raw = "  原始拒绝原因\r\n保留  ";
    for pending_save in [false, true] {
        for (organizer_enabled, memory_enabled) in [(false, true), (true, false), (false, false)] {
            let mut state = MemoryUiState::default();
            state.snapshot.organizer_enabled = true;
            state.snapshot.memory_enabled = true;
            state.raw_input = raw.into();
            let original = Command::HarnessFeedback {
                request_id: 42,
                sources: vec![target.clone()],
                reason: "原始拒绝原因 保留".into(),
                scope: FeedbackScope::Directory,
            };
            state.submit(original).unwrap();
            assert!(state.receipt(42, Err("同目录不可用或磁盘未落盘".into()), pending_save));
            let Command::HarnessMemorySnapshot { request_id } = state.request_snapshot().unwrap()
            else {
                panic!()
            };
            state.adopt(
                request_id,
                MemorySnapshot {
                    organizer_enabled,
                    memory_enabled,
                    ..Default::default()
                },
            );
            assert!(!state.snapshot.feedback_available());
            let buttons = state.footer_buttons(true);
            let settings = buttons
                .iter()
                .position(|button| {
                    button.action == MemoryFooterAction::Settings
                        && button.visible
                        && button.enabled
                })
                .expect("停用反馈的失败 pending 仍需独立设置入口");
            let retry = buttons
                .iter()
                .position(|button| {
                    button.action == MemoryFooterAction::Retry && button.visible && button.enabled
                })
                .expect("必须保留原命令重试");
            assert_ne!(settings, retry);
            assert_eq!(buttons[settings].label, "管家设置");
            assert_eq!(
                buttons[retry].label,
                if pending_save {
                    "重试保存"
                } else {
                    "重试上次提交"
                }
            );
            assert_eq!(state.footer_action(true, 0), None);
            assert_eq!(
                state.footer_action(true, settings),
                Some(MemoryFooterAction::Settings)
            );
            assert!(state.pending.is_some());
            assert!(!state.saving);
            assert_eq!(state.pending_save, pending_save);
            assert_eq!(state.raw_input, raw);
            assert_eq!(
                state.footer_action(true, retry),
                Some(MemoryFooterAction::Retry)
            );
            let Command::HarnessFeedback {
                request_id,
                sources,
                reason,
                scope,
            } = state.retry().unwrap()
            else {
                panic!()
            };
            assert_eq!(request_id, 42);
            assert_eq!(sources.as_slice(), std::slice::from_ref(&target));
            assert_eq!(reason, "原始拒绝原因 保留");
            assert_eq!(scope, FeedbackScope::Directory);
            assert_eq!(state.raw_input, raw);
            assert_eq!(
                state.footer_action(true, settings),
                None,
                "实际重试期间不重复发起导航"
            );
            for dpi in [96, 144, 192] {
                for (width, height) in [(320, 280), (640, 360), (1000, 620)] {
                    let layout = memory_layout(width, height, dpi, true);
                    for index in [settings, retry] {
                        let rect = layout.buttons[index];
                        assert!(
                            rect.x >= 0
                                && rect.right() <= width
                                && rect.y >= layout.viewport_height
                                && rect.bottom() <= height
                                && rect.width > 0
                                && rect.height > 0
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn memory_footer_keeps_management_delete_and_disables_empty_feedback_retry() {
    use quick_panel::model::Command;
    let mut state = MemoryUiState::default();
    assert_eq!(
        state.footer_action(false, 0),
        Some(MemoryFooterAction::Save)
    );
    assert_eq!(
        state.footer_action(false, 1),
        Some(MemoryFooterAction::Settings)
    );
    assert_eq!(
        state.footer_action(false, 2),
        Some(MemoryFooterAction::Delete)
    );
    assert_eq!(state.footer_action(true, 1), None);
    assert_eq!(
        state.footer_action(true, 2),
        Some(MemoryFooterAction::Settings)
    );
    assert_eq!(state.footer_action(true, 4), None);
    state
        .submit(Command::HarnessMemoryDelete {
            request_id: 7,
            ids: vec![1],
        })
        .unwrap();
    assert_eq!(state.footer_action(false, 1), None);
    assert_eq!(state.footer_action(false, 2), None);
    assert_eq!(
        state.footer_action(false, 3),
        Some(MemoryFooterAction::Close)
    );
    assert!(state.receipt(7, Err("合成磁盘错误".into()), true));
    assert_eq!(
        state.footer_action(false, 1),
        Some(MemoryFooterAction::Retry)
    );
    assert_eq!(state.footer_action(false, 2), None);
}

#[test]
fn settings_return_restores_only_unaccepted_feedback_edits_without_changing_retry() {
    use quick_panel::{harness_tools::FeedbackScope, model::Command};
    let target = displayed_feedback(&Snapshot {
        proposals: vec![proposal()],
        ..Default::default()
    })
    .remove(0)
    .1;
    for accepted in [false, true] {
        let mut state = MemoryUiState::default();
        state.raw_input = "  原提交草稿  ".into();
        state
            .submit(Command::HarnessFeedback {
                request_id: 52,
                sources: vec![target.clone()],
                reason: "原提交原因".into(),
                scope: FeedbackScope::Directory,
            })
            .unwrap();
        assert!(state.receipt(52, Err("未保存".into()), accepted));
        // A later failed receipt must not revoke earlier semantic admission.
        if accepted {
            assert!(state.receipt(52, Err("仍未保存".into()), false));
        }
        state.navigation_draft = Some(MemoryNavigationDraft {
            target: Some(target.clone()),
            record: None,
            raw_input: "  新未提交草稿\r\n  ".into(),
            scope_choice: 2,
            enabled: false,
            confirmed: false,
        });
        assert_eq!(
            state.footer_buttons(true)[1].label,
            if accepted {
                "重试保存"
            } else {
                "重试上次提交"
            }
        );
        let restored = state.navigation_draft_for(Some(&target));
        if accepted {
            assert!(restored.is_none());
        } else {
            let draft = restored.expect("未接受失败的新编辑应跨设置往返保留");
            assert_eq!(draft.raw_input, "  新未提交草稿\r\n  ");
            assert_eq!(draft.scope_choice, 2);
            let mut different = target.clone();
            different.expected_version = "另一版本".into();
            state.navigation_draft.as_mut().unwrap().target = Some(different.clone());
            assert!(
                state.navigation_draft_for(Some(&different)).is_none(),
                "不能将 pending 的导航草稿绑定到另一所见版本"
            );
            state.navigation_draft.as_mut().unwrap().target = Some(target.clone());
        }
        assert_eq!(state.raw_input, "  原提交草稿  ");
        let Command::HarnessFeedback {
            request_id,
            sources,
            reason,
            scope,
        } = state.retry().unwrap()
        else {
            panic!()
        };
        assert_eq!(request_id, 52);
        assert_eq!(sources.as_slice(), std::slice::from_ref(&target));
        assert_eq!(reason, "原提交原因");
        assert_eq!(scope, FeedbackScope::Directory);
        assert!(state.navigation_draft_for(Some(&target)).is_none());
        if !accepted {
            assert!(state.receipt(52, Err("原提交仍失败".into()), false));
            let Command::HarnessMemorySnapshot { request_id } = state.request_snapshot().unwrap()
            else {
                panic!()
            };
            state.adopt(
                request_id,
                quick_panel::harness_tools::MemorySnapshot {
                    organizer_enabled: true,
                    memory_enabled: true,
                    ..Default::default()
                },
            );
            assert_eq!(state.footer_action(true, 0), Some(MemoryFooterAction::Save));
            let draft = state.navigation_draft_for(Some(&target)).unwrap();
            let reason = memory_text_input(&draft.raw_input, true).unwrap();
            state.raw_input = draft.raw_input;
            let Command::HarnessFeedback {
                request_id,
                reason,
                scope,
                ..
            } = state
                .submit(Command::HarnessFeedback {
                    request_id: 53,
                    sources: vec![target.clone()],
                    reason,
                    scope: FeedbackScope::Global,
                })
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(request_id, 53);
            assert_eq!(reason, "新未提交草稿");
            assert_eq!(scope, FeedbackScope::Global);
        }
    }
}

fn navigation_record(id: u64) -> quick_panel::harness_tools::MemoryRecord {
    use quick_panel::harness_tools::*;
    MemoryRecord {
        id,
        scope: MemoryScope::Global,
        kind: MemoryKind::Rule,
        origin: MemoryOrigin::Human,
        proposed_by_ai: false,
        text: format!("原记录 {id}"),
        alias: None,
        tags: vec![],
        evidence_ids: vec![],
        confirmed: true,
        enabled: true,
        created_at_ms: 0,
    }
}

#[test]
fn navigation_frames_preserve_feedback_and_nested_memory_drafts() {
    use quick_panel::settings_ui::{Group, Navigation, Page};
    let target = displayed_feedback(&Snapshot {
        proposals: vec![proposal()],
        ..Default::default()
    })
    .remove(0)
    .1;
    for page in [Page::Review, Page::Memory] {
        let mut nav = Navigation::default();
        let mut state = MemoryUiState::default();
        nav.open_with_memory(page, &mut state);
        state.navigation_draft = Some(MemoryNavigationDraft {
            target: (page == Page::Review).then(|| target.clone()),
            record: (page == Page::Memory).then(|| navigation_record(101)),
            raw_input: "  来源 A\r\n未提交  ".into(),
            scope_choice: 2,
            enabled: true,
            confirmed: false,
        });
        nav.open_with_memory(Page::Hub(Group::Organizer), &mut state);
        nav.open_with_memory(Page::Memory, &mut state);
        // Exactly the production selected() transition, before editing record B.
        state.clear_current_navigation_draft();
        state.navigation_draft = Some(MemoryNavigationDraft {
            target: None,
            record: Some(navigation_record(202)),
            raw_input: "  B\r\n草稿  ".into(),
            scope_choice: 1,
            enabled: false,
            confirmed: true,
        });
        nav.open_with_memory(Page::Hub(Group::Services), &mut state);
        nav.close_with_memory(&mut state);
        let b = state.navigation_draft_for(None).unwrap();
        assert_eq!(b.raw_input, "  B\r\n草稿  ");
        assert_eq!(b.scope_choice, 1);
        assert_eq!(b.record, Some(navigation_record(202)));
        nav.close_with_memory(&mut state);
        nav.close_with_memory(&mut state);
        assert_eq!(nav.current(), Some(page));
        let a = state
            .navigation_draft_for((page == Page::Review).then_some(&target))
            .expect("来源帧 A 的草稿不能被 B 的选择或设置覆盖");
        assert_eq!(a.raw_input, "  来源 A\r\n未提交  ");
        assert_eq!(a.scope_choice, 2);
        assert!(a.enabled && !a.confirmed);
        assert_eq!(
            a.record,
            (page == Page::Memory).then(|| navigation_record(101))
        );
        let mut changed = target.clone();
        changed.expected_version = "不同版本".into();
        assert!(state.navigation_draft_for(Some(&changed)).is_none());
    }
}

#[test]
fn late_memory_receipt_clears_only_its_origin_frame_and_keeps_retry_immutable() {
    use quick_panel::{
        harness_tools::FeedbackScope,
        model::Command,
        settings_ui::{Group, Navigation, Page},
    };
    let target = displayed_feedback(&Snapshot {
        proposals: vec![proposal()],
        ..Default::default()
    })
    .remove(0)
    .1;
    for admitted in [false, true] {
        let mut nav = Navigation::default();
        let mut state = MemoryUiState::default();
        nav.open_with_memory(Page::Review, &mut state);
        state.raw_input = "  已发送原文  ".into();
        state.navigation_draft = Some(MemoryNavigationDraft {
            target: Some(target.clone()),
            record: None,
            raw_input: "A 未提交编辑".into(),
            scope_choice: 2,
            enabled: true,
            confirmed: true,
        });
        state
            .submit(Command::HarnessFeedback {
                request_id: 770,
                sources: vec![target.clone()],
                reason: "已发送原文".into(),
                scope: FeedbackScope::Directory,
            })
            .unwrap();
        assert!(state.receipt(770, Err("失败".into()), admitted));
        nav.open_with_memory(Page::Hub(Group::Organizer), &mut state);
        nav.select_group(Group::Services);
        nav.open_with_memory(Page::Hub(Group::Services), &mut state); // re-show, no extra frame
        nav.open_with_memory(Page::Memory, &mut state);
        state.navigation_draft = Some(MemoryNavigationDraft {
            target: None,
            record: None,
            raw_input: "B 的独立草稿".into(),
            scope_choice: 1,
            enabled: false,
            confirmed: false,
        });
        let Command::HarnessFeedback {
            request_id,
            sources,
            reason,
            scope,
        } = state.retry().unwrap()
        else {
            panic!()
        };
        assert_eq!(request_id, 770);
        assert_eq!(sources, vec![target.clone()]);
        assert_eq!(reason, "已发送原文");
        assert_eq!(scope, FeedbackScope::Directory);
        assert!(state.receipt(770, Err("重试失败".into()), false));
        assert!(state.navigation_draft_for(None).is_none());
        assert_eq!(state.raw_input, "  已发送原文  ");
        assert!(state.receipt(770, Ok(()), false));
        assert_eq!(
            state.navigation_draft_for(None).unwrap().raw_input,
            "B 的独立草稿"
        );
        nav.close_with_memory(&mut state);
        nav.close_with_memory(&mut state);
        assert_eq!(nav.current(), Some(Page::Review));
        assert!(
            state.navigation_draft_for(Some(&target)).is_none(),
            "已保存的暂停帧草稿不能复活"
        );
        assert!(state.pending.is_none());
    }
}

#[test]
fn nested_memory_save_and_navigation_teardown_never_consume_another_frame_draft() {
    use quick_panel::{
        model::Command,
        settings_ui::{Group, Navigation, Page},
    };
    let mut nav = Navigation::default();
    let mut state = MemoryUiState::default();
    nav.open_with_memory(Page::Memory, &mut state);
    state.navigation_draft = Some(MemoryNavigationDraft {
        target: None,
        record: Some(navigation_record(101)),
        raw_input: "A 原始草稿".into(),
        scope_choice: 2,
        enabled: true,
        confirmed: false,
    });
    nav.open_with_memory(Page::Hub(Group::Organizer), &mut state);
    nav.open_with_memory(Page::Memory, &mut state);
    state.clear_current_navigation_draft();
    state
        .submit(Command::HarnessMemoryDelete {
            request_id: 771,
            ids: vec![202],
        })
        .unwrap();
    assert!(state.receipt(771, Ok(()), false));
    nav.close_with_memory(&mut state);
    nav.close_with_memory(&mut state);
    let restored = state.navigation_draft_for(None).unwrap();
    assert_eq!(restored.record, Some(navigation_record(101)));
    assert_eq!(restored.raw_input, "A 原始草稿");
    nav.clear_with_memory(&mut state);
    assert!(nav.allows_panel_query());
    nav.open_with_memory(Page::Memory, &mut state);
    assert!(state.navigation_draft_for(None).is_none());
    assert!(!state.receipt(771, Ok(()), false));
}

#[test]
fn unchanged_human_save_locks_selection_and_receipt_rechecks_action_identity() {
    use quick_panel::{harness_tools::*, model::Command};
    let mut state = MemoryUiState::default();
    let input = MemoryEdit {
        id: Some(101),
        scope: MemoryScope::Global,
        text: "合成规则A".into(),
        enabled: true,
        confirmed: true,
    };
    state
        .submit(Command::HarnessMemoryEdit {
            request_id: 44,
            input,
        })
        .unwrap();
    assert!(!state.selection_enabled());
    assert!(!state.action_identity_current(Some(102), Some(101)));
    assert!(state.receipt(44, Ok(()), true));
    assert!(!state.selection_enabled());
    assert!(state.retry().is_some());
    assert!(state.receipt(44, Ok(()), false));
    assert!(state.selection_enabled());
    assert!(!state.action_identity_current(Some(102), Some(101)));
    assert!(state.action_identity_current(Some(101), Some(101)));
}
