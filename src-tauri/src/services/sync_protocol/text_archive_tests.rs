//! Synthetic actual-codec tests; no transport or system credential access.
use super::*;
use crate::database::Database;
use crate::secrets::{
    key_store::{KeyStore, KeyStoreError},
    protection::{ProtectionRecord, ProtectionStorage},
    session::{completed_metadata, write_metadata, SecretSession},
    storage::{PlaintextIdentity, StorageContext},
    testing::TestHome,
    VaultContext,
};
use zeroize::Zeroizing;

const TEXT: &str = "  synthetic archive 文本\n key  ";
const STORE: &str = "11111111-1111-4111-8111-111111111111";
fn text_vault() -> VaultContext {
    VaultContext::create_generation_with_user_text(STORE.into(), 1, TEXT).unwrap()
}
fn manifest(metadata: &VaultMetadata) -> SyncManifest {
    let artifacts = [
        (REMOTE_DB_SQL, b"fixture-sql".as_slice()),
        (REMOTE_SKILLS_ZIP, b"fixture-zip".as_slice()),
    ]
    .into_iter()
    .map(|(name, bytes)| {
        (
            name.into(),
            ArtifactMeta {
                sha256: sha256_hex(bytes),
                size: bytes.len() as u64,
            },
        )
    })
    .collect();
    SyncManifest {
        format: PROTOCOL_FORMAT.into(),
        version: PROTOCOL_VERSION,
        db_compat_version: Some(DB_COMPAT_VERSION),
        device_name: "synthetic".into(),
        created_at: "2026-10-26T00:00:00Z".into(),
        snapshot_id: compute_snapshot_id(&artifacts, metadata),
        artifacts,
        vault: Some(metadata.clone()),
    }
}
fn assert_code<T>(result: Result<T, AppError>, code: &str) {
    match result {
        Err(AppError::Config(actual)) => assert_eq!(actual, code),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("unexpected success: {code}"),
    }
}
fn encrypted_database(home: &TestHome, vault: &VaultContext) -> Database {
    let root = crate::config::get_app_config_dir();
    assert!(root.starts_with(home.path()));
    std::fs::create_dir_all(&root).unwrap();
    let conn =
        crate::database::vault::prepare(&root.join(crate::config::DB_FILE_NAME), vault).unwrap();
    write_metadata(&root, &completed_metadata(vault, false).unwrap()).unwrap();
    let record = ProtectionRecord {
        commitment: None,
        cleanup: None,
        format_version: 1,
        choice_complete: true,
        storage: ProtectionStorage::Encrypted {
            vault_id: vault.metadata().vault_id.clone(),
            key_id: vault.metadata().key_id.clone(),
        },
    };
    std::fs::write(
        root.join(crate::secrets::protection::FILE),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let session = SecretSession::from_context(root, vault.clone());
    crate::settings::unlock_settings_for_test(session.clone()).unwrap();
    Database::from_connection(conn, session)
}
fn downloaded(local: LocalSnapshot) -> DownloadedSnapshot {
    DownloadedSnapshot {
        manifest: serde_json::from_slice(&local.manifest_bytes).unwrap(),
        manifest_hash: local.manifest_hash,
        etag: Some("\"synthetic-version\"".into()),
        db_sql: local.db_sql,
        skills_zip: local.skills_zip,
        layout: RemoteLayout::Current,
        source_path: "synthetic-local-only".into(),
    }
}
struct NoStore;
impl KeyStore for NoStore {
    fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        panic!("archive admission must not read OS/cache keys")
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
        panic!("legacy installer fence must precede key writes")
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        panic!("legacy installer fence must precede key deletion")
    }
}

#[cfg_attr(test, test)]
fn valid_user_text_metadata_is_a_recoverable_manifest_source_without_legacy_wrapper() {
    let vault = text_vault();
    assert!(vault.metadata().wrapped_key.is_none());
    assert!(vault.metadata().user_text_key.is_some());
    validate_manifest_compat(&manifest(vault.metadata()), RemoteLayout::Current).unwrap();
}

#[cfg_attr(test, test)]
fn user_text_authentication_preserves_original_whitespace_and_unicode() {
    let vault = text_vault();
    let sealed = vault
        .seal(
            &["synthetic-archive", "proof"],
            b"synthetic-authenticated-value",
        )
        .unwrap();
    let reopened = authenticate_restore_source(vault.metadata(), TEXT).unwrap();
    assert_eq!(reopened.metadata(), vault.metadata());
    assert_eq!(
        &*reopened
            .open(&["synthetic-archive", "proof"], &sealed)
            .unwrap(),
        b"synthetic-authenticated-value"
    );
    assert!(authenticate_restore_source(vault.metadata(), TEXT.trim()).is_err());
    assert!(authenticate_restore_source(vault.metadata(), "wrong synthetic text").is_err());
}

