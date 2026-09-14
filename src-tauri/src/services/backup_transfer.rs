use crate::{
    database::Database,
    error::AppError,
    secrets::{key_store::KeyStore, session::read_metadata, transition::install_generation},
    services::skill::skill_state_write_guard,
};

/// The command owns the common sync mutex across source validation and install.
/// Manual backups restore content into the current device's key generation.
pub(crate) fn restore_sql(
    db: &Database,
    sql: &str,
    password: Option<&str>,
    store: &dyn KeyStore,
) -> Result<String, AppError> {
    let current = db.secrets.read()?.clone();
    let incoming = Database::prepare_backup_content(sql, password, &current)?;
    let automatic_unlock = read_metadata(db.secrets.root())?.automatic_unlock;
    let _skills = skill_state_write_guard();
    // Authentication and schema validation finish before even the safety copy.
    let safety = db.backup_database_file()?;
    install_generation(
        db,
        store,
        current,
        automatic_unlock,
        |connection, current, next| {
            Database::prepare_sync_join(incoming, connection, current, next)
        },
        None,
    )?;
    Ok(safety
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        files::CredentialFile, session::SecretSession, testing::MemoryKeyStore, VaultContext,
    };

    struct Home {
        temporary: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl Home {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
            Self {
                temporary,
                previous,
            }
        }
        fn root(&self) -> std::path::PathBuf {
            self.temporary.path().join(crate::APP_DIR_NAME)
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }
    fn local(home: &Home, store: &MemoryKeyStore) -> Database {
        let session = SecretSession::open(&home.root(), store, None).unwrap();
        let conn = crate::database::vault::prepare(
            &home.root().join(crate::config::DB_FILE_NAME),
            &session.read().unwrap(),
        )
        .unwrap();
        session.complete_migration().unwrap();
        crate::settings::unlock_settings_for_test(session.clone()).unwrap();
        Database::from_connection(conn, session)
    }
    fn portable_sql(home: &Home) -> String {
        let source_root = home.temporary.path().join("source");
        let context = VaultContext::generate()
            .unwrap()
            .with_password("backup source password")
            .unwrap();
        let connection = crate::database::vault::prepare(
            &source_root.join(crate::config::DB_FILE_NAME),
            &context,
        )
        .unwrap();
        let db = Database::from_connection(
            connection,
            SecretSession::from_context(source_root, context),
        );
        let provider = crate::provider::Provider::with_id(
            "restored-provider".into(),
            "Restored provider".into(),
            serde_json::json!({"apiKey":"portable-backup-canary"}),
            None,
        );
        db.save_provider("claude", &provider).unwrap();
        let path = home.temporary.path().join("portable.sql");
        db.export_sql(&path).unwrap();
        let sql = std::fs::read_to_string(path).unwrap();
        assert!(!sql.contains("portable-backup-canary"));
        sql
    }

    #[test]
    #[serial_test::serial]
    fn password_content_restore_keeps_the_local_key_and_signin_credentials() {
        let home = Home::new();
        let sql = portable_sql(&home);
        let store = MemoryKeyStore::default();
        let db = local(&home, &store);
        let before = db.secrets.read().unwrap().metadata().clone();
        CredentialFile::Codex
            .write(&db.secrets, br#"{"token":"local-signin-canary"}"#)
            .unwrap();
        let safety = restore_sql(&db, &sql, Some("backup source password"), &store).unwrap();
        assert!(!safety.is_empty());
        assert_eq!(db.secrets.read().unwrap().metadata(), &before);
        let provider = db
            .get_provider_by_id("restored-provider", "claude")
            .unwrap()
            .unwrap();
        assert_eq!(provider.settings_config["apiKey"], "portable-backup-canary");
        let oauth = CredentialFile::Codex.read(&db.secrets).unwrap().unwrap();
        assert!(std::str::from_utf8(&oauth)
            .unwrap()
            .contains("local-signin-canary"));
        let reopened = SecretSession::open_existing(&home.root(), &store, None).unwrap();
        assert_eq!(reopened.read().unwrap().metadata(), &before);
    }

    #[test]
    #[serial_test::serial]
    fn explicit_legacy_plaintext_file_restore_encrypts_the_imported_credentials() {
        let home = Home::new();
        let source = Database::memory().unwrap();
        {
            let conn = source.conn.lock().unwrap();
            conn.execute_batch("DROP TABLE loongport_vault;
                INSERT INTO providers(id,app_type,name,settings_config,meta)
                VALUES ('legacy-provider','claude','Legacy provider','{\"apiKey\":\"legacy-backup-canary\"}','{}')").unwrap();
        }
        let sql = source.export_sql_string().unwrap();
        let store = MemoryKeyStore::default();
        let db = local(&home, &store);

        restore_sql(&db, &sql, None, &store).unwrap();

        let provider = db
            .get_provider_by_id("legacy-provider", "claude")
            .unwrap()
            .unwrap();
        assert_eq!(provider.settings_config["apiKey"], "legacy-backup-canary");
        assert!(!db
            .export_sql_string()
            .unwrap()
            .contains("legacy-backup-canary"));
    }

    #[test]
    #[serial_test::serial]
    fn portable_backup_remains_restorable_after_its_system_key_is_retired() {
        let home = Home::new();
        let store = MemoryKeyStore::default();
        let db = local(&home, &store);
        crate::secrets::transition::rotate(&db, &store, "first backup password", true).unwrap();
        db.set_setting("common_config_claude", "retired-key-content")
            .unwrap();
        let previous = db.secrets.read().unwrap().metadata().clone();
        let path = home.temporary.path().join("retained-backup.sql");
        db.export_sql(&path).unwrap();
        let sql = std::fs::read_to_string(path).unwrap();
        crate::secrets::transition::rotate(&db, &store, "current device password", true).unwrap();
        let current = db.secrets.read().unwrap().metadata().clone();
        assert!(store
            .load(&previous.vault_id, &previous.key_id)
            .unwrap()
            .is_none());
        db.set_setting("common_config_claude", "newer-content")
            .unwrap();

        restore_sql(&db, &sql, Some("first backup password"), &store).unwrap();

        assert_eq!(db.secrets.read().unwrap().metadata(), &current);
        assert_eq!(
            db.get_setting("common_config_claude").unwrap().as_deref(),
            Some("retired-key-content")
        );
        assert!(store
            .load(&previous.vault_id, &previous.key_id)
            .unwrap()
            .is_none());
    }

    #[test]
    #[serial_test::serial]
    fn missing_or_failed_backup_password_leaves_database_and_files_unchanged() {
        let home = Home::new();
        let sql = portable_sql(&home);
        let store = MemoryKeyStore::default();
        let db = local(&home, &store);
        db.set_setting("common_config_claude", "local-content-canary")
            .unwrap();
        let database = std::fs::read(home.root().join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(home.root().join("vault.json")).unwrap();
        let backup_files = || {
            std::fs::read_dir(home.root().join("backups"))
                .into_iter()
                .flatten()
                .map(|entry| entry.unwrap().file_name())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let before_backups = backup_files();
        for password in [None, Some("wrong backup password")] {
            assert!(restore_sql(&db, &sql, password, &store).is_err());
            assert_eq!(
                std::fs::read(home.root().join(crate::config::DB_FILE_NAME)).unwrap(),
                database
            );
            assert_eq!(
                std::fs::read(home.root().join("vault.json")).unwrap(),
                metadata
            );
        }
        assert_eq!(backup_files(), before_backups);
        assert_eq!(
            db.get_setting("common_config_claude").unwrap().as_deref(),
            Some("local-content-canary")
        );
    }
}
