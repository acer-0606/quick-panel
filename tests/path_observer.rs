use quick_panel::{
    model::ItemKind,
    path_activity::{ObservationSource, PathActivitySettings, PathObservation},
    path_observer::{PathObserver, ShellViewIdentity, select_active_view},
};
fn enabled() -> PathObserver {
    let mut observer = PathObserver::default();
    observer.configure(PathActivitySettings {
        enabled: true,
        ..Default::default()
    });
    observer.foreground(Some(10));
    observer
}
fn path(path: &str, source: ObservationSource) -> PathObservation {
    PathObservation {
        path: path.into(),
        kind: ItemKind::Folder,
        observed_at_ms: 100_000,
        source,
    }
}
fn observe(observer: &mut PathObserver, target: &str, time: u64) -> Vec<PathObservation> {
    let ticket = observer
        .begin_probe()
        .expect("enabled idle observer admits one probe");
    observer.complete(
        ticket,
        vec![path(target, ObservationSource::Explorer)],
        time,
    )
}
#[test]
fn stable_visit_counts_once_until_navigation_even_after_focus_loss() {
    let mut observer = enabled();
    assert!(observe(&mut observer, "C:/A", 0).is_empty());
    assert_eq!(observe(&mut observer, "C:/A", 1500).len(), 1);
    assert!(observe(&mut observer, "C:/A", 4000).is_empty());
    observer.foreground(None);
    observer.foreground(Some(10));
    assert!(observe(&mut observer, "C:/A", 9000).is_empty());
    assert!(observe(&mut observer, "C:/B", 10000).is_empty());
    assert_eq!(observe(&mut observer, "C:/B", 11500)[0].path, "C:\\B");
    assert!(observe(&mut observer, "C:/A", 12000).is_empty());
    assert_eq!(observe(&mut observer, "C:/A", 13500).len(), 1);
}
#[test]
fn quick_navigation_and_new_windows_have_independent_visits() {
    let mut observer = enabled();
    assert!(observe(&mut observer, "C:/A", 0).is_empty());
    assert!(observe(&mut observer, "C:/B", 500).is_empty());
    assert!(observe(&mut observer, "C:/C", 1000).is_empty());
    assert_eq!(observe(&mut observer, "C:/C", 2500).len(), 1);
    observer.foreground(Some(11));
    assert!(observe(&mut observer, "C:/C", 3000).is_empty());
    assert_eq!(observe(&mut observer, "C:/C", 4500).len(), 1);
}
#[test]
fn config_clear_and_focus_invalidate_receipts_without_releasing_occupancy() {
    for boundary in 0..4 {
        let mut observer = enabled();
        let old = observer.begin_probe().unwrap();
        assert!(observer.busy());
        assert!(observer.begin_probe().is_none());
        match boundary {
            0 => observer.configure(PathActivitySettings::default()),
            1 => observer.configure(PathActivitySettings {
                enabled: true,
                recent_documents: false,
                excluded_paths: vec![],
            }),
            2 => observer.clear(),
            _ => {
                observer.foreground(None);
                observer.foreground(Some(10));
            }
        }
        assert!(observer.busy());
        assert!(observer.begin_probe().is_none());
        assert!(
            observer
                .complete(old, vec![path("C:/A", ObservationSource::Explorer)], 2000)
                .is_empty()
        );
        assert!(!observer.busy());
    }
}
#[test]
fn failure_releases_probe_and_exclusions_apply_before_visits() {
    let mut observer = enabled();
    let ticket = observer.begin_probe().unwrap();
    assert!(observer.complete(ticket, vec![], 0).is_empty());
    assert!(!observer.busy());
    observer.configure(PathActivitySettings {
        enabled: true,
        recent_documents: false,
        excluded_paths: vec!["C:/private".into()],
    });
    assert!(observe(&mut observer, "C:/private/child", 0).is_empty());
    assert!(observe(&mut observer, "C:/private/child", 2000).is_empty());
    let ticket = observer.begin_probe().unwrap();
    assert!(
        observer
            .complete(
                ticket,
                vec![path("C:/public", ObservationSource::RecentDocuments)],
                3000
            )
            .is_empty()
    );
}
#[test]
fn same_path_multiple_sources_is_one_observation_per_receipt() {
    let mut observer = enabled();
    assert!(observe(&mut observer, "C:/A", 0).is_empty());
    let ticket = observer.begin_probe().unwrap();
    let output = observer.complete(
        ticket,
        vec![
            path("C:/A", ObservationSource::Explorer),
            path("c:/a", ObservationSource::RecentDocuments),
        ],
        2000,
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].source, ObservationSource::Explorer);
}
#[test]
fn hidden_tabs_wrong_roots_and_ambiguous_visible_views_are_rejected() {
    let views = [
        ShellViewIdentity {
            root: 10,
            view: 101,
            visible: false,
            active: true,
        },
        ShellViewIdentity {
            root: 10,
            view: 102,
            visible: true,
            active: true,
        },
        ShellViewIdentity {
            root: 11,
            view: 103,
            visible: true,
            active: true,
        },
    ];
    assert_eq!(select_active_view(10, &views), Some(102));
    assert_eq!(select_active_view(99, &views), None);
    let mut ambiguous = views.to_vec();
    ambiguous.push(ShellViewIdentity {
        root: 10,
        view: 104,
        visible: true,
        active: true,
    });
    assert_eq!(select_active_view(10, &ambiguous), None);
}