#[cfg_attr(test, test)]
fn text_descriptor_never_falls_back_to_a_valid_legacy_password_wrapper() {
    let vault = text_vault()
        .with_password("different legacy recovery password")
        .unwrap();
    assert!(vault.metadata().wrapped_key.is_some());
    assert!(vault.metadata().user_text_key.is_some());
    assert!(
        authenticate_restore_source(vault.metadata(), "different legacy recovery password")
            .is_err(),
        "metadata source is text: a matching legacy wrapper is not a fallback"
    );
    let reopened = authenticate_restore_source(vault.metadata(), TEXT).unwrap();
    assert_eq!(reopened.metadata(), vault.metadata());
}

#[cfg_attr(test, test)]
fn legacy_wrapped_password_authentication_remains_exact_and_recoverable() {
    let password = "  legacy synthetic archive password  ";
    let vault = VaultContext::create_with_user_key(Zeroizing::new(vec![31; 32]))
        .unwrap()
        .with_password(password)
        .unwrap();
    assert!(vault.metadata().user_text_key.is_none());
    validate_manifest_compat(&manifest(vault.metadata()), RemoteLayout::Current).unwrap();
    let reopened = authenticate_restore_source(vault.metadata(), password).unwrap();
    assert_eq!(reopened.metadata(), vault.metadata());
    assert!(authenticate_restore_source(vault.metadata(), password.trim()).is_err());
    assert!(authenticate_restore_source(vault.metadata(), "wrong").is_err());
}

