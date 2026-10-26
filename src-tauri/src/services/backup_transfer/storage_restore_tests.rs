//! Explicit local SQL restore through the original service and database owners.
//! All roots, inputs and keys are synthetic; any key-store access is a failure.
use super::*;
use crate::secrets::{
    files::CredentialFile,
    key_store::KeyStoreError,
    protection::{ProtectionRecord, ProtectionStorage},
    session::{completed_metadata, write_metadata, SecretSession},
    storage::{PlaintextIdentity, StorageContext},
    testing::TestHome,
    VaultContext,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const SOURCE_TEXT: &str = "  synthetic source 文本\n key  ";
const CURRENT_TEXT: &str = "synthetic current text key";
const BEFORE: &str = "local-before-restore-canary";
const AFTER: &str = "imported-sql-content-canary";
const CREDENTIAL: &[u8] = br#"{"token":"local-credential-stays-byte-identical"}"#;

struct NoStore;
impl KeyStore for NoStore {
    fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        panic!("same-generation local SQL restore must not read system keys")
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
        panic!("same-generation local SQL restore must not save system keys")
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        panic!("same-generation local SQL restore must not remove system keys")
    }
}

fn context(encrypted: bool, source: bool) -> StorageContext {
    if encrypted {
        VaultContext::create_generation_with_user_text(
            if source {
                "33333333-3333-4333-8333-333333333333"
            } else {
                "11111111-1111-4111-8111-111111111111"
            }
            .into(),
            1,
            if source { SOURCE_TEXT } else { CURRENT_TEXT },
        )
        .unwrap()
        .into()
    } else {
        StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: if source {
                "33333333-3333-4333-8333-333333333333"
            } else {
                "11111111-1111-4111-8111-111111111111"
            }
            .into(),
            generation_id: if source {
                "44444444-4444-4444-8444-444444444444"
            } else {
                "22222222-2222-4222-8222-222222222222"
            }
            .into(),
            revision: 1,
        })
    }
}
fn database(root: &Path, context: StorageContext, unlock: bool) -> Database {
    let connection =
        crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &context)
            .unwrap();
    let record = match &context {
        StorageContext::Encrypted(vault) => {
            write_metadata(root, &completed_metadata(vault, false).unwrap()).unwrap();
            ProtectionRecord::encrypted(vault, None).unwrap()
        }
        StorageContext::Plaintext(identity) => ProtectionRecord {
            format_version: 1,
            choice_complete: true,
            storage: ProtectionStorage::Plaintext {
                identity: identity.clone(),
            },
            commitment: None,
            cleanup: None,
        },
    };
    std::fs::write(
        root.join(crate::secrets::protection::FILE),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let session = SecretSession::from_storage_context(root.to_path_buf(), context);
    if unlock {
        crate::settings::unlock_settings_for_test(session.clone()).unwrap();
    }
    Database::from_connection(connection, session)
}
fn target(home: &TestHome, encrypted: bool) -> Database {
    let root = crate::config::get_app_config_dir();
    assert!(root.starts_with(home.path()));
    let db = database(&root, context(encrypted, false), true);
    db.set_setting("common_config_claude", BEFORE).unwrap();
    CredentialFile::Codex
        .write(&db.secrets, CREDENTIAL)
        .unwrap();
    std::fs::create_dir_all(root.join("skills/local")).unwrap();
    std::fs::write(root.join("skills/local/SKILL.md"), b"local skill unchanged").unwrap();
    db
}
fn exported_source(home: &TestHome, encrypted: bool) -> String {
    let source = database(&home.path().join("source"), context(encrypted, true), false);
    source.set_setting("common_config_claude", AFTER).unwrap();
    let path = home.path().join("explicit-local-backup.sql");
    source.export_sql(&path).unwrap();
    std::fs::read_to_string(path).unwrap()
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<[u8; 32]>> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(!entry.file_type().is_symlink());
            let digest = if entry.file_type().is_file() {
                Some(Sha256::digest(std::fs::read(entry.path()).unwrap()).into())
            } else {
                assert!(entry.file_type().is_dir());
                None
            };
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                digest,
            )
        })
        .collect()
}
fn optional_bytes(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("fixture read failed: {e}"),
    }
}
fn assert_config_code<T>(result: Result<T, AppError>, expected: &str) {
    match result {
        Err(AppError::Config(actual)) => assert_eq!(actual, expected),
        Err(other) => panic!("wrong error: {other}"),
        Ok(_) => panic!("unexpected restore success"),
    }
}
fn successful_restore(encrypted: bool) {
    let home = TestHome::new().unwrap();
    let db = target(&home, encrypted);
    let sql = exported_source(&home, encrypted);
    let original = db.secrets.read().unwrap().clone();
    let root = db.secrets.root();
    let metadata = optional_bytes(&root.join("vault.json"));
    let record = std::fs::read(root.join(crate::secrets::protection::FILE)).unwrap();
    let credentials = std::fs::read(root.join("codex_oauth_auth.json")).unwrap();
    let _sync = crate::services::sync_protocol::sync_mutex().blocking_lock();
    let backup_id = restore_sql(&db, &sql, encrypted.then_some(SOURCE_TEXT), &NoStore).unwrap();
    assert!(!backup_id.is_empty(), "original safety backup is required");
    assert_eq!(
        db.get_setting("common_config_claude").unwrap().as_deref(),
        Some(AFTER)
    );
    assert_eq!(db.secrets.read().unwrap().identity(), original.identity());
    crate::database::storage::check_identity(&db.conn.lock().unwrap(), &original).unwrap();
    assert_eq!(optional_bytes(&root.join("vault.json")), metadata);
    assert_eq!(
        std::fs::read(root.join(crate::secrets::protection::FILE)).unwrap(),
        record
    );
    assert_eq!(
        std::fs::read(root.join("codex_oauth_auth.json")).unwrap(),
        credentials
    );
    assert_eq!(
        &**CredentialFile::Codex.read(&db.secrets).unwrap().unwrap(),
        CREDENTIAL
    );
    assert_eq!(
        std::fs::read(root.join("skills/local/SKILL.md")).unwrap(),
        b"local skill unchanged"
    );
    assert!(!root.join(crate::secrets::transition::INTENT).exists());
    let backup_path = root.join("backups").join(format!("{backup_id}.db"));
    let backup = rusqlite::Connection::open_with_flags(
        &backup_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    crate::database::storage::check_identity(&backup, &original).unwrap();
    crate::secrets::inventory::validate_storage_database(&backup, &original).unwrap();
    let backup_db = Database::from_connection(
        backup,
        SecretSession::from_storage_context(root.to_path_buf(), original),
    );
    assert_eq!(
        backup_db
            .get_setting("common_config_claude")
            .unwrap()
            .as_deref(),
        Some(BEFORE)
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_plain_sql_restore_keeps_plain_identity_and_original_safety_backup() {
    successful_restore(false);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_user_text_sql_restore_keeps_current_key_and_original_safety_backup() {
    successful_restore(true);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn wrong_source_text_or_missing_input_changes_no_file_and_creates_no_backup() {
    for encrypted in [false, true] {
        let home = TestHome::new().unwrap();
        let db = target(&home, encrypted);
        let sql = exported_source(&home, true);
        let before = tree(db.secrets.root());
        let digest = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
        let _sync = crate::services::sync_protocol::sync_mutex().blocking_lock();
        for (input, expected) in [
            (None, "backup.password_required"),
            (Some("wrong source text"), "secret.key_rejected"),
        ] {
            assert_config_code(restore_sql(&db, &sql, input, &NoStore), expected);
            assert!(
                tree(db.secrets.root()) == before,
                "rejected source changed target tree"
            );
            assert_eq!(
                Database::content_digest(&db.conn.lock().unwrap()).unwrap(),
                digest
            );
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn malformed_local_sql_is_rejected_before_any_safety_backup() {
    for encrypted in [false, true] {
        let home = TestHome::new().unwrap();
        let db = target(&home, encrypted);
        let before = tree(db.secrets.root());
        let digest = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
        let _sync = crate::services::sync_protocol::sync_mutex().blocking_lock();
        let result = restore_sql(&db, "this is not an exported SQL image", None, &NoStore);
        assert!(
            matches!(
                result,
                Err(AppError::Localized {
                    key: "backup.sql.invalid_format",
                    ..
                })
            ),
            "source format must be checked by original SQL owner"
        );
        assert!(
            tree(db.secrets.root()) == before,
            "invalid SQL changed target tree"
        );
        assert_eq!(
            Database::content_digest(&db.conn.lock().unwrap()).unwrap(),
            digest
        );
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "plain_restore" => {
            explicit_plain_sql_restore_keeps_plain_identity_and_original_safety_backup()
        }
        "text_restore" => {
            explicit_user_text_sql_restore_keeps_current_key_and_original_safety_backup()
        }
        "wrong_input" => wrong_source_text_or_missing_input_changes_no_file_and_creates_no_backup(),
        "invalid_sql" => malformed_local_sql_is_rejected_before_any_safety_backup(),
        _ => panic!("unknown local SQL storage restore case"),
    }
}
