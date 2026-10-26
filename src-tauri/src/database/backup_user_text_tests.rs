//! Actual portable snapshot owner with exact user-text-derived generations.
use super::*;
use crate::database::storage_generation_tests::{self as fixture, DERIVED, VALUE};
use crate::secrets::{inventory, session::SecretSession, testing::TestHome};

const TEXT: &str = "  synthetic 用户 key\nnot-normalized  ";
const SECRET: &str = "synthetic-portable-setting-canary";
fn text_context() -> VaultContext {
    VaultContext::create_generation_with_user_text(uuid::Uuid::new_v4().to_string(), 1, TEXT)
        .unwrap()
}
fn database(home: &TestHome, vault: &VaultContext) -> Database {
    let context = StorageContext::from(vault.clone());
    let db = Database::from_connection(
        fixture::source(&context),
        SecretSession::from_context(home.path().join("portable-source"), vault.clone()),
    );
    db.set_setting("global_proxy_url", SECRET).unwrap();
    db
}
fn assert_portable(sql: &str, metadata: &VaultMetadata, original: &VaultContext) {
    assert_eq!(metadata, original.metadata());
    assert!(
        metadata.wrapped_key.is_none(),
        "export must not invent a password-wrapped random master"
    );
    assert_eq!(metadata.user_text_key, original.metadata().user_text_key);
    let descriptor = metadata.user_text_key.as_ref().unwrap();
    assert_eq!(descriptor.kdf.algorithm, "argon2id");
    assert_eq!(
        descriptor.kdf.salt,
        original.metadata().user_text_key.as_ref().unwrap().kdf.salt
    );
    assert!(!descriptor.kdf.salt.is_empty());
    metadata.validate().unwrap();
    let public_bytes = serde_json::to_vec(metadata).unwrap();
    assert!(!public_bytes
        .windows(TEXT.len())
        .any(|window| window == TEXT.as_bytes()));
    let parsed = serde_json::from_slice(&public_bytes).unwrap();
    let reopened = VaultContext::from_user_text(parsed, TEXT).unwrap();
    assert!(VaultContext::from_user_text(metadata.clone(), TEXT.trim()).is_err());
    assert_eq!(
        reopened.export_key().as_slice(),
        original.export_key().as_slice()
    );
    for canary in [SECRET, VALUE, DERIVED, TEXT] {
        assert!(!sql.contains(canary), "plaintext leaked into portable SQL");
    }
    let staged = Database::stage_import_sql(sql, true).unwrap();
    assert_eq!(
        crate::database::vault::stored_metadata(&staged).unwrap(),
        Some(metadata.clone())
    );
    inventory::validate_database(&staged, &reopened).unwrap();
    fixture::assert_values(&staged, &StorageContext::from(reopened.clone()));
    let encoded: String = staged
        .query_row(
            "SELECT value FROM settings WHERE key='global_proxy_url'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        inventory::open_db(
            &StorageContext::from(reopened),
            "settings",
            "value",
            &["global_proxy_url"],
            &encoded
        )
        .unwrap(),
        SECRET
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn user_text_sync_snapshot_preserves_metadata_and_values() {
    let home = TestHome::new().unwrap();
    let vault = text_context();
    let db = database(&home, &vault);
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    let (sql, metadata) = db.export_sync_snapshot().unwrap();
    assert_portable(&sql, &metadata, &vault);
    fixture::assert_unchanged(&db.conn.lock().unwrap(), &before);
    assert_eq!(
        db.secret_session()
            .read()
            .unwrap()
            .encrypted()
            .unwrap()
            .metadata(),
        vault.metadata()
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn user_text_file_export_reopens_with_exact_text() {
    let home = TestHome::new().unwrap();
    let vault = text_context();
    let db = database(&home, &vault);
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    let destination = home.path().join("explicit-export").join("snapshot.sql");
    db.export_sql(&destination).unwrap();
    let sql = fs::read_to_string(&destination).unwrap();
    let staged = Database::stage_import_sql(&sql, true).unwrap();
    let metadata = crate::database::vault::stored_metadata(&staged)
        .unwrap()
        .unwrap();
    assert_portable(&sql, &metadata, &vault);
    fixture::assert_unchanged(&db.conn.lock().unwrap(), &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn user_text_export_rejects_corrupt_values_before_creating_file() {
    let home = TestHome::new().unwrap();
    let vault = text_context();
    let db = database(&home, &vault);
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE providers SET qoder_http_secret='synthetic-unsealed-derived'",
            [],
        )
        .unwrap();
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    let parent = home.path().join("must-not-create");
    assert!(db.export_sync_snapshot().is_err());
    assert!(db.export_sql(&parent.join("snapshot.sql")).is_err());
    assert!(!parent.exists());
    fixture::assert_unchanged(&db.conn.lock().unwrap(), &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn user_text_export_rejects_malformed_descriptors() {
    let home = TestHome::new().unwrap();
    let vault = text_context();
    for corrupt in ["format", "algorithm", "salt", "cost"] {
        let db = database(&home, &vault);
        let mut metadata = vault.metadata().clone();
        let descriptor = metadata.user_text_key.as_mut().unwrap();
        match corrupt {
            "format" => descriptor.format_version = 999,
            "algorithm" => descriptor.kdf.algorithm = "unsupported".into(),
            "salt" => descriptor.kdf.salt = "not base64".into(),
            "cost" => descriptor.kdf.memory_kib = u32::MAX,
            _ => unreachable!(),
        }
        assert!(metadata.validate().is_err());
        assert!(VaultContext::from_user_text(metadata.clone(), TEXT).is_err());
        // A real authenticated session cannot be constructed from malformed
        // metadata. Corrupt the actual DB stamp instead; retain the real session
        // and prove the original stamp/inventory boundary still prevents export.
        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE loongport_vault SET metadata=?1 WHERE id=1",
                [serde_json::to_string(&metadata).unwrap()],
            )
            .unwrap();
        let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
        let destination = home
            .path()
            .join(format!("invalid-{corrupt}"))
            .join("snapshot.sql");
        assert!(db.export_sync_snapshot().is_err());
        assert!(db.export_sql(&destination).is_err());
        assert!(!destination.parent().unwrap().exists());
        fixture::assert_unchanged(&db.conn.lock().unwrap(), &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_portable_material_contract_is_preserved() {
    let home = TestHome::new().unwrap();
    let raw = VaultContext::generate().unwrap();
    let db = database(&home, &raw);
    let destination = home
        .path()
        .join("legacy-unrecoverable")
        .join("snapshot.sql");
    let sync_error = db.export_sync_snapshot().unwrap_err();
    assert!(matches!(sync_error,AppError::Config(code) if code=="sync.recovery_password_required"));
    let file_error = db.export_sql(&destination).unwrap_err();
    assert!(
        matches!(file_error,AppError::Config(code) if code=="backup.recovery_password_required")
    );
    assert!(!destination.parent().unwrap().exists());
    let portable = raw
        .with_password("synthetic legacy export password")
        .unwrap();
    let db = database(&home, &portable);
    let (sql, metadata) = db.export_sync_snapshot().unwrap();
    assert_eq!(&metadata, portable.metadata());
    assert!(metadata.user_text_key.is_none());
    assert!(metadata.wrapped_key.is_some());
    let reopened =
        VaultContext::from_password(metadata, "synthetic legacy export password").unwrap();
    let staged = Database::stage_import_sql(&sql, true).unwrap();
    inventory::validate_database(&staged, &reopened).unwrap();
    fixture::assert_values(&staged, &StorageContext::from(reopened));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_sync_does_not_gain_implicit_portability() {
    let home = TestHome::new().unwrap();
    let encrypted = StorageContext::from(VaultContext::generate().unwrap());
    let plain = fixture::plaintext_successor(&encrypted);
    let db = Database::from_connection(
        fixture::source(&plain),
        SecretSession::from_storage_context(home.path().join("explicit-plain"), plain),
    );
    let before = Database::content_digest(&db.conn.lock().unwrap()).unwrap();
    assert!(
        matches!(db.export_sync_snapshot(), Err(AppError::Config(code)) if code=="protection.encryption_required")
    );
    fixture::assert_unchanged(&db.conn.lock().unwrap(), &before);
}

fn source_sql(db: &Database) -> String {
    // Isolate source authentication from the separate public export admission.
    Database::dump_sql(&db.snapshot_to_memory().unwrap(), &[]).unwrap()
}
fn assert_prepared_source(staged: &ValidatedSyncSnapshot, current: &VaultContext) {
    assert_eq!(&staged.destination, current.metadata());
    inventory::validate_database(&staged.connection, current).unwrap();
    fixture::assert_values(&staged.connection, &StorageContext::from(current.clone()));
    let encoded: String = staged
        .connection
        .query_row(
            "SELECT value FROM settings WHERE key='global_proxy_url'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        inventory::open_db(
            &StorageContext::from(current.clone()),
            "settings",
            "value",
            &["global_proxy_url"],
            &encoded
        )
        .unwrap(),
        SECRET
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn sql_source_user_text_authenticates_without_bypassing_installer() {
    use crate::secrets::transition::fence_test_support as fence;
    let home = TestHome::new().unwrap();
    let source_context = text_context();
    let source = database(&home, &source_context);
    let sql = source_sql(&source);
    let current = VaultContext::generate().unwrap();
    let local = database(&home, &current);
    local
        .set_setting("global_proxy_url", "synthetic-local-before-prepare")
        .unwrap();
    fence::write_record(
        local.secret_session().root(),
        fence::Scope::Encrypted,
        &current,
    );
    let before_tree = fence::tree(home.path());
    let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    let before_source = Database::content_digest(&source.conn.lock().unwrap()).unwrap();
    let staged = local
        .prepare_backup_content(&sql, Some(TEXT), &current)
        .unwrap();
    assert_prepared_source(&staged, &current);
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
    fixture::assert_unchanged(&source.conn.lock().unwrap(), &before_source);
    let store = fence::CountingStore::default();
    let rejected = crate::secrets::transition::install_generation(
        &local,
        &store,
        current.clone(),
        false,
        |_, _, _| panic!("protected installer entered preparation"),
        None,
    );
    fence::assert_rejected(rejected, fence::Scope::Encrypted);
    assert_eq!(store.calls(), 0);
    assert_eq!(fence::tree(home.path()), before_tree);
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn sql_source_wrong_text_never_falls_back_to_legacy_wrapper() {
    let home = TestHome::new().unwrap();
    // Dual metadata deliberately tests deterministic descriptor precedence.
    let fallback = "synthetic legacy fallback password";
    let source_context = text_context().with_password(fallback).unwrap();
    let source = database(&home, &source_context);
    let sql = source_sql(&source);
    let current = VaultContext::generate().unwrap();
    let local = database(&home, &current);
    let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    for wrong in [
        Some(TEXT.trim()),
        Some(fallback),
        Some("wrong exact key"),
        Some(""),
        None,
    ] {
        assert!(
            local.prepare_backup_content(&sql, wrong, &current).is_err(),
            "wrong text fell back to legacy recovery password"
        );
        fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn sql_source_legacy_wrapper_uses_metadata_not_input_shape() {
    let home = TestHome::new().unwrap();
    let looks_like_raw_key = "a".repeat(64);
    let source_context = VaultContext::generate()
        .unwrap()
        .with_password(&looks_like_raw_key)
        .unwrap();
    let source = database(&home, &source_context);
    let sql = source_sql(&source);
    let current = VaultContext::generate().unwrap();
    let local = database(&home, &current);
    let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    let staged = local
        .prepare_backup_content(&sql, Some(&looks_like_raw_key), &current)
        .unwrap();
    assert_prepared_source(&staged, &current);
    assert!(local
        .prepare_backup_content(&sql, Some("wrong legacy password"), &current)
        .is_err());
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn sql_source_missing_descriptor_has_no_user_text_fallback() {
    let home = TestHome::new().unwrap();
    let source_context = VaultContext::generate().unwrap();
    assert!(source_context.metadata().user_text_key.is_none());
    assert!(source_context.metadata().wrapped_key.is_none());
    let source = database(&home, &source_context);
    let sql = source_sql(&source);
    let current = VaultContext::generate().unwrap();
    let local = database(&home, &current);
    let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    for text in [
        Some(TEXT),
        Some("a0123456789012345678901234567890123456789012345678901234567890123"),
        None,
    ] {
        assert!(local.prepare_backup_content(&sql, text, &current).is_err());
        fixture::assert_unchanged(&local.conn.lock().unwrap(), &before);
    }
}

pub(super) fn verify(case: &str) {
    match case {
        "sql_source_user_text_authenticates_without_bypassing_installer" => {
            sql_source_user_text_authenticates_without_bypassing_installer()
        }
        "sql_source_wrong_text_never_falls_back_to_legacy_wrapper" => {
            sql_source_wrong_text_never_falls_back_to_legacy_wrapper()
        }
        "sql_source_legacy_wrapper_uses_metadata_not_input_shape" => {
            sql_source_legacy_wrapper_uses_metadata_not_input_shape()
        }
        "sql_source_missing_descriptor_has_no_user_text_fallback" => {
            sql_source_missing_descriptor_has_no_user_text_fallback()
        }
        "user_text_sync_snapshot_preserves_metadata_and_values" => {
            user_text_sync_snapshot_preserves_metadata_and_values()
        }
        "user_text_file_export_reopens_with_exact_text" => {
            user_text_file_export_reopens_with_exact_text()
        }
        "user_text_export_rejects_corrupt_values_before_creating_file" => {
            user_text_export_rejects_corrupt_values_before_creating_file()
        }
        "user_text_export_rejects_malformed_descriptors" => {
            user_text_export_rejects_malformed_descriptors()
        }
        "legacy_portable_material_contract_is_preserved" => {
            legacy_portable_material_contract_is_preserved()
        }
        "plaintext_sync_does_not_gain_implicit_portability" => {
            plaintext_sync_does_not_gain_implicit_portability()
        }
        _ => panic!("unknown user-text portable export test case"),
    }
}
