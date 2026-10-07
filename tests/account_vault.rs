use quick_panel::{account_vault::*, engine::Engine, model::*};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn password() -> Secret {
    Secret::new("Synthetic master password".into())
}
fn document() -> VaultDocument {
    VaultDocument {
        accounts: vec![Account {
            id: "a1".into(),
            name: "Synthetic secret name".into(),
            username: "synthetic-user@example".into(),
            password: Secret::new("Synthetic account password".into()),
        }],
        bindings: vec![],
    }
}
fn wait(engine: &Engine, id: u64) -> Event {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(event) = engine.try_recv() {
            match event {
                Event::AccountVaultLoaded { request_id, .. }
                | Event::AccountVaultWritten { request_id, .. }
                    if request_id == id =>
                {
                    return event;
                }
                Event::Changed | Event::Error { .. } => panic!("unexpected actor event: {event:?}"),
                _ => {}
            }
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn crypto_round_trip_authenticates_password_and_payload() {
    let session = VaultSession::create(&password(), &password(), document())
        .expect("create encrypted session");
    let envelope = session.encrypt().unwrap();
    let snapshot = VaultSnapshot {
        revision: 1,
        envelope,
    };
    let opened = VaultSession::unlock(&password(), &snapshot).unwrap();
    assert_eq!(
        opened.document().unwrap().accounts[0].password.expose(),
        "Synthetic account password"
    );
    assert!(VaultSession::unlock(&Secret::new("Wrong master password".into()), &snapshot).is_err());
}
#[test]
fn actor_persists_only_valid_envelope_and_correlates_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        EngineOptions::new(temp.path().join("vault.sqlite")),
        Arc::new(|| {}),
    )
    .unwrap();
    engine
        .send(Command::ReadAccountVault { request_id: 1 })
        .unwrap();
    assert!(matches!(
        wait(&engine, 1),
        Event::AccountVaultLoaded {
            result: Ok(None),
            ..
        }
    ));
    let mut envelope = b"QPVAULT\0".to_vec();
    for value in [1u32, 2, 19, 65536, 3, 1, 32] {
        envelope.extend(value.to_le_bytes());
    }
    envelope.extend([0; 40]);
    envelope.extend(20u32.to_le_bytes());
    envelope.extend([0; 20]);
    engine
        .send(Command::WriteAccountVault {
            request_id: 2,
            expected_revision: 0,
            envelope,
        })
        .unwrap();
    assert!(
        matches!(
            wait(&engine, 2),
            Event::AccountVaultWritten { result: Ok(1), .. }
        ),
        "encrypted store is not implemented"
    );
}
#[test]
fn envelope_randomness_tampering_and_limits() {
    let session = VaultSession::create(&password(), &password(), document()).unwrap();
    let one = session.encrypt().unwrap();
    let two = session.encrypt().unwrap();
    assert_ne!(&one[52..76], &two[52..76]);
    let independent = VaultSession::create(&password(), &password(), document())
        .unwrap()
        .encrypt()
        .unwrap();
    assert_ne!(&one[36..52], &independent[36..52]);
    for index in [0, 8, 12, 16, 20, 24, 28, 32, 36, 52, 76, 80, one.len() - 1] {
        let mut envelope = one.clone();
        envelope[index] ^= 1;
        assert!(
            VaultSession::unlock(
                &password(),
                &VaultSnapshot {
                    revision: 1,
                    envelope
                }
            )
            .is_err(),
            "tamper {index}"
        );
    }
    for length in [0, 7, 79, 95, one.len() - 1] {
        assert!(validate_envelope(&one[..length]).is_err());
    }
    assert!(validate_envelope(&vec![0; MAX_ENVELOPE_BYTES + 1]).is_err());
    assert!(!format!("{:?}", password()).contains("Synthetic"));
}
#[test]
fn document_edits_preserve_password_and_enforce_references_limits() {
    let mut doc = document();
    doc.upsert_account(AccountDraft {
        id: "a1".into(),
        name: "Changed".into(),
        username: "u".into(),
        password: Secret::new(String::new()),
    })
    .unwrap();
    assert_eq!(
        doc.accounts[0].password.expose(),
        "Synthetic account password"
    );
    assert!(
        doc.upsert_account(AccountDraft {
            id: "a2".into(),
            name: "n".into(),
            username: "u".into(),
            password: Secret::new(String::new())
        })
        .is_err()
    );
    let mut invalid = document();
    invalid.accounts[0].name = "x".repeat(97);
    assert!(invalid.validate().is_err());
    invalid = document();
    invalid.accounts[0].password = Secret::new("x".repeat(1025));
    assert!(invalid.validate().is_err());
    invalid = document();
    invalid.accounts[0].password = Secret::new("bad\npassword".into());
    assert!(invalid.validate().is_err());
    invalid = document();
    invalid.accounts.push(invalid.accounts[0].clone());
    assert!(invalid.validate().is_err());
    invalid = VaultDocument::default();
    for n in 0..513 {
        let mut a = document().accounts.remove(0);
        a.id = n.to_string();
        invalid.accounts.push(a);
    }
    assert!(invalid.validate().is_err());
    for master in ["short".to_string(), "x".repeat(257)] {
        let p = Secret::new(master);
        assert!(VaultSession::create(&p, &p, document()).is_err());
    }
    assert!(
        VaultSession::create(
            &password(),
            &Secret::new("Mismatch password".into()),
            document()
        )
        .is_err()
    );
}
use quick_panel::account_vault_jobs::*;
fn poll(controller: &mut VaultController) -> VaultNotice {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(n) = controller.poll() {
            return n;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn accepted_preparation_survives_lock_and_late_receipt_cannot_unlock() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        EngineOptions::new(temp.path().join("vault.sqlite")),
        Arc::new(|| {}),
    )
    .unwrap();
    let mut c = VaultController::new(Arc::new(|| {}));
    c.accept_create(10, password(), password(), document())
        .unwrap();
    assert_eq!(c.state(), SaveState::Preparing);
    assert!(!c.can_shutdown());
    c.lock();
    assert!(c.busy());
    assert!(!c.session().is_unlocked());
    assert!(c.session().summaries().is_empty());
    assert_eq!(poll(&mut c), VaultNotice::Prepared);
    assert!(!c.can_shutdown());
    c.dispatch_save(|command| engine.send(command)).unwrap();
    assert_eq!(
        c.handle_event(&Event::AccountVaultWritten {
            request_id: 999,
            result: Ok(1)
        }),
        None
    );
    assert_eq!(c.handle_event(&wait(&engine, 10)), Some(VaultNotice::Saved));
    assert!(!c.session().is_unlocked());
    assert!(c.can_shutdown());
    engine
        .send(Command::ReadAccountVault { request_id: 11 })
        .unwrap();
    let Event::AccountVaultLoaded {
        result: Ok(Some(snapshot)),
        ..
    } = wait(&engine, 11)
    else {
        panic!()
    };
    c.begin_unlock(c.session().epoch(), password(), snapshot)
        .unwrap();
    assert_eq!(poll(&mut c), VaultNotice::Unlocked);
    assert_eq!(
        c.session().credentials("a1").unwrap().1.expose(),
        "Synthetic account password"
    );
    c.accept_save(12, document()).unwrap();
    c.lock();
    assert_eq!(poll(&mut c), VaultNotice::Prepared);
    let mut original = None;
    c.dispatch_save(|command| {
        original = Some(command);
        Err("合成发送失败".into())
    })
    .unwrap_err();
    assert_eq!(c.state(), SaveState::FailedWrite);
    assert!(!c.can_shutdown());
    c.retry().unwrap();
    c.dispatch_save(|command| {
        if let (
            Command::WriteAccountVault {
                request_id: a,
                expected_revision: b,
                envelope: x,
            },
            Some(Command::WriteAccountVault {
                request_id: c,
                expected_revision: d,
                envelope: y,
            }),
        ) = (&command, &original)
        {
            assert_eq!((a, b, x), (c, d, y));
        } else {
            panic!()
        };
        engine.send(command)
    })
    .unwrap();
    assert_eq!(c.handle_event(&wait(&engine, 12)), Some(VaultNotice::Saved));
    assert!(!c.session().is_unlocked());
}
#[test]
fn canceled_unlock_retains_actual_slot_until_completion() {
    let s = VaultSession::create(&password(), &password(), document()).unwrap();
    let snapshot = VaultSnapshot {
        revision: 1,
        envelope: s.encrypt().unwrap(),
    };
    let mut c = VaultController::new(Arc::new(|| {}));
    c.begin_unlock(0, password(), snapshot.clone()).unwrap();
    c.lock();
    assert!(c.busy());
    assert!(
        c.begin_unlock(c.session().epoch(), password(), snapshot)
            .is_err()
    );
    assert_eq!(poll(&mut c), VaultNotice::Stale);
    assert!(!c.busy());
    assert!(!c.session().is_unlocked());
}
fn binding() -> quick_panel::account_fill::Binding {
    use quick_panel::account_fill::*;
    let selector = FieldSelector {
        automation_id: Some("Synthetic-password-field".into()),
        control_type: 50004,
        class_name: Some("Edit".into()),
        anchors: vec![],
    };
    Binding {
        binding_id: "b1".into(),
        account_id: "a1".into(),
        app: AppIdentity {
            canonical_path: "C:\\Synthetic\\client.exe".into(),
            file_fingerprint: [3; 32],
        },
        scene_key: "Synthetic-secret-scene".into(),
        password: selector.clone(),
        username: None,
        target: Some(TargetCondition {
            selector: FieldSelector {
                automation_id: Some("server".into()),
                ..selector
            },
            expected: "Synthetic-secret-server:5432".into(),
        }),
    }
}
fn read_snapshot(engine: &Engine, id: u64) -> Option<VaultSnapshot> {
    engine
        .send(Command::ReadAccountVault { request_id: id })
        .unwrap();
    let Event::AccountVaultLoaded { result: Ok(s), .. } = wait(engine, id) else {
        panic!()
    };
    s
}
fn write_snapshot(
    engine: &Engine,
    id: u64,
    expected: u64,
    envelope: Vec<u8>,
) -> Result<u64, String> {
    engine
        .send(Command::WriteAccountVault {
            request_id: id,
            expected_revision: expected,
            envelope,
        })
        .unwrap();
    let Event::AccountVaultWritten { result, .. } = wait(engine, id) else {
        panic!()
    };
    result
}
#[test]
fn sqlite_failure_cas_replay_and_shutdown_preserve_encrypted_state() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault.sqlite");
    let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    assert!(read_snapshot(&engine, 1).is_none());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='account_vault'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(
        engine
            .send(Command::WriteAccountVault {
                request_id: 2,
                expected_revision: 0,
                envelope: b"Synthetic plaintext must not be stored".to_vec()
            })
            .is_err()
    );
    let mut doc = document();
    doc.upsert_binding(binding()).unwrap();
    let s = VaultSession::create(&password(), &password(), doc.clone()).unwrap();
    let first = s.encrypt().unwrap();
    assert_eq!(write_snapshot(&engine, 3, 0, first.clone()), Ok(1));
    let baseline: i64 = db
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(write_snapshot(&engine, 3, 0, first.clone()), Ok(1));
    for id in 4..8 {
        assert_eq!(read_snapshot(&engine, id).unwrap().revision, 1);
    }
    assert_eq!(
        db.query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        baseline
    );
    db.execute_batch("CREATE TRIGGER fail_vault BEFORE UPDATE ON account_vault BEGIN SELECT RAISE(ABORT,'Synthetic SQL failure'); END;").unwrap();
    let second = s.encrypt().unwrap();
    assert!(write_snapshot(&engine, 8, 1, second.clone()).is_err());
    let original = read_snapshot(&engine, 9).unwrap();
    assert_eq!(original.envelope, first);
    assert_eq!(
        VaultSession::unlock(&password(), &original)
            .unwrap()
            .document()
            .unwrap()
            .bindings[0]
            .target
            .as_ref()
            .unwrap()
            .expected,
        "Synthetic-secret-server:5432"
    );
    db.execute_batch("DROP TRIGGER fail_vault").unwrap();
    assert_eq!(write_snapshot(&engine, 8, 1, second.clone()), Ok(2));
    assert_eq!(write_snapshot(&engine, 8, 1, second.clone()), Ok(2));
    assert!(write_snapshot(&engine, 10, 1, first).is_err());
    assert_eq!(read_snapshot(&engine, 11).unwrap().envelope, second);
    for filename in [
        path.clone(),
        std::path::PathBuf::from(format!("{}-wal", path.display())),
    ] {
        let bytes = std::fs::read(filename).unwrap();
        for secret in [
            "Synthetic secret name",
            "synthetic-user@example",
            "Synthetic account password",
            "Synthetic-secret-server:5432",
            "Synthetic-secret-scene",
        ] {
            assert!(
                !bytes.windows(secret.len()).any(|v| v == secret.as_bytes()),
                "plaintext in DB/WAL"
            );
        }
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM items", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let final_envelope = s.encrypt().unwrap();
    engine
        .send(Command::WriteAccountVault {
            request_id: 12,
            expected_revision: 2,
            envelope: final_envelope.clone(),
        })
        .unwrap();
    engine.send(Command::Shutdown).unwrap();
    assert!(matches!(
        wait(&engine, 12),
        Event::AccountVaultWritten { result: Ok(3), .. }
    ));
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if matches!(engine.try_recv(), Ok(Event::ExitReady)) {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(engine);
    drop(db);
    let engine = Engine::start(EngineOptions::new(path), Arc::new(|| {})).unwrap();
    let reopened = read_snapshot(&engine, 13).unwrap();
    assert_eq!(reopened.envelope, final_envelope);
    assert_eq!(reopened.revision, 3);
    assert_eq!(
        VaultSession::unlock(&password(), &reopened)
            .unwrap()
            .credentials("a1")
            .unwrap()
            .1
            .expose(),
        "Synthetic account password"
    );
}
#[test]
fn binding_integrity_removal_and_serialized_document_limit() {
    let mut doc = document();
    let mut b = binding();
    b.account_id = "missing".into();
    assert!(doc.upsert_binding(b).is_err());
    assert!(doc.bindings.is_empty());
    doc.upsert_binding(binding()).unwrap();
    doc.delete_account("a1");
    assert!(doc.accounts.is_empty());
    assert!(doc.bindings.is_empty());
    doc = document();
    doc.upsert_binding(binding()).unwrap();
    doc.bindings.push(binding());
    assert!(doc.validate().is_err());
}
#[test]
fn change_master_and_failed_save_do_not_replace_confirmed_session() {
    let mut c = VaultController::new(Arc::new(|| {}));
    c.accept_create(1, password(), password(), document())
        .unwrap();
    assert_eq!(poll(&mut c), VaultNotice::Prepared);
    assert!(!c.session().is_unlocked());
    c.dispatch_save(|_| Ok(())).unwrap();
    c.handle_event(&Event::AccountVaultWritten {
        request_id: 1,
        result: Ok(1),
    })
    .unwrap();
    let mut doc = document();
    doc.accounts[0].name = "Edited name".into();
    c.accept_save(2, doc).unwrap();
    assert_eq!(poll(&mut c), VaultNotice::Prepared);
    assert_eq!(c.session().summaries()[0].name, "Synthetic secret name");
    c.dispatch_save(|_| Ok(())).unwrap();
    assert!(matches!(
        c.handle_event(&Event::AccountVaultWritten {
            request_id: 2,
            result: Err("合成失败".into())
        }),
        Some(VaultNotice::Failed(_))
    ));
    assert_eq!(
        c.pending_document().unwrap().accounts[0].name,
        "Edited name"
    );
    assert_eq!(c.session().summaries()[0].name, "Synthetic secret name");
    c.retry().unwrap();
    c.dispatch_save(|_| Ok(())).unwrap();
    c.handle_event(&Event::AccountVaultWritten {
        request_id: 2,
        result: Ok(2),
    })
    .unwrap();
    assert_eq!(c.session().summaries()[0].name, "Edited name");
    c.accept_change_master(
        3,
        Secret::new("Replacement master password".into()),
        Secret::new("Replacement master password".into()),
    )
    .unwrap();
    assert_eq!(poll(&mut c), VaultNotice::Prepared);
    let mut envelope = Vec::new();
    c.dispatch_save(|cmd| {
        let Command::WriteAccountVault {
            envelope: bytes, ..
        } = cmd
        else {
            panic!()
        };
        envelope = bytes;
        Ok(())
    })
    .unwrap();
    let s = VaultSnapshot {
        revision: 3,
        envelope,
    };
    assert!(VaultSession::unlock(&password(), &s).is_err());
    assert_eq!(
        VaultSession::unlock(&Secret::new("Replacement master password".into()), &s)
            .unwrap()
            .summaries()[0]
            .name,
        "Edited name"
    );
    c.lock();
    c.handle_event(&Event::AccountVaultWritten {
        request_id: 3,
        result: Ok(3),
    })
    .unwrap();
    assert!(!c.session().is_unlocked());
}
#[test]
fn ciphertext_body_counts_toward_queue_and_packet_budget() {
    let temp = tempfile::tempdir().unwrap();
    let mut options = EngineOptions::new(temp.path().join("vault.sqlite"));
    options.max_pending_bytes = 128;
    let engine = Engine::start(options, Arc::new(|| {})).unwrap();
    let s = VaultSession::create(&password(), &password(), document()).unwrap();
    assert!(
        engine
            .send(Command::WriteAccountVault {
                request_id: 1,
                expected_revision: 0,
                envelope: s.encrypt().unwrap()
            })
            .is_err()
    );
    assert!(
        engine
            .send(Command::WriteAccountVault {
                request_id: 2,
                expected_revision: 0,
                envelope: vec![0; MAX_ENVELOPE_BYTES + 1]
            })
            .is_err()
    );
    assert!(read_snapshot(&engine, 3).is_none());
}
#[test]
fn actor_rejects_corrupt_envelope_on_read() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault.sqlite");
    let engine = Engine::start(EngineOptions::new(path.clone()), Arc::new(|| {})).unwrap();
    read_snapshot(&engine, 1);
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE account_vault(id INTEGER PRIMARY KEY,revision INTEGER,envelope BLOB); INSERT INTO account_vault VALUES(1,1,zeroblob(4194305));").unwrap();
    engine
        .send(Command::ReadAccountVault { request_id: 2 })
        .unwrap();
    assert!(matches!(
        wait(&engine, 2),
        Event::AccountVaultLoaded { result: Err(_), .. }
    ));
    db.execute(
        "UPDATE account_vault SET envelope=?1",
        [b"ordinary plaintext".as_slice()],
    )
    .unwrap();
    engine
        .send(Command::ReadAccountVault { request_id: 3 })
        .unwrap();
    assert!(matches!(
        wait(&engine, 3),
        Event::AccountVaultLoaded { result: Err(_), .. }
    ));
}
#[test]
fn configured_idle_lock_clears_session_and_changes_epoch() {
    let mut c = VaultController::new(Arc::new(|| {}));
    c.accept_create(1, password(), password(), document())
        .unwrap();
    poll(&mut c);
    c.dispatch_save(|_| Ok(())).unwrap();
    c.handle_event(&Event::AccountVaultWritten {
        request_id: 1,
        result: Ok(1),
    })
    .unwrap();
    let settings = quick_panel::account_fill::AccountFillSettings {
        enabled: false,
        show_hint: false,
        lock_minutes: 1,
    };
    assert!(!c.lock_if_idle(&settings, Duration::from_secs(59)).unwrap());
    assert!(c.session().is_unlocked());
    let epoch = c.session().epoch();
    assert!(c.lock_if_idle(&settings, Duration::from_secs(60)).unwrap());
    assert!(!c.session().is_unlocked());
    assert!(c.session().summaries().is_empty());
    assert!(c.session().epoch() > epoch);
}

