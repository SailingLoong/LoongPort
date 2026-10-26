//! Actual original session and SQLite owners; all paths and keys are synthetic.
use super::super::{
    key_store::KeyStoreError, protection::ProtectionRecord, storage::StorageContext,
    testing::TestHome,
};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use zeroize::Zeroizing;
struct Cache {
    key: Option<Vec<u8>>,
    denied: bool,
    loads: AtomicUsize,
}
impl Cache {
    fn empty() -> Self {
        Self {
            key: None,
            denied: false,
            loads: AtomicUsize::new(0),
        }
    }
}
impl KeyStore for Cache {
    fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.denied {
            Err(KeyStoreError::Unavailable)
        } else {
            Ok(self.key.clone().map(Zeroizing::new))
        }
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
        panic!("opening cannot create or replace a system key")
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        panic!("opening cannot remove a system key")
    }
}
fn encrypted(root: &Path, record: bool) -> VaultContext {
    crate::config::ensure_private_directory(root).unwrap();
    let vault = VaultContext::create_with_user_key(Zeroizing::new(vec![41; 32])).unwrap();
    write_metadata(root, &completed_metadata(&vault, true).unwrap()).unwrap();
    if record {
        write_durable(
            &root.join(super::super::protection::FILE),
            &serde_json::to_vec(&ProtectionRecord::encrypted(&vault, None).unwrap()).unwrap(),
        )
        .unwrap();
    }
    vault
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn fresh_session_is_real_plaintext_and_can_initialize_the_original_database() {
    let _home = TestHome::new().unwrap();
    let root = crate::config::get_app_config_dir();
    let device = crate::live::engine::DeviceStore::for_device();
    let cache = Cache::empty();
    let session = SecretSession::open_protected(&root, device.root(), &cache, None).unwrap();
    assert!(session.read().unwrap().is_plaintext());
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    crate::settings::unlock_settings_for_test(session.clone()).unwrap();
    let db = crate::database::Database::init_with_secrets(session.clone()).unwrap();
    assert!(
        crate::database::vault::stored_metadata(&db.conn.lock().unwrap())
            .unwrap()
            .is_none()
    );
    let record = super::super::protection::read(&root).unwrap().unwrap();
    assert!(!record.choice_complete);
    assert!(!record.should_prompt(false));
    assert!(record.should_prompt(true));
    assert!(!root.join("vault.json").exists());
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    drop(db);
    drop(session);
    assert!(
        SecretSession::open_protected(&root, device.root(), &cache, None)
            .unwrap()
            .read()
            .unwrap()
            .is_plaintext()
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn configured_encryption_cache_none_denied_and_wrong_key_preserve_original_bytes() {
    let home = TestHome::new().unwrap();
    let root = home.path().join("data");
    let vault = encrypted(&root, true);
    let before = std::fs::read(root.join("vault.json")).unwrap();
    let choice = std::fs::read(root.join("protection.json")).unwrap();
    for cache in [
        Cache::empty(),
        Cache {
            denied: true,
            ..Cache::empty()
        },
        Cache {
            key: Some(vec![42; 32]),
            ..Cache::empty()
        },
    ] {
        assert!(SecretSession::open_protected(&root, &root, &cache, None).is_err());
        assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
        assert_eq!(std::fs::read(root.join("protection.json")).unwrap(), choice);
        assert!(!root.join(crate::config::DB_FILE_NAME).exists());
    }
    let cache = Cache {
        key: Some(vault.export_key().to_vec()),
        ..Cache::empty()
    };
    let session = SecretSession::open_protected(&root, &root, &cache, None).unwrap();
    assert_eq!(
        session.read().unwrap().identity(),
        StorageContext::from(vault).identity()
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn manual_original_key_reopens_legacy_and_user_key_vault_without_cache_io() {
    let home = TestHome::new().unwrap();
    for configured in [false, true] {
        let root = home
            .path()
            .join(if configured { "configured" } else { "legacy" });
        let vault = encrypted(&root, configured);
        let cache = Cache {
            denied: true,
            ..Cache::empty()
        };
        let original = std::fs::read(root.join("vault.json")).unwrap();
        let session =
            SecretSession::open_protected(&root, &root, &cache, Some(vault.export_key())).unwrap();
        assert_eq!(
            session.read().unwrap().identity(),
            StorageContext::from(vault).identity()
        );
        assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), original);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn missing_record_with_existing_unknown_database_is_never_fresh_or_random_key_setup() {
    let home = TestHome::new().unwrap();
    let root = home.path().join("data");
    std::fs::create_dir(&root).unwrap();
    let path = root.join(crate::config::DB_FILE_NAME);
    std::fs::write(&path, b"old-data-not-proven-plaintext").unwrap();
    let cache = Cache::empty();
    assert!(SecretSession::open_protected(&root, &root, &cache, None).is_err());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"old-data-not-proven-plaintext"
    );
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert!(!root.join("vault.json").exists());
    assert!(!root.join("protection.json").exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_session_authenticates_versioned_user_text_cache_without_rewriting_anything() {
    let home = TestHome::new().unwrap();
    let root = home.path().join("text");
    crate::config::ensure_private_directory(&root).unwrap();
    let text = " 中文🔑 aBc ";
    let vault =
        VaultContext::create_generation_with_user_text(uuid::Uuid::new_v4().to_string(), 1, text)
            .unwrap();
    write_metadata(&root, &completed_metadata(&vault, true).unwrap()).unwrap();
    let record = ProtectionRecord::encrypted(&vault, None).unwrap();
    write_durable(
        &root.join(super::super::protection::FILE),
        &serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let bytes = super::super::key_store::user_text::encode(text).unwrap();
    let cache = Cache {
        key: Some(bytes.to_vec()),
        ..Cache::empty()
    };
    let before = std::fs::read(root.join("vault.json")).unwrap();
    let choice = std::fs::read(root.join("protection.json")).unwrap();
    let session = SecretSession::open_protected(&root, &root, &cache, None).unwrap();
    assert_eq!(
        session
            .read()
            .unwrap()
            .encrypted()
            .unwrap()
            .export_key()
            .as_slice(),
        vault.export_key().as_slice()
    );
    let legacy_entry = authenticate_existing(&root, &cache, None).unwrap();
    assert_eq!(
        legacy_entry.export_key().as_slice(),
        vault.export_key().as_slice()
    );
    let manual = VaultContext::from_user_text(vault.metadata().clone(), text).unwrap();
    let denied = Cache {
        denied: true,
        ..Cache::empty()
    };
    SecretSession::open_protected(&root, &root, &denied, Some(manual.export_key())).unwrap();
    assert_eq!(denied.loads.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
    assert_eq!(std::fs::read(root.join("protection.json")).unwrap(), choice);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn user_text_metadata_without_its_choice_owner_is_not_adopted_as_legacy() {
    let home = TestHome::new().unwrap();
    let root = home.path().join("missing-choice");
    crate::config::ensure_private_directory(&root).unwrap();
    let vault =
        VaultContext::create_generation_with_user_text(uuid::Uuid::new_v4().to_string(), 1, "x")
            .unwrap();
    write_metadata(&root, &completed_metadata(&vault, true).unwrap()).unwrap();
    let before = std::fs::read(root.join("vault.json")).unwrap();
    let cache = Cache::empty();
    let result = SecretSession::open_protected(&root, &root, &cache, Some(vault.export_key()));
    assert!(matches!(result,Err(AppError::Config(code)) if code=="protection.record_missing"));
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
    assert!(!root.join("protection.json").exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn malformed_migration_state_is_rejected_before_a_protected_session_is_returned() {
    let home = TestHome::new().unwrap();
    let root = home.path().join("corrupt-state");
    let vault = encrypted(&root, true);
    let mut local = read_metadata(&root).unwrap();
    local.migration_state = "lpenc:synthetic-corrupt".into();
    write_metadata(&root, &local).unwrap();
    let before = std::fs::read(root.join("vault.json")).unwrap();
    let cache = Cache::empty();
    assert!(SecretSession::open_protected(&root, &root, &cache, Some(vault.export_key())).is_err());
    assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
    assert_eq!(cache.loads.load(Ordering::SeqCst), 0);
    assert!(!root.join(crate::config::DB_FILE_NAME).exists());
}
#[cfg(feature = "test-hooks")]
pub(crate) fn verify(case: &str) {
    match case {
        "text_cache" => {
            original_session_authenticates_versioned_user_text_cache_without_rewriting_anything()
        }
        "text_missing_record" => {
            user_text_metadata_without_its_choice_owner_is_not_adopted_as_legacy()
        }
        "migration_state" => {
            malformed_migration_state_is_rejected_before_a_protected_session_is_returned()
        }
        "fresh" => fresh_session_is_real_plaintext_and_can_initialize_the_original_database(),
        "cache" => configured_encryption_cache_none_denied_and_wrong_key_preserve_original_bytes(),
        "manual" => manual_original_key_reopens_legacy_and_user_key_vault_without_cache_io(),
        "unknown" => {
            missing_record_with_existing_unknown_database_is_never_fresh_or_random_key_setup()
        }
        _ => panic!("unknown protection startup case"),
    }
}
