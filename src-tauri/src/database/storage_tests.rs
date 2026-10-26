//! Real SQLite/storage owner tests. No OS credential store or runtime recovery.

use super::{storage, vault, Database};
use crate::provider::Provider;
use crate::secrets::testing::TestHome;
use crate::secrets::{
    storage::{PlaintextIdentity, StorageContext},
    VaultContext,
};
use rusqlite::Connection;
use serde_json::json;

fn identity() -> PlaintextIdentity {
    PlaintextIdentity {
        format_version: 1,
        store_id: "b06dd983-1dd7-40ac-ad44-4e84753bd58d".into(),
        generation_id: "b5b61e34-8328-4f21-bec8-fda33740b097".into(),
        revision: 1,
    }
}

fn assert_code<T>(result: Result<T, crate::error::AppError>, expected: &str) {
    match result {
        Err(crate::error::AppError::Config(code)) => assert_eq!(code, expected),
        Err(error) => panic!("unexpected error: {error}"),
        Ok(_) => panic!("operation unexpectedly succeeded"),
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_plaintext_database_reopens_without_vault_metadata() {
    let root = TestHome::new().unwrap();
    let path = root.path().join(crate::config::DB_FILE_NAME);
    let context = StorageContext::Plaintext(identity());
    let conn = storage::prepare(&path, &context).unwrap();
    conn.execute(
        "INSERT INTO settings(key,value) VALUES ('global_proxy_url','https://proxy.example')",
        [],
    )
    .unwrap();
    assert!(vault::stored_metadata(&conn).unwrap().is_none());
    storage::check_identity(&conn, &context).unwrap();
    drop(conn);
    let reopened = storage::prepare(&path, &context).unwrap();
    assert_eq!(
        reopened
            .query_row(
                "SELECT value FROM settings WHERE key='global_proxy_url'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "https://proxy.example"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn missing_plaintext_database_identity_is_not_legacy_adoption() {
    let root = TestHome::new().unwrap();
    let path = root.path().join(crate::config::DB_FILE_NAME);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT); INSERT INTO settings VALUES ('global_proxy_url','preserve-me');").unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert_code(
        storage::prepare(&path, &StorageContext::Plaintext(identity())),
        "protection.database_identity_required",
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_generation_mismatch_preserves_original_database() {
    let root = TestHome::new().unwrap();
    let path = root.path().join(crate::config::DB_FILE_NAME);
    drop(storage::prepare(&path, &StorageContext::Plaintext(identity())).unwrap());
    let before = std::fs::read(&path).unwrap();
    let mut other = identity();
    other.generation_id = "d4ada1c0-d540-4183-b51b-ea5027c1deda".into();
    assert_code(
        storage::prepare(&path, &StorageContext::Plaintext(other)),
        "protection.database_identity_mismatch",
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn encrypted_database_cannot_be_opened_as_plaintext() {
    let root = TestHome::new().unwrap();
    let path = root.path().join(crate::config::DB_FILE_NAME);
    let conn = Connection::open(&path).unwrap();
    vault::stamp(&conn, &VaultContext::generate().unwrap()).unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert_code(
        storage::prepare(&path, &StorageContext::Plaintext(identity())),
        "protection.encrypted_data_present",
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn mixed_envelopes_reject_plaintext_reopen_without_writing() {
    let root = TestHome::new().unwrap();
    let path = root.path().join(crate::config::DB_FILE_NAME);
    let context = StorageContext::Plaintext(identity());
    let conn = storage::prepare(&path, &context).unwrap();
    conn.execute(
        "INSERT INTO settings(key,value) VALUES ('global_proxy_url','lpenc:old-ciphertext')",
        [],
    )
    .unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    assert!(storage::prepare(&path, &context).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_plaintext_staging_preserves_legacy_values_and_identity() {
    let _home = TestHome::new().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    conn.execute("INSERT INTO providers(id,app_type,name,settings_config,meta) VALUES ('plain-provider','claude','Example','{\"apiKey\":\"plain-secret\"}','{}')", []).unwrap();
    storage::prepare_plaintext_staging(&conn, &identity()).unwrap();
    storage::check_identity(&conn, &StorageContext::Plaintext(identity())).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT settings_config FROM providers WHERE id='plain-provider'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "{\"apiKey\":\"plain-secret\"}"
    );
    assert!(vault::stored_metadata(&conn).unwrap().is_none());
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_staging_rejection_rolls_back_schema_and_values() {
    let _home = TestHome::new().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    conn.execute(
        "INSERT INTO settings(key,value) VALUES ('global_proxy_url','lpenc:old-ciphertext')",
        [],
    )
    .unwrap();
    let before = Database::content_digest(&conn).unwrap();
    assert!(storage::prepare_plaintext_staging(&conn, &identity()).is_err());
    assert_eq!(Database::content_digest(&conn).unwrap(), before);
}

fn plaintext_database(root: &std::path::Path) -> Database {
    let context = StorageContext::Plaintext(identity());
    let conn = storage::prepare(&root.join(crate::config::DB_FILE_NAME), &context).unwrap();
    let session = crate::SecretSession::from_storage_context(root.to_path_buf(), context);
    Database::from_connection(conn, session)
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_provider_and_setting_daos_preserve_original_sql_contracts() {
    let root = TestHome::new().unwrap();
    let db = plaintext_database(root.path());
    let provider = Provider::with_id(
        "plain-provider".into(),
        "Example".into(),
        json!({"apiKey":"plain-secret"}),
        None,
    );
    db.save_provider("claude", &provider).unwrap();
    db.set_setting("global_proxy_url", "https://proxy.example")
        .unwrap();
    assert_eq!(
        db.get_provider_by_id("plain-provider", "claude")
            .unwrap()
            .unwrap()
            .settings_config,
        json!({"apiKey":"plain-secret"})
    );
    assert_eq!(
        db.get_setting("global_proxy_url").unwrap().as_deref(),
        Some("https://proxy.example")
    );
    let conn = db.conn.lock().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT settings_config FROM providers WHERE id='plain-provider'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "{\"apiKey\":\"plain-secret\"}"
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM settings WHERE key='global_proxy_url'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "https://proxy.example"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_session_block_still_denies_secret_and_nonsecret_operations() {
    let root = TestHome::new().unwrap();
    let db = plaintext_database(root.path());
    let epoch = db.secret_session().lifecycle_epoch();
    db.secret_session().set_blocked(true);
    assert!(db.secret_session().lifecycle_epoch() > epoch);
    assert!(db.get_setting("global_proxy_url").is_err());
    assert!(db.get_current_provider("claude").is_err());
    db.secret_session().set_blocked(false);
    assert!(db.get_current_provider("claude").is_ok());
    assert!(db.secret_session().lifecycle_epoch() > epoch);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_backup_keeps_mode_identity_and_raw_values() {
    let root = TestHome::new().unwrap();
    let db = plaintext_database(root.path());
    db.set_setting("global_proxy_url", "https://proxy.example")
        .unwrap();
    let path = db.backup_database_file().unwrap().unwrap();
    let backup =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    storage::check_identity(&backup, &StorageContext::Plaintext(identity())).unwrap();
    assert!(vault::stored_metadata(&backup).unwrap().is_none());
    assert_eq!(
        backup
            .query_row(
                "SELECT value FROM settings WHERE key='global_proxy_url'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "https://proxy.example"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_sql_export_reuses_original_dump_without_recovery_password() {
    let root = TestHome::new().unwrap();
    let db = plaintext_database(root.path());
    db.set_setting("global_proxy_url", "https://proxy.example")
        .unwrap();
    let path = root.path().join("explicit-export.sql");
    db.export_sql(&path).unwrap();
    let sql = std::fs::read_to_string(&path).unwrap();
    assert!(sql.contains("https://proxy.example"));
    assert!(!sql.contains("lpenc"));
    assert!(sql.contains("loongport_plaintext_storage"));
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_restore_reuses_original_backup_owner_and_preserves_new_safety_copy() {
    let root = TestHome::new().unwrap();
    let db = plaintext_database(root.path());
    db.set_setting("global_proxy_url", "https://before.example")
        .unwrap();
    let source = db.backup_database_file().unwrap().unwrap();
    db.set_setting("global_proxy_url", "https://after.example")
        .unwrap();
    let safety = db
        .restore_from_backup(source.file_name().unwrap().to_str().unwrap())
        .unwrap();
    assert_eq!(
        db.get_setting("global_proxy_url").unwrap().as_deref(),
        Some("https://before.example")
    );
    assert!(source.exists());
    let path = root.path().join("backups").join(format!("{safety}.db"));
    let backup =
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert_eq!(
        backup
            .query_row(
                "SELECT value FROM settings WHERE key='global_proxy_url'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "https://after.example"
    );
    storage::check_identity(&backup, &StorageContext::Plaintext(identity())).unwrap();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn encryption_stamp_does_not_overwrite_explicit_plaintext_identity() {
    let root = TestHome::new().unwrap();
    let context = StorageContext::Plaintext(identity());
    let conn = storage::prepare(&root.path().join(crate::config::DB_FILE_NAME), &context).unwrap();
    let before = Database::content_digest(&conn).unwrap();
    assert_code(
        vault::stamp(&conn, &VaultContext::generate().unwrap()),
        "protection.plaintext_data_present",
    );
    assert_eq!(Database::content_digest(&conn).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn encryption_staging_does_not_implicitly_convert_explicit_plaintext() {
    let root = TestHome::new().unwrap();
    let context = StorageContext::Plaintext(identity());
    let conn = storage::prepare(&root.path().join(crate::config::DB_FILE_NAME), &context).unwrap();
    conn.execute(
        "INSERT INTO settings(key,value) VALUES('global_proxy_url','https://preserve.example')",
        [],
    )
    .unwrap();
    let before = Database::content_digest(&conn).unwrap();
    assert_code(
        vault::upgrade_staging(&conn, &VaultContext::generate().unwrap()),
        "protection.plaintext_data_present",
    );
    assert_eq!(Database::content_digest(&conn).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_setup_does_not_adopt_authenticated_schema_with_missing_metadata() {
    let _home = TestHome::new().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    Database::apply_upgrade_source_migrations_on_conn(&conn).unwrap();
    super::loongport_schema::apply(&conn).unwrap();
    assert!(vault::stored_metadata(&conn).unwrap().is_none());
    let before = Database::content_digest(&conn).unwrap();
    assert_code(
        storage::prepare_plaintext_staging(&conn, &identity()),
        "protection.database_identity_required",
    );
    assert_eq!(Database::content_digest(&conn).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_provider_guard_still_rejects_another_session() {
    let home = TestHome::new().unwrap();
    let first = home.path().join("first");
    let second = home.path().join("second");
    let db = plaintext_database(&first);
    let other = plaintext_database(&second);
    let guard = other.secret_session().read().unwrap();
    assert_code(
        db.get_provider_by_id_with_vault("provider", "claude", other.secret_session(), &guard),
        "settings.session_mismatch",
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_qoder_lease_cannot_publish_after_session_epoch_changes() {
    use crate::services::provider::qoder_daily::http::{CredentialSource, PatDeclaration};
    use crate::services::provider::qoder_http_auth::{
        Established, IdentityAction, IdentityActionRequest, PreparedIdentityAction, SourceBinding,
        TokenPair,
    };
    let home = TestHome::new().unwrap();
    let db = plaintext_database(home.path());
    let mut provider = Provider::with_id(
        "qoder-plain".into(),
        "Synthetic".into(),
        json!({"env":{"OPENAI_API_KEY":"synthetic-pat"}}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_cn".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let stored = db
        .get_provider_by_id(&provider.id, "codex")
        .unwrap()
        .unwrap();
    let source = SourceBinding::from_provider(&stored).unwrap().0;
    let declaration = PatDeclaration {
        provider_id: provider.id,
        credential_generation: source.credential_generation,
        source: CredentialSource::ChinaQoderPat,
    };
    let PreparedIdentityAction::Establish { receipt, .. } = db
        .qoder_http_prepare_identity_action(
            &IdentityActionRequest {
                declaration,
                action: IdentityAction::Establish,
            },
            1_000_000,
        )
        .unwrap()
    else {
        panic!("new plaintext fixture must require establishment")
    };
    let lease = db.qoder_http_reserve_identity_action(receipt).unwrap();
    let before: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT qoder_http_secret FROM providers WHERE id='qoder-plain'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!before.starts_with("lpenc"));
    db.secret_session().set_blocked(true);
    db.secret_session().set_blocked(false);
    assert_code(
        db.qoder_http_publish_establish(
            &lease,
            Established {
                uid: "synthetic-owner".into(),
                tokens: TokenPair {
                    access_token: "synthetic-access".into(),
                    refresh_token: "synthetic-refresh".into(),
                    expires_at_ms: 2_000_000,
                },
            },
            1_000_000,
        ),
        "qoder.http_auth.stale_owner",
    );
    let after: String = db
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT qoder_http_secret FROM providers WHERE id='qoder-plain'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(after, before);
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_storage_case(case: &str) {
    match case {
        "explicit_plaintext_database_reopens_without_vault_metadata" => {
            explicit_plaintext_database_reopens_without_vault_metadata()
        }
        "missing_plaintext_database_identity_is_not_legacy_adoption" => {
            missing_plaintext_database_identity_is_not_legacy_adoption()
        }
        "plaintext_generation_mismatch_preserves_original_database" => {
            plaintext_generation_mismatch_preserves_original_database()
        }
        "encrypted_database_cannot_be_opened_as_plaintext" => {
            encrypted_database_cannot_be_opened_as_plaintext()
        }
        "mixed_envelopes_reject_plaintext_reopen_without_writing" => {
            mixed_envelopes_reject_plaintext_reopen_without_writing()
        }
        "explicit_plaintext_staging_preserves_legacy_values_and_identity" => {
            explicit_plaintext_staging_preserves_legacy_values_and_identity()
        }
        "plaintext_staging_rejection_rolls_back_schema_and_values" => {
            plaintext_staging_rejection_rolls_back_schema_and_values()
        }
        "plaintext_provider_and_setting_daos_preserve_original_sql_contracts" => {
            plaintext_provider_and_setting_daos_preserve_original_sql_contracts()
        }
        "plaintext_session_block_still_denies_secret_and_nonsecret_operations" => {
            plaintext_session_block_still_denies_secret_and_nonsecret_operations()
        }
        "plaintext_backup_keeps_mode_identity_and_raw_values" => {
            plaintext_backup_keeps_mode_identity_and_raw_values()
        }
        "plaintext_sql_export_reuses_original_dump_without_recovery_password" => {
            plaintext_sql_export_reuses_original_dump_without_recovery_password()
        }
        "plaintext_restore_reuses_original_backup_owner_and_preserves_new_safety_copy" => {
            plaintext_restore_reuses_original_backup_owner_and_preserves_new_safety_copy()
        }
        "encryption_stamp_does_not_overwrite_explicit_plaintext_identity" => {
            encryption_stamp_does_not_overwrite_explicit_plaintext_identity()
        }
        "encryption_staging_does_not_implicitly_convert_explicit_plaintext" => {
            encryption_staging_does_not_implicitly_convert_explicit_plaintext()
        }
        "plaintext_setup_does_not_adopt_authenticated_schema_with_missing_metadata" => {
            plaintext_setup_does_not_adopt_authenticated_schema_with_missing_metadata()
        }
        "plaintext_provider_guard_still_rejects_another_session" => {
            plaintext_provider_guard_still_rejects_another_session()
        }
        "plaintext_qoder_lease_cannot_publish_after_session_epoch_changes" => {
            plaintext_qoder_lease_cannot_publish_after_session_epoch_changes()
        }
        _ => panic!("unknown explicit storage fixture case: {case}"),
    }
}
