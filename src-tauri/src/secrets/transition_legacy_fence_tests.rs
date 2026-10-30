use super::fence_test_support::{assert_rejected, tree, Fixture, Scope, PASSWORD, SCOPES};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
fn assert_untouched(
    f: &Fixture,
    before: &impl PartialEq<
        std::collections::BTreeMap<PathBuf, (bool, u64, String, Option<std::time::SystemTime>)>,
    >,
) {
    assert!(
        *before == tree(f.home.path()),
        "legacy writer changed synthetic scope"
    );
    assert_eq!(f.store.calls(), 0, "legacy writer touched key store");
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_rotate_protected_scope() {
    for scope in SCOPES {
        let f = Fixture::new();
        f.record(scope);
        let before = tree(f.home.path());
        assert_rejected(rotate(&f.db, &f.store, PASSWORD, true), scope);
        assert_untouched(&f, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_install_protected_scope() {
    for scope in SCOPES {
        for settings in [false, true] {
            let f = Fixture::new();
            let next = f
                .current()
                .rotate_key()
                .unwrap()
                .with_password(PASSWORD)
                .unwrap();
            f.record(scope);
            let before = tree(f.home.path());
            let calls = AtomicUsize::new(0);
            let prepare = |_: &Connection, _: &VaultContext, _: &VaultContext| {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(AppError::Config("test.prepare_called".into()))
            };
            let result = if settings {
                install_generation_with_settings(
                    &f.db,
                    &f.store,
                    next,
                    true,
                    prepare,
                    None,
                    crate::settings::AppSettings::default(),
                )
            } else {
                install_generation(&f.db, &f.store, next, true, prepare, None)
            };
            assert_rejected(result, scope);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_untouched(&f, &before);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_upgrade_protected_scope() {
    for scope in SCOPES {
        let f = Fixture::new();
        f.record(scope);
        let before = tree(f.home.path());
        let calls = AtomicUsize::new(0);
        let prepare = |_: &Connection, _: &VaultContext, _: &VaultContext| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(AppError::Config("test.prepare_called".into()))
        };
        let device = crate::live::engine::DeviceStore::for_device();
        let result = install_upgrade_database(
            &f.db,
            &f.store,
            device.root(),
            prepare,
            &mut |_, _| Err(AppError::Config("test.checkpoint_called".into())),
            &mut |_| Ok(()),
            None,
        );
        assert_rejected(result, scope);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_untouched(&f, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_recover_protected_scope() {
    for scope in SCOPES {
        for inspected in [false, true] {
            let f = Fixture::new();
            let current = f.current();
            let intent = Intent {
                version: FORMAT,
                id: uuid::Uuid::new_v4().to_string(),
                next: super::super::session::completed_metadata(&current, true).unwrap(),
                manifest: "synthetic-unread-manifest".into(),
            };
            let bytes = serde_json::to_vec(&intent).unwrap();
            std::fs::write(f.root().join(INTENT), &bytes).unwrap();
            f.record(scope);
            let before = tree(f.home.path());
            let device = crate::live::engine::DeviceStore::for_device();
            let result = if inspected {
                recover_inspected(f.root(), device.root(), &f.store, None, &bytes)
            } else {
                recover(f.root(), &f.store, None)
            };
            assert_rejected(result, scope);
            assert_untouched(&f, &before);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_plaintext_session_protected_scope() {
    use crate::secrets::{
        session::SecretSession,
        storage::{PlaintextIdentity, StorageContext},
        testing::TestHome,
    };
    let _home = TestHome::new().unwrap();
    let root = crate::config::get_app_config_dir();
    let id = PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    };
    let context = StorageContext::Plaintext(id.clone());
    let conn = crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &context)
        .unwrap();
    let db = Database::from_connection(
        conn,
        SecretSession::from_storage_context(root.clone(), context),
    );
    let record = crate::secrets::protection::ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage: crate::secrets::protection::ProtectionStorage::Plaintext { identity: id },
    };
    std::fs::write(
        root.join("protection.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let store = super::fence_test_support::CountingStore::default();
    let before = tree(_home.path());
    assert_rejected(rotate(&db, &store, PASSWORD, true), Scope::Plaintext);
    assert_eq!(before, tree(_home.path()));
    assert_eq!(store.calls(), 0);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_legacy_no_record_rotates() {
    let f = Fixture::new();
    let old = f.current().metadata().clone();
    rotate(&f.db, &f.store, PASSWORD, false).unwrap();
    let next = f.current();
    assert_ne!(next.metadata().key_id, old.key_id);
    assert_eq!(next.metadata().vault_id, old.vault_id);
    assert_eq!(
        f.db.get_setting("global_proxy_url").unwrap().as_deref(),
        Some("synthetic-fence-value")
    );
    assert!(!f.root().join("protection.json").exists());
    assert!(!f.root().join(INTENT).exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_target_text_without_record() {
    for inspected in [false, true] {
        let f = Fixture::new();
        let next = VaultContext::create_generation_with_user_text(
            f.current().metadata().vault_id.clone(),
            2,
            "synthetic exact text",
        )
        .unwrap()
        .with_password(PASSWORD)
        .unwrap();
        let before;
        let result = if inspected {
            let intent = Intent {
                version: FORMAT,
                id: uuid::Uuid::new_v4().to_string(),
                next: super::super::session::completed_metadata(&next, true).unwrap(),
                manifest: "synthetic-unread-manifest".into(),
            };
            let bytes = serde_json::to_vec(&intent).unwrap();
            std::fs::write(f.root().join(INTENT), &bytes).unwrap();
            before = tree(f.home.path());
            let device = crate::live::engine::DeviceStore::for_device();
            recover_inspected(f.root(), device.root(), &f.store, None, &bytes)
        } else {
            before = tree(f.home.path());
            install_generation(
                &f.db,
                &f.store,
                next,
                true,
                |_, _, _| Err(AppError::Config("test.prepare_called".into())),
                None,
            )
        };
        assert_rejected(result, Scope::Encrypted);
        assert_untouched(&f, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn transition_current_text_without_record() {
    let f = Fixture::with_user_text();
    let before = tree(f.home.path());
    let device = crate::live::engine::DeviceStore::for_device();
    assert_rejected(
        install_upgrade_database(
            &f.db,
            &f.store,
            device.root(),
            |_, _, _| Err(AppError::Config("test.prepare_called".into())),
            &mut |_, _| Err(AppError::Config("test.checkpoint_called".into())),
            &mut |_| Ok(()),
            None,
        ),
        Scope::Encrypted,
    );
    assert_untouched(&f, &before);
}
#[cfg(feature = "test-hooks")]
pub(crate) fn verify(case: &str) {
    match case {
        "transition_current_text_without_record" => transition_current_text_without_record(),
        "transition_target_text_without_record" => transition_target_text_without_record(),
        "transition_rotate_protected_scope" => transition_rotate_protected_scope(),
        "transition_install_protected_scope" => transition_install_protected_scope(),
        "transition_upgrade_protected_scope" => transition_upgrade_protected_scope(),
        "transition_recover_protected_scope" => transition_recover_protected_scope(),
        "transition_plaintext_session_protected_scope" => {
            transition_plaintext_session_protected_scope()
        }
        "transition_legacy_no_record_rotates" => transition_legacy_no_record_rotates(),
        case if case.starts_with("rewrap_") => {
            crate::secrets::rewrap::verify_legacy_generation_fence_case(case)
        }
        case => crate::secrets::bootstrap_restore::verify_legacy_generation_fence_case(case),
    }
}

#[test]
#[serial_test::serial]
fn legacy_native_pending_corrupt_and_wrong_key_refuse_before_writes() {
    use crate::secrets::storage::StorageContext;
    use crate::workbuddy::native_owner::testing;
    for operation in ["rotate", "install", "settings", "upgrade"] {
        for receipt in ["pending", "corrupt", "wrong-key"] {
            let f = Fixture::new();
            let client = crate::secrets::testing::tempdir().unwrap();
            let storage = StorageContext::Encrypted(if receipt == "wrong-key" {
                VaultContext::generate().unwrap()
            } else {
                f.current()
            });
            let (pending, _, terminal) = testing::phase_receipts(f.root(), &storage, client.path());
            let bytes = match receipt {
                "pending" => pending,
                "corrupt" => b"corrupt synthetic native receipt".to_vec(),
                _ => terminal,
            };
            std::fs::write(
                f.root()
                    .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE),
                bytes,
            )
            .unwrap();
            let before = tree(f.home.path());
            for _ in 0..2 {
                let prepare = |_: &Connection, _: &VaultContext, _: &VaultContext| {
                    panic!("native admission must precede database preparation")
                };
                let next = f
                    .current()
                    .rotate_key()
                    .unwrap()
                    .with_password(PASSWORD)
                    .unwrap();
                let result = match operation {
                    "rotate" => rotate(&f.db, &f.store, PASSWORD, false),
                    "install" => install_generation(&f.db, &f.store, next, false, prepare, None),
                    "settings" => install_generation_with_settings(
                        &f.db,
                        &f.store,
                        next,
                        false,
                        prepare,
                        None,
                        crate::settings::AppSettings::default(),
                    ),
                    _ => install_upgrade_database(
                        &f.db,
                        &f.store,
                        crate::live::engine::DeviceStore::for_device().root(),
                        prepare,
                        &mut |_, _| panic!("checkpoint must not run"),
                        &mut |_| panic!("staging must not run"),
                        None,
                    ),
                };
                assert!(
                    matches!(result, Err(AppError::Config(ref code)) if code == "secret.recovery_required"),
                    "{operation}/{receipt}: {result:?}"
                );
                assert_untouched(&f, &before);
                f.db.secrets.ensure_available().unwrap();
            }
        }
    }
}

#[test]
#[serial_test::serial]
fn legacy_terminal_native_receipts_authenticate_after_repeated_rotation() {
    use crate::secrets::storage::StorageContext;
    use crate::workbuddy::native_owner::{inspect, testing, Disposition};
    for restored in [false, true] {
        let f = Fixture::new();
        let client = crate::secrets::testing::tempdir().unwrap();
        let storage = StorageContext::Encrypted(f.current());
        let (_, r, c) = testing::phase_receipts(f.root(), &storage, client.path());
        std::fs::write(
            f.root()
                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE),
            if restored { r } else { c },
        )
        .unwrap();
        for _ in 0..2 {
            rotate(&f.db, &f.store, PASSWORD, false).unwrap();
            assert_eq!(
                inspect(f.root(), &StorageContext::Encrypted(f.current())).unwrap(),
                Disposition::Clear
            );
            assert_eq!(
                f.db.get_setting("global_proxy_url").unwrap().as_deref(),
                Some("synthetic-fence-value")
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn legacy_foreign_bootstrap_intent_refuses_before_writes() {
    let f = Fixture::new();
    std::fs::write(
        f.root().join(".vault-bootstrap"),
        b"foreign synthetic intent",
    )
    .unwrap();
    let before = tree(f.home.path());
    assert!(rotate(&f.db, &f.store, PASSWORD, false).is_err());
    assert_untouched(&f, &before);
}

#[test]
#[serial_test::serial]
fn legacy_native_phase_or_epoch_change_at_staged_refuses_publication() {
    use crate::secrets::storage::StorageContext;
    use crate::workbuddy::native_owner::testing;
    for changed in ["phase", "epoch"] {
        let f = Fixture::new();
        let client = crate::secrets::testing::tempdir().unwrap();
        let (pending, _, terminal) = testing::phase_receipts(
            f.root(),
            &StorageContext::Encrypted(f.current()),
            client.path(),
        );
        let journal = f
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
        std::fs::write(&journal, terminal).unwrap();
        let database = std::fs::read(f.root().join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(f.root().join("vault.json")).unwrap();
        let result = rotate_with_hook(&f.db, &f.store, PASSWORD, false, &mut |point| {
            if point == Checkpoint::Staged {
                if changed == "phase" {
                    std::fs::write(&journal, &pending).unwrap();
                } else {
                    f.db.secrets.invalidate_database_leases();
                }
            }
            Ok(())
        });
        assert!(result.is_err(), "{changed} changed after initial admission");
        assert_eq!(
            std::fs::read(f.root().join(crate::config::DB_FILE_NAME)).unwrap(),
            database
        );
        assert_eq!(
            std::fs::read(f.root().join("vault.json")).unwrap(),
            metadata
        );
        assert!(pending_record(f.root()).unwrap().is_none());
        assert_eq!(f.store.calls(), 0);
    }
}

#[test]
#[serial_test::serial]
fn legacy_clear_native_receipt_authenticates_after_rotation() {
    use crate::secrets::storage::StorageContext;
    use crate::workbuddy::native_owner::{inspect, testing, Disposition};
    let f = Fixture::new();
    testing::clear_receipt(f.root(), &StorageContext::Encrypted(f.current()));
    for _ in 0..2 {
        rotate(&f.db, &f.store, PASSWORD, false).unwrap();
        assert_eq!(
            inspect(f.root(), &StorageContext::Encrypted(f.current())).unwrap(),
            Disposition::Clear
        );
    }
}

#[test]
#[serial_test::serial]
fn legacy_session_database_context_mismatch_refuses_without_writes() {
    let f = Fixture::new();
    vault::stamp(
        &f.db.conn.lock().unwrap(),
        &VaultContext::generate().unwrap(),
    )
    .unwrap();
    let before = tree(f.home.path());
    assert!(rotate(&f.db, &f.store, PASSWORD, false).is_err());
    assert_untouched(&f, &before);
}

#[test]
#[serial_test::serial]
fn legacy_native_or_epoch_change_after_intent_keeps_database_unpublished() {
    use crate::secrets::storage::StorageContext;
    for phase_change in [false, true] {
        let f = Fixture::new();
        let client = crate::secrets::testing::tempdir().unwrap();
        let (pending, _, _) = crate::workbuddy::native_owner::testing::phase_receipts(
            f.root(),
            &StorageContext::Encrypted(f.current()),
            client.path(),
        );
        let database = std::fs::read(f.root().join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(f.root().join("vault.json")).unwrap();
        assert!(
            rotate_with_hook(&f.db, &f.store, PASSWORD, false, &mut |point| {
                if point == Checkpoint::Intent {
                    if phase_change {
                        std::fs::write(
                            f.root()
                                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE),
                            &pending,
                        )
                        .unwrap();
                    } else {
                        f.db.secrets.invalidate_database_leases();
                    }
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            database,
            std::fs::read(f.root().join(crate::config::DB_FILE_NAME)).unwrap()
        );
        assert_eq!(
            metadata,
            std::fs::read(f.root().join("vault.json")).unwrap()
        );
        assert!(pending_record(f.root()).unwrap().is_some());
        assert_eq!(f.store.calls(), 0);
        if phase_change {
            let before = tree(f.home.path());
            assert!(recover(f.root(), &f.store, Some(PASSWORD)).is_err());
            assert_eq!(before, tree(f.home.path()));
        }
    }
}

#[test]
#[serial_test::serial]
fn legacy_terminal_native_receipts_survive_original_interrupted_recovery() {
    use crate::secrets::storage::StorageContext;
    use crate::workbuddy::native_owner::{inspect, testing, Disposition};
    for point in [
        Checkpoint::Intent,
        Checkpoint::Database,
        Checkpoint::Artifact(1),
        Checkpoint::Metadata,
    ] {
        let f = Fixture::new();
        let client = crate::secrets::testing::tempdir().unwrap();
        testing::phase_receipts(
            f.root(),
            &StorageContext::Encrypted(f.current()),
            client.path(),
        );
        assert!(
            rotate_with_hook(&f.db, &f.store, PASSWORD, false, &mut |at| {
                if at == point {
                    Err(AppError::Config("synthetic interruption".into()))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let intent: Intent =
            serde_json::from_slice(&pending_record(f.root()).unwrap().unwrap()).unwrap();
        let next = VaultContext::from_password(intent.next.metadata, PASSWORD).unwrap();
        recover(f.root(), &f.store, Some(PASSWORD)).unwrap();
        assert_eq!(
            inspect(f.root(), &StorageContext::Encrypted(next)).unwrap(),
            Disposition::Clear
        );
        assert!(pending_record(f.root()).unwrap().is_none());
    }
}