#[test]
fn oversized_document_never_becomes_an_unlocked_session() {
    let mut doc = document();
    for n in 0..2048 {
        let mut b = binding();
        b.binding_id = n.to_string();
        b.scene_key = "s".repeat(1024);
        b.target.as_mut().unwrap().expected = "t".repeat(1024);
        doc.bindings.push(b);
    }
    assert!(VaultSession::create(&password(), &password(), doc).is_err());
}
#[test]
fn overallocated_ciphertext_is_rejected_synchronously_without_losing_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let mut options = EngineOptions::new(temp.path().join("vault.sqlite"));
    options.max_pending_bytes = 4096;
    options.max_item_bytes = 1024;
    let engine = Engine::start(options, Arc::new(|| {})).unwrap();
    let mut envelope = Vec::with_capacity(2048);
    envelope.extend(b"QPVAULT\0");
    for n in [1u32, 2, 19, 65536, 3, 1, 32] {
        envelope.extend(n.to_le_bytes());
    }
    envelope.extend([0; 40]);
    envelope.extend(20u32.to_le_bytes());
    envelope.extend([0; 20]);
    assert!(
        engine
            .send(Command::WriteAccountVault {
                request_id: 1,
                expected_revision: 0,
                envelope
            })
            .is_err()
    );
}
#[test]
fn cas_conflict_explicit_discard_reloads_and_revokes_old_fill_epoch() {
    use quick_panel::account_fill::*;
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        EngineOptions::new(temp.path().join("vault.sqlite")),
        Arc::new(|| {}),
    )
    .unwrap();
    let mut c = VaultController::new(Arc::new(|| {}));
    c.accept_create(1, password(), password(), document())
        .unwrap();
    poll(&mut c);
    assert!(c.discard_failed_save().is_err());
    c.dispatch_save(|cmd| engine.send(cmd)).unwrap();
    c.handle_event(&wait(&engine, 1)).unwrap();
    let settings = AccountFillSettings {
        enabled: true,
        ..Default::default()
    };
    let mut b = binding();
    b.target = None;
    let scene = SceneSnapshot {
        complete: true,
        scope: quick_panel::account_fill::SceneScope::Window,
        app: b.app.clone(),
        window: WindowIdentity {
            handle: 1,
            pid: 2,
            process_start: 3,
        },
        scene_key: b.scene_key.clone(),
        origin: SceneOrigin::NativeDesktop,
        fields: vec![LiveField {
            field_id: 1,
            selector: b.password.clone(),
            depth: 1,
            is_password: true,
            visible: true,
            enabled: true,
            writable: true,
        }],
        targets: vec![],
        node_count: 1,
        max_depth: 1,
    };
    let mut gate = FillGate::default();
    let ticket = gate
        .begin(
            UserFillAction::SelectedAccount,
            &settings,
            c.session().epoch(),
            true,
            &b,
            &scene,
        )
        .unwrap();
    let mut other = document();
    other.accounts[0].name = "Externally updated".into();
    let external = VaultSession::create(&password(), &password(), other).unwrap();
    assert_eq!(
        write_snapshot(&engine, 20, 1, external.encrypt().unwrap()),
        Ok(2)
    );
    c.accept_save(2, document()).unwrap();
    poll(&mut c);
    c.dispatch_save(|cmd| engine.send(cmd)).unwrap();
    assert!(matches!(
        c.handle_event(&wait(&engine, 2)),
        Some(VaultNotice::Failed(_))
    ));
    assert_eq!(c.state(), SaveState::FailedWrite);
    assert!(!c.can_shutdown());
    let old_epoch = c.session().epoch();
    c.discard_failed_save().unwrap();
    assert!(c.can_shutdown());
    assert!(!c.session().is_unlocked());
    assert!(c.session().credentials("a1").is_err());
    assert!(c.session().epoch() > old_epoch);
    let snapshot = read_snapshot(&engine, 21).unwrap();
    c.begin_unlock(c.session().epoch(), password(), snapshot)
        .unwrap();
    assert_eq!(poll(&mut c), VaultNotice::Unlocked);
    assert_eq!(c.session().summaries()[0].name, "Externally updated");
    assert!(
        gate.validate(
            &ticket,
            &settings,
            c.session().epoch(),
            c.session().is_unlocked(),
            &scene
        )
        .is_err()
    );
}
