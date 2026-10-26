//! Original startup coordinator and inspection, using only isolated synthetic owners.
use super::*;
use crate::secrets::{
    key_store::{KeyStore, KeyStoreError},
    protection::ProtectionRecord,
    storage::StorageContext,
    testing::TestHome,
    VaultContext,
};
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Cache {
    loads: AtomicUsize,
    saves: AtomicUsize,
}
impl KeyStore for Cache {
    fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        Err(KeyStoreError::Unavailable)
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        panic!("startup cannot remove a key")
    }
}
fn scope() -> (PathBuf, crate::live::engine::DeviceStore) {
    (
        crate::config::get_app_config_dir(),
        crate::live::engine::DeviceStore::for_device(),
    )
}
fn coordinator(root: &Path, device: &crate::live::engine::DeviceStore) -> StartupCoordinator {
    StartupCoordinator::new(
        root.into(),
        crate::secrets::upgrade::inspect(root, device).unwrap(),
    )
}
fn save_record(root: &Path, record: &ProtectionRecord) {
    crate::config::ensure_private_directory(root).unwrap();
    crate::config_file_io::write_durable(
        &root.join("protection.json"),
        &serde_json::to_vec(record).unwrap(),
    )
    .unwrap();
}
fn encrypted_record(vault: &VaultContext) -> ProtectionRecord {
    ProtectionRecord::encrypted(vault, None).unwrap()
}
fn save_vault(root: &Path, vault: &VaultContext) {
    crate::config::ensure_private_directory(root).unwrap();
    crate::secrets::session::write_metadata(
        root,
        &crate::secrets::session::completed_metadata(vault, false).unwrap(),
    )
    .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn production_fresh_entry_uses_plaintext_onboarding_without_random_master_or_cache() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    let startup = coordinator(&root, &device);
    let calls = AtomicUsize::new(0);
    startup
        .initialize_with(None, &cache, |session| {
            assert!(session.read().unwrap().is_plaintext());
            crate::settings::unlock_settings_for_test(session.clone()).unwrap();
            let db = crate::database::Database::init_with_secrets(session).unwrap();
            assert!(
                crate::database::vault::stored_metadata(&db.conn.lock().unwrap())
                    .unwrap()
                    .is_none()
            );
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
    assert!(!root.join("vault.json").exists());
    let record = crate::secrets::protection::read(&root).unwrap().unwrap();
    assert!(!record.choice_complete);
    assert!(!record.should_prompt(false));
    assert!(record.should_prompt(true));
    startup
        .initialize_with(None, &cache, |_| panic!("runtime must publish once"))
        .unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn production_plaintext_restart_validates_original_database_and_device_owners() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    let session = SecretSession::open_protected(&root, device.root(), &cache, None).unwrap();
    let context = session.read().unwrap().clone();
    let db = crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &context)
        .unwrap();
    drop(db);
    let file = crate::secrets::owned_file::DeviceFile::registered("live-state.json").unwrap();
    let state = file
        .encode_storage(&context, br#"{"version":1,"apps":{}}"#)
        .unwrap();
    crate::config::ensure_private_directory(device.root()).unwrap();
    std::fs::write(device.state_path(), &state).unwrap();
    let before = std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap();
    let startup = coordinator(&root, &device);
    startup
        .initialize_with(None, &cache, |opened| {
            assert_eq!(opened.read().unwrap().identity(), context.identity());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap(),
        before
    );
    assert_eq!(std::fs::read(device.state_path()).unwrap(), state);
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn passive_inspection_rejects_malformed_missing_field_and_mismatched_protection_records() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    crate::config::ensure_private_directory(&root).unwrap();
    for bytes in [
        b"{".as_slice(),
        br#"{"formatVersion":1,"choiceComplete":false}"#.as_slice(),
        br#"{"formatVersion":1,"choiceComplete":false,"storage":{"mode":"unknown"}}"#.as_slice(),
    ] {
        std::fs::write(root.join("protection.json"), bytes).unwrap();
        assert!(crate::secrets::upgrade::inspect(&root, &device).is_err());
        assert_eq!(std::fs::read(root.join("protection.json")).unwrap(), bytes);
    }
    let vault = VaultContext::generate().unwrap();
    save_vault(&root, &vault);
    let wrong = VaultContext::generate().unwrap();
    save_record(&root, &encrypted_record(&wrong));
    assert!(crate::secrets::upgrade::inspect(&root, &device).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn protection_record_exact_preimage_invalidates_the_original_review_token() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    let vault = VaultContext::generate()
        .unwrap()
        .with_password("synthetic legacy password")
        .unwrap();
    save_vault(&root, &vault);
    save_record(&root, &encrypted_record(&vault));
    let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
    crate::database::Database::create_tables_on_conn(&conn).unwrap();
    crate::database::Database::apply_upgrade_source_migrations_on_conn(&conn).unwrap();
    crate::database::loongport_schema::apply(&conn).unwrap();
    crate::database::vault::stamp(&conn, &vault).unwrap();
    drop(conn);
    let inspected = crate::secrets::upgrade::inspect(&root, &device).unwrap();
    let review = crate::secrets::upgrade::AuthenticatedUpgrade::authenticate(
        &root,
        &device,
        &inspected,
        &cache,
        Some("synthetic legacy password"),
    )
    .unwrap();
    assert!(review.view(&inspected).unwrap().review_token.is_some());
    let mut changed = std::fs::read(root.join("protection.json")).unwrap();
    changed.push(b'\n');
    std::fs::write(root.join("protection.json"), &changed).unwrap();
    assert!(inspected.verify_unchanged(&root).is_err());
    assert!(review.view(&inspected).is_err());
    assert_eq!(
        std::fs::read(root.join("protection.json")).unwrap(),
        changed
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn unknown_existing_database_never_reaches_fresh_or_random_key_setup() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    crate::config::ensure_private_directory(&root).unwrap();
    let cache = Cache::default();
    let path = root.join(crate::config::DB_FILE_NAME);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE unknown_payload(value TEXT); INSERT INTO unknown_payload VALUES('synthetic-old-data');").unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    let startup = coordinator(&root, &device);
    assert!(startup
        .initialize_with(None, &cache, |_| panic!(
            "unknown data cannot start runtime"
        ))
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
    assert!(!root.join("vault.json").exists());
    assert!(!root.join("protection.json").exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn internal_master_startup_reopens_the_text_vault_without_legacy_password_or_cache() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    let vault = VaultContext::create_generation_with_user_text(
        uuid::Uuid::new_v4().to_string(),
        1,
        "synthetic 文本 key",
    )
    .unwrap();
    save_vault(&root, &vault);
    save_record(&root, &encrypted_record(&vault));
    let before = std::fs::read(root.join("vault.json")).unwrap();
    let startup = coordinator(&root, &device);
    startup
        .initialize_with_internal_key(None, Some(vault.export_key()), &cache, |session| {
            assert_eq!(
                session.read().unwrap().identity(),
                StorageContext::Encrypted(vault.clone()).identity()
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_wrapped_password_remains_legacy_authentication_and_never_plaintext_fallback() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    let vault = VaultContext::generate()
        .unwrap()
        .with_password("synthetic legacy password")
        .unwrap();
    save_vault(&root, &vault);
    let before = std::fs::read(root.join("vault.json")).unwrap();
    let startup = coordinator(&root, &device);
    assert!(startup
        .initialize_with(Some("wrong legacy password"), &cache, |_| panic!(
            "bad password cannot publish"
        ))
        .is_err());
    startup
        .initialize_with(Some("synthetic legacy password"), &cache, |session| {
            assert!(!session.read().unwrap().is_plaintext());
            Ok(())
        })
        .unwrap();
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
    assert!(!root.join("protection.json").exists());
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn text_descriptor_without_protection_record_is_missing_evidence_not_legacy() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let vault = VaultContext::create_generation_with_user_text(
        uuid::Uuid::new_v4().to_string(),
        1,
        "synthetic missing record",
    )
    .unwrap();
    save_vault(&root, &vault);
    let before = std::fs::read(root.join("vault.json")).unwrap();
    assert!(crate::secrets::upgrade::inspect(&root, &device).is_err());
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
    assert!(!root.join("protection.json").exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn external_protection_insert_after_inspection_is_not_acknowledged_or_overwritten() {
    let _home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    let startup = coordinator(&root, &device);
    let published = crate::secrets::protection::FreshScope::capture(&root, device.root(), &[])
        .unwrap()
        .publish_initial_plaintext()
        .unwrap();
    let before = std::fs::read(root.join("protection.json")).unwrap();
    assert!(startup
        .initialize_with(None, &cache, |_| panic!(
            "uninspected record cannot publish runtime"
        ))
        .is_err());
    assert_eq!(
        crate::secrets::protection::read(&root).unwrap().unwrap(),
        published
    );
    assert_eq!(std::fs::read(root.join("protection.json")).unwrap(), before);
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
    assert!(!root.join("vault.json").exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn external_protection_replacement_after_inspection_preserves_the_foreign_bytes() {
    let home = TestHome::new().unwrap();
    let (root, device) = scope();
    let cache = Cache::default();
    crate::secrets::protection::FreshScope::capture(&root, device.root(), &[])
        .unwrap()
        .publish_initial_plaintext()
        .unwrap();
    let startup = coordinator(&root, &device);
    let alternate = home.path().join("synthetic-alternate-owner");
    let replacement = crate::secrets::protection::FreshScope::capture(&alternate, &alternate, &[])
        .unwrap()
        .publish_initial_plaintext()
        .unwrap();
    save_record(&root, &replacement);
    let before = std::fs::read(root.join("protection.json")).unwrap();
    assert!(startup
        .initialize_with(None, &cache, |_| panic!(
            "changed record cannot publish runtime"
        ))
        .is_err());
    assert_eq!(std::fs::read(root.join("protection.json")).unwrap(), before);
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(cache.saves.load(Ordering::SeqCst), 0);
    assert!(!root.join("vault.json").exists());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "record_insert_race" => {
            external_protection_insert_after_inspection_is_not_acknowledged_or_overwritten()
        }
        "record_replace_race" => {
            external_protection_replacement_after_inspection_preserves_the_foreign_bytes()
        }
        "fresh" => {
            production_fresh_entry_uses_plaintext_onboarding_without_random_master_or_cache()
        }
        "plaintext_restart" => {
            production_plaintext_restart_validates_original_database_and_device_owners()
        }
        "record_strict" => {
            passive_inspection_rejects_malformed_missing_field_and_mismatched_protection_records()
        }
        "record_preimage" => {
            protection_record_exact_preimage_invalidates_the_original_review_token()
        }
        "unknown" => unknown_existing_database_never_reaches_fresh_or_random_key_setup(),
        "internal_master" => {
            internal_master_startup_reopens_the_text_vault_without_legacy_password_or_cache()
        }
        "legacy_password" => {
            legacy_wrapped_password_remains_legacy_authentication_and_never_plaintext_fallback()
        }
        "text_record_missing" => {
            text_descriptor_without_protection_record_is_missing_evidence_not_legacy()
        }
        _ => panic!("unknown production startup protection case"),
    }
}