#[cfg_attr(test, test)]
fn malformed_descriptor_and_nonrecoverable_metadata_stay_rejected() {
    let vault = text_vault();
    let mut bad = vault.metadata().clone();
    bad.user_text_key.as_mut().unwrap().format_version = 2;
    assert!(validate_manifest_compat(&manifest(&bad), RemoteLayout::Current).is_err());
    assert!(authenticate_restore_source(&bad, TEXT).is_err());
    let legacy = VaultContext::create_with_user_key(Zeroizing::new(vec![32; 32])).unwrap();
    assert_code(
        validate_manifest_compat(&manifest(legacy.metadata()), RemoteLayout::Current),
        "sync.recovery_password_required",
    );
    assert!(authenticate_restore_source(legacy.metadata(), TEXT).is_err());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn original_database_export_and_snapshot_builder_accept_authenticated_text_generation() {
    let home = TestHome::new().unwrap();
    let vault = text_vault();
    let db = encrypted_database(&home, &vault);
    db.set_setting("common_config_claude", "archive-secret-canary")
        .unwrap();
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    let local = build_local_snapshot(&db).unwrap();
    let manifest: SyncManifest = serde_json::from_slice(&local.manifest_bytes).unwrap();
    assert_eq!(manifest.vault_metadata().unwrap(), vault.metadata());
    assert!(!String::from_utf8_lossy(&local.db_sql).contains("archive-secret-canary"));
    validate_manifest_compat(&manifest, RemoteLayout::Current).unwrap();
    let source = authenticate_restore_source(manifest.vault_metadata().unwrap(), TEXT).unwrap();
    Database::validate_sync_snapshot(
        std::str::from_utf8(&local.db_sql).unwrap(),
        manifest.vault_metadata().unwrap(),
        &source,
    )
    .unwrap();
    let _skills =
        crate::services::webdav_sync::archive::stage_skills_zip(&local.skills_zip).unwrap();
    assert_eq!(
        Database::content_digest(&db.conn.lock().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        db.get_setting("common_config_claude").unwrap().as_deref(),
        Some("archive-secret-canary")
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn authenticated_text_restore_rejects_an_unsigned_target_protection_record() {
    let home = TestHome::new().unwrap();
    let vault = text_vault();
    let db = encrypted_database(&home, &vault);
    db.set_setting("common_config_claude", "source-unchanged-canary")
        .unwrap();
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    let metadata = std::fs::read(db.secrets.root().join("vault.json")).unwrap();
    let record = std::fs::read(db.secrets.root().join(crate::secrets::protection::FILE)).unwrap();
    let snapshot = downloaded(build_local_snapshot(&db).unwrap());
    let id = snapshot.manifest.snapshot_id.clone();
    assert_code(
        restore_from_sync(&db, &snapshot, &id, TEXT, &NoStore),
        "protection.record_authentication_failed",
    );
    assert_eq!(
        Database::content_digest(&db.conn.lock().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(db.secrets.root().join("vault.json")).unwrap(),
        metadata
    );
    assert_eq!(
        std::fs::read(db.secrets.root().join(crate::secrets::protection::FILE)).unwrap(),
        record
    );
    assert!(!db
        .secrets
        .root()
        .join(crate::secrets::transition::INTENT)
        .exists());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_mode_does_not_become_a_remote_plaintext_snapshot_or_a_fake_vault() {
    let home = TestHome::new().unwrap();
    let root = crate::config::get_app_config_dir();
    assert!(root.starts_with(home.path()));
    let context = StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: STORE.into(),
        generation_id: "22222222-2222-4222-8222-222222222222".into(),
        revision: 1,
    });
    let conn = crate::database::storage::prepare(&root.join(crate::config::DB_FILE_NAME), &context)
        .unwrap();
    let session = SecretSession::from_storage_context(root.clone(), context);
    crate::settings::unlock_settings_for_test(session.clone()).unwrap();
    let db = Database::from_connection(conn, session);
    db.set_setting("common_config_claude", "plaintext-stays-local-canary")
        .unwrap();
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    assert_code(build_local_snapshot(&db), "protection.encryption_required");
    assert_eq!(
        Database::content_digest(&db.conn.lock().unwrap()).unwrap(),
        before
    );
    assert!(!root.join("vault.json").exists());
    assert!(db.secrets.read().unwrap().is_plaintext());
}

fn local_tree(root: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn protected_active_backup_restore_uses_the_same_generation_database_installer() {
    let home = TestHome::new().unwrap();
    let password = "legacy synthetic backup password";
    let vault = VaultContext::create_with_user_key(Zeroizing::new(vec![33; 32]))
        .unwrap()
        .with_password(password)
        .unwrap();
    let db = encrypted_database(&home, &vault);
    db.set_setting("common_config_claude", "imported-protected-canary")
        .unwrap();
    let exported = home.path().join("synthetic-source.sql");
    db.export_sql(&exported).unwrap();
    let sql = std::fs::read_to_string(&exported).unwrap();
    db.set_setting("common_config_claude", "pre-restore-local-canary")
        .unwrap();
    let before_metadata = std::fs::read(db.secrets.root().join("vault.json")).unwrap();
    let before_record =
        std::fs::read(db.secrets.root().join(crate::secrets::protection::FILE)).unwrap();
    let before_identity = db.secrets.read().unwrap().identity();
    let _sync = sync_mutex().blocking_lock();
    let backup_id =
        crate::services::backup_transfer::restore_sql(&db, &sql, Some(password), &NoStore).unwrap();
    assert!(!backup_id.is_empty());
    assert_eq!(
        db.get_setting("common_config_claude").unwrap().as_deref(),
        Some("imported-protected-canary")
    );
    assert_eq!(db.secrets.read().unwrap().identity(), before_identity);
    assert_eq!(
        std::fs::read(db.secrets.root().join("vault.json")).unwrap(),
        before_metadata
    );
    assert_eq!(
        std::fs::read(db.secrets.root().join(crate::secrets::protection::FILE)).unwrap(),
        before_record
    );
    assert!(!db
        .secrets
        .root()
        .join(crate::secrets::transition::INTENT)
        .exists());
    let backup = rusqlite::Connection::open_with_flags(
        db.secrets
            .root()
            .join("backups")
            .join(format!("{backup_id}.db")),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    crate::database::vault::check_identity(&backup, &vault).unwrap();
    let old = Database::from_connection(
        backup,
        SecretSession::from_context(db.secrets.root().to_path_buf(), vault),
    );
    assert_eq!(
        old.get_setting("common_config_claude").unwrap().as_deref(),
        Some("pre-restore-local-canary")
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn text_generation_backup_restore_is_rejected_before_source_parsing_without_a_record() {
    let home = TestHome::new().unwrap();
    let vault = text_vault();
    let db = encrypted_database(&home, &vault);
    std::fs::remove_file(db.secrets.root().join(crate::secrets::protection::FILE)).unwrap();
    let before = local_tree(db.secrets.root());
    let _sync = sync_mutex().blocking_lock();
    assert_code(
        crate::services::backup_transfer::restore_sql(
            &db,
            "invalid SQL must not be admitted",
            None,
            &NoStore,
        ),
        "protection.record_missing",
    );
    assert_eq!(local_tree(db.secrets.root()), before);
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "manifest_text" => {
            valid_user_text_metadata_is_a_recoverable_manifest_source_without_legacy_wrapper()
        }
        "exact_text" => user_text_authentication_preserves_original_whitespace_and_unicode(),
        "no_legacy_fallback" => {
            text_descriptor_never_falls_back_to_a_valid_legacy_password_wrapper()
        }
        "legacy_password" => legacy_wrapped_password_authentication_remains_exact_and_recoverable(),
        "invalid_metadata" => malformed_descriptor_and_nonrecoverable_metadata_stay_rejected(),
        "build_package" => {
            original_database_export_and_snapshot_builder_accept_authenticated_text_generation()
        }
        "installer_fence" => {
            authenticated_text_restore_rejects_an_unsigned_target_protection_record()
        }
        "plaintext_blocked" => {
            plaintext_mode_does_not_become_a_remote_plaintext_snapshot_or_a_fake_vault()
        }
        "backup_restore_same_generation" => {
            protected_active_backup_restore_uses_the_same_generation_database_installer()
        }
        "backup_text_preflight" => {
            text_generation_backup_restore_is_rejected_before_source_parsing_without_a_record()
        }
        _ => panic!("unknown text archive case"),
    }
}
