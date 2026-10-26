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
        commitment: None, cleanup: None, format_version: 1,
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
