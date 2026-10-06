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