#[test]
fn recent_baseline_never_replays_old_links_and_limits_changed_parses() {
    use quick_panel::path_observer::{RecentBaseline, RecentLink};
    let links: Vec<_> = (0..2050)
        .map(|i| RecentLink {
            name: format!("{i:04}.lnk"),
            modified: 1,
            size: 100,
        })
        .collect();
    let mut baseline = RecentBaseline::default();
    assert!(baseline.changed(links.clone()).is_empty());
    assert!(baseline.changed(links.clone()).is_empty());
    let changed = links
        .iter()
        .take(20)
        .cloned()
        .map(|mut link| {
            link.modified = 2;
            link
        })
        .chain(links.iter().skip(20).cloned())
        .collect();
    assert_eq!(baseline.changed(changed).len(), 16);
    let changed = links
        .iter()
        .cloned()
        .map(|mut link| {
            if link.name < "0020.lnk".into() {
                link.modified = 2;
            }
            link
        })
        .collect();
    assert!(baseline.changed(changed).is_empty());
    baseline.reset();
    assert!(baseline.changed(links).is_empty());
}
#[test]
fn exclusion_form_validates_syntax_and_entry_budget_without_target_io() {
    use quick_panel::path_observer::parse_exclusions;
    assert_eq!(
        parse_exclusions(" C:/Private\r\n\\\\server\\share\n\n").unwrap(),
        vec!["C:\\Private", "\\\\server\\share\\"]
    );
    assert!(parse_exclusions("relative/path").is_err());
    assert!(
        parse_exclusions(
            &(0..129)
                .map(|n| format!("C:/Path{n}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
        .is_err()
    );
}

#[test]
fn failed_or_ambiguous_shell_sample_interrupts_the_stability_window() {
    let mut observer = enabled();
    assert!(observe(&mut observer, "C:/A", 0).is_empty());
    let failed = observer.begin_probe().unwrap();
    assert!(observer.complete(failed, vec![], 1000).is_empty());
    assert!(observe(&mut observer, "C:/A", 2000).is_empty());
    assert_eq!(observe(&mut observer, "C:/A", 3500).len(), 1);
}

#[test]
fn real_blocked_worker_keeps_its_slot_through_config_cancellation() {
    use quick_panel::background_job::BackgroundWorker;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = BackgroundWorker::new(
        move |ticket| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            ticket
        },
        Arc::new(move || {
            done_tx.send(()).unwrap();
        }),
    );
    let mut observer = enabled();
    let ticket = observer.begin_probe().unwrap();
    assert!(worker.submit(ticket));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    observer.clear();
    observer.configure(PathActivitySettings::default());
    observer.configure(PathActivitySettings {
        enabled: true,
        ..Default::default()
    });
    assert!(observer.begin_probe().is_none());
    assert!(!worker.submit(ticket));
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let receipt = worker.try_result().unwrap();
    assert!(
        observer
            .complete(
                receipt,
                vec![path("C:/A", ObservationSource::Explorer)],
                5000
            )
            .is_empty()
    );
    assert!(!worker.busy());
    assert!(observer.begin_probe().is_some());
}

#[test]
fn recent_raw_targets_reject_truncation_variables_and_ambiguous_type_without_io() {
    use quick_panel::path_observer::recent_target;
    assert_eq!(
        recent_target("C:/DeletedDocument.txt", false, true).unwrap(),
        ("C:\\DeletedDocument.txt".into(), ItemKind::Document)
    );
    assert_eq!(
        recent_target("\\\\OfflineServer\\Share\\Folder", true, true).unwrap(),
        ("\\\\OfflineServer\\Share\\Folder".into(), ItemKind::Folder)
    );
    assert!(recent_target("C:/%USERPROFILE%/file.txt", false, true).is_none());
    assert!(recent_target("C:/file.txt", false, false).is_none());
    assert!(recent_target(&format!("C:/{}", "a".repeat(256)), false, true).is_none());
    assert!(recent_target("relative/file.txt", false, true).is_none());
}

#[test]
fn recent_initialization_failure_retries_only_after_thirty_seconds() {
    use quick_panel::path_observer::RecentRecovery;
    let mut recovery = RecentRecovery::default();
    recovery.reset(true, 1, 0);
    let first = recovery.begin(0).unwrap();
    assert!(recovery.begin(2000).is_none());
    assert!(recovery.complete(first, false, 1000));
    for tick in (2000..31000).step_by(2000) {
        assert!(!recovery.due(tick));
        assert!(recovery.begin(tick).is_none());
    }
    assert!(!recovery.due(30999));
    assert!(recovery.due(31000));
    let second = recovery.begin(31000).unwrap();
    assert!(recovery.complete(second, false, 32000));
    assert!(recovery.begin(61999).is_none());
    assert!(recovery.begin(62000).is_some());
}

#[test]
fn recent_renewal_failure_rebuilds_after_backoff_and_success_restores_notifications() {
    use quick_panel::path_observer::RecentRecovery;
    let mut recovery = RecentRecovery::default();
    recovery.reset(true, 3, 0);
    let initial = recovery.begin(0).unwrap();
    assert!(recovery.complete(initial, true, 10));
    assert!(recovery.ready());
    assert!(!recovery.due(5000));
    let revision = recovery.revision();
    recovery.lost(5000);
    assert_ne!(recovery.revision(), revision);
    assert!(!recovery.ready());
    assert!(recovery.begin(34999).is_none());
    let rebuilt = recovery.begin(35000).unwrap();
    assert!(recovery.complete(rebuilt, true, 35001));
    assert!(recovery.ready());
    assert!(recovery.begin(100000).is_none());
}

#[test]
fn cancelled_recent_setup_cannot_restore_a_watcher_or_release_a_new_attempt() {
    use quick_panel::path_observer::RecentRecovery;
    for enabled_after_cancel in [false, true] {
        let mut recovery = RecentRecovery::default();
        recovery.reset(true, 1, 0);
        let old = recovery.begin(0).unwrap();
        recovery.reset(enabled_after_cancel, 2, 100);
        assert!(recovery.begin(1000).is_none());
        assert!(!recovery.complete(old, true, 1000));
        assert!(!recovery.ready());
        let next = recovery.begin(1000);
        assert_eq!(next.is_some(), enabled_after_cancel);
        if let Some(next) = next {
            assert!(!recovery.complete(old, true, 1001));
            assert!(recovery.begin(1002).is_none());
            assert!(recovery.complete(next, true, 1003));
            assert!(recovery.ready());
        }
    }
}

#[test]
fn recent_backoff_does_not_block_explorer_visits() {
    use quick_panel::path_observer::RecentRecovery;
    let mut recovery = RecentRecovery::default();
    recovery.reset(true, 1, 0);
    let ticket = recovery.begin(0).unwrap();
    assert!(recovery.complete(ticket, false, 0));
    let mut observer = enabled();
    assert!(observe(&mut observer, "C:/Explorer", 0).is_empty());
    assert_eq!(observe(&mut observer, "C:/Explorer", 2000).len(), 1);
    assert!(!recovery.due(2000));
}
