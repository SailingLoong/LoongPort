//! 显式明文导出：用户主动触发的所有权出口，不改变已选存储模式。
//!
//! 输出原有 `inventory::TABLES` 受保护列（排除派生 Qoder 凭据）与 `AUTH_FILES`，
//! 并非全部账户/设备文件的完整备份。两种存储模式共用原 inventory 与文件 codec；
//! 仅经用户在 UI 确认后触发，落盘仍使用 0600 私密权限。

use super::files::AUTH_FILES;
use super::inventory;
use super::session::SecretSession;
use crate::error::AppError;
use serde_json::{json, Map, Value};

pub(crate) fn build_plaintext_export(session: &SecretSession) -> Result<String, AppError> {
    let context = session.read()?;
    crate::config_file_io::directory_exists_no_alias(session.root())?;
    super::protection::validate_runtime_storage(session.root(), &context)?;
    let db_path = session.root().join(crate::config::DB_FILE_NAME);
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(e.to_string()))?;
    crate::database::storage::check_identity(&conn, &context)?;

    // database = {表名: [{身份列…, 受保护列: 明文}, …]}；行按身份列去重聚合
    //（同一行的多列秘密合并进同一个对象，读的人一眼一行）。BTreeMap 让键序稳定。
    let mut database: std::collections::BTreeMap<
        String,
        Vec<std::collections::BTreeMap<String, String>>,
    > = std::collections::BTreeMap::new();
    inventory::collect_plaintext_values_with_storage(
        &conn,
        &context,
        |table, column, keys: Vec<String>, plaintext| {
            let columns = inventory::table_identity_columns(table);
            let rows = database.entry(table.to_string()).or_default();
            let identity: Vec<(&str, &String)> = columns
                .iter()
                .zip(keys.iter())
                .map(|(name, key)| (*name, key))
                .collect();
            if let Some(entry) = rows.iter_mut().find(|row| {
                identity
                    .iter()
                    .all(|(name, value)| row.get(*name) == Some(value))
            }) {
                entry.insert(column.to_string(), plaintext);
            } else {
                let mut entry: std::collections::BTreeMap<String, String> = identity
                    .into_iter()
                    .map(|(name, value)| (name.to_string(), value.clone()))
                    .collect();
                entry.insert(column.to_string(), plaintext);
                rows.push(entry);
            }
        },
    )?;

    // Only absent original AUTH_FILES are skipped. A present unreadable or invalid
    // file must fail the export rather than silently producing an incomplete result.
    let mut files = Map::new();
    for file in AUTH_FILES {
        let path = file.path(session);
        // Match the existing credential-file owner's bounded passive reader
        // (files::ensure_plaintext_destination); never follow a leaf alias.
        let Some(bytes) = crate::config_file_io::read_regular_file(&path, 64 * 1024 * 1024)
            .map_err(|error| AppError::io(&path, error))?
        else {
            continue;
        };
        let plaintext = file.decode_storage(&context, &bytes)?;
        files.insert(
            file.filename().to_string(),
            Value::String(String::from_utf8_lossy(&plaintext).into_owned()),
        );
    }

    let database_json: Map<String, Value> = database
        .into_iter()
        .map(|(table, rows)| {
            (
                table,
                Value::Array(
                    rows.into_iter()
                        .map(|row| {
                            Value::Object(
                                row.into_iter()
                                    .map(|(k, v)| (k, Value::String(v)))
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
            )
        })
        .collect();
    let export = json!({
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "database": Value::Object(database_json),
        "files": Value::Object(files),
    });
    serde_json::to_string_pretty(&export).map_err(|e| AppError::Config(e.to_string()))
}

/// 自带保存对话框的命令壳：用户取消返回 None；确认则 0600 私密落盘。
#[cfg(feature = "gui")]
#[tauri::command]
pub async fn export_plaintext_secrets<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name("loongport-plaintext-secrets.json")
        .blocking_save_file()
        .map(|p| p.to_string())
    else {
        return Ok(None);
    };
    let db = state.db.clone();
    let content = tauri::async_runtime::spawn_blocking(move || build_plaintext_export(&db.secrets))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    crate::config::atomic_write_private(std::path::Path::new(&path), content.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(Some(path))
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) mod storage_tests {
    use super::*;
    use crate::{
        database::Database,
        secrets::{
            files::{CredentialFile, OwnedFile},
            protection::{ProtectionRecord, ProtectionStorage},
            session::{completed_metadata, write_metadata, SecretSession},
            storage::{PlaintextIdentity, StorageContext},
            VaultContext,
        },
    };
    use sha2::{Digest, Sha256};
    use std::{collections::BTreeMap, path::PathBuf};
    use zeroize::Zeroizing;

    const STORE: &str = "11111111-1111-4111-8111-111111111111";
    const DB_SECRET: &str = "aggregate-database-secret-canary";
    const SETTING_SECRET: &str = "aggregate-setting-secret-canary";
    const QODER_SECRET: &str = "derived-qoder-must-not-be-exported-canary";
    const AUTH: &[u8] = br#"{"token":"aggregate-auth-file-canary"}"#;
    fn context(mode: &str) -> StorageContext {
        match mode {
            "plain" => StorageContext::Plaintext(PlaintextIdentity {
                format_version: 1,
                store_id: STORE.into(),
                generation_id: "22222222-2222-4222-8222-222222222222".into(),
                revision: 1,
            }),
            "text" => VaultContext::create_generation_with_user_text(
                STORE.into(),
                1,
                "  synthetic export 文本\n key  ",
            )
            .unwrap()
            .into(),
            "legacy" => VaultContext::create_generation_with_user_key(
                STORE.into(),
                1,
                Zeroizing::new(vec![31; 32]),
            )
            .unwrap()
            .into(),
            _ => panic!("unknown synthetic storage mode"),
        }
    }
    struct Fixture {
        root: tempfile::TempDir,
        db: Database,
    }
    impl Fixture {
        fn new(mode: &str) -> Self {
            Self::with_context(context(mode))
        }
        fn with_context(context: StorageContext) -> Self {
            let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
            let conn = crate::database::storage::prepare(
                &root.path().join(crate::config::DB_FILE_NAME),
                &context,
            )
            .unwrap();
            match &context {
                StorageContext::Plaintext(identity) => {
                    let record = ProtectionRecord {
                        format_version: 1,
                        choice_complete: true,
                        storage: ProtectionStorage::Plaintext {
                            identity: identity.clone(),
                        },
                        commitment: None,
                        cleanup: None,
                    };
                    std::fs::write(
                        root.path().join(crate::secrets::protection::FILE),
                        serde_json::to_vec(&record).unwrap(),
                    )
                    .unwrap();
                }
                StorageContext::Encrypted(vault) => {
                    write_metadata(root.path(), &completed_metadata(vault, false).unwrap())
                        .unwrap();
                    if vault.metadata().user_text_key.is_some() {
                        let record = ProtectionRecord::encrypted(vault, None).unwrap();
                        std::fs::write(
                            root.path().join(crate::secrets::protection::FILE),
                            serde_json::to_vec(&record).unwrap(),
                        )
                        .unwrap();
                    }
                }
            }
            let session = SecretSession::from_storage_context(root.path().to_path_buf(), context);
            let db = Database::from_connection(conn, session);
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    "aggregate-fixture".into(),
                    "Synthetic export fixture".into(),
                    json!({"apiKey":DB_SECRET}),
                    None,
                ),
            )
            .unwrap();
            db.set_setting("common_config_claude", SETTING_SECRET)
                .unwrap();
            db.set_setting("ui.language", "public-setting-not-exported")
                .unwrap();
            {
                let context = db.secrets.read().unwrap();
                let encoded = inventory::seal_db(
                    &context,
                    "providers",
                    "qoder_http_secret",
                    &["aggregate-fixture", "claude"],
                    QODER_SECRET,
                )
                .unwrap();
                db.conn.lock().unwrap().execute("UPDATE providers SET qoder_http_secret=?1 WHERE id='aggregate-fixture' AND app_type='claude'",[encoded]).unwrap();
                let other = OwnedFile::registered("workbuddy_accounts.json").unwrap();
                std::fs::write(
                    root.path().join("workbuddy_accounts.json"),
                    other
                        .encode_storage(&context, b"out-of-original-export-list")
                        .unwrap(),
                )
                .unwrap();
            }
            for file in AUTH_FILES {
                file.write(&db.secrets, AUTH).unwrap();
            }
            Self { root, db }
        }
        fn image(&self) -> BTreeMap<PathBuf, (u8, [u8; 32])> {
            walkdir::WalkDir::new(self.root.path())
                .follow_links(false)
                .into_iter()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let bytes = if entry.file_type().is_symlink() {
                        (
                            2,
                            Sha256::digest(
                                std::fs::read_link(entry.path())
                                    .unwrap()
                                    .as_os_str()
                                    .as_encoded_bytes(),
                            )
                            .into(),
                        )
                    } else if entry.file_type().is_file() {
                        (
                            1,
                            Sha256::digest(std::fs::read(entry.path()).unwrap()).into(),
                        )
                    } else {
                        assert!(entry.file_type().is_dir());
                        (0, [0; 32])
                    };
                    (
                        entry
                            .path()
                            .strip_prefix(self.root.path())
                            .unwrap()
                            .to_path_buf(),
                        bytes,
                    )
                })
                .collect()
        }
        fn reject(&self, expected: &str) {
            let before = self.image();
            match build_plaintext_export(&self.db.secrets) {
                Err(AppError::Config(code)) => assert_eq!(code, expected),
                Err(other) => panic!("unexpected export error: {other}"),
                Ok(_) => panic!("invalid source was silently exported"),
            }
            assert!(
                self.image() == before,
                "failed export changed source bytes or tree membership"
            );
        }
    }
    fn successful(mode: &str) {
        let fixture = Fixture::new(mode);
        let before = fixture.image();
        let raw = build_plaintext_export(&fixture.db.secrets).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert!(raw.contains(DB_SECRET));
        assert!(raw.contains(SETTING_SECRET));
        assert!(!raw.contains(QODER_SECRET));
        assert!(!raw.contains("public-setting-not-exported"));
        let providers = value["database"]["providers"].as_array().unwrap();
        let provider = providers
            .iter()
            .find(|row| row["id"] == "aggregate-fixture")
            .unwrap();
        assert_eq!(provider["app_type"], "claude");
        assert!(provider.get("qoder_http_secret").is_none());
        let files = value["files"].as_object().unwrap();
        assert_eq!(files.len(), AUTH_FILES.len());
        for file in AUTH_FILES {
            assert_eq!(files[file.filename()], std::str::from_utf8(AUTH).unwrap());
        }
        assert!(
            !files.contains_key("workbuddy_accounts.json"),
            "do not widen the existing AUTH_FILES export registry"
        );
        assert!(
            fixture.image() == before,
            "successful export changed source bytes or tree membership"
        );
    }
    #[cfg_attr(test, test)]
    fn explicit_plain_storage_aggregate_export_preserves_original_registry() {
        successful("plain");
    }
    #[cfg_attr(test, test)]
    fn user_text_storage_aggregate_export_preserves_original_registry() {
        successful("text");
    }
    #[cfg_attr(test, test)]
    fn legacy_storage_aggregate_export_preserves_original_registry() {
        successful("legacy");
    }
    #[cfg_attr(test, test)]
    fn blocked_session_cannot_export_any_storage_values() {
        let fixture = Fixture::new("plain");
        fixture.db.secrets.set_blocked(true);
        fixture.reject("secret.recovery_required");
    }
    #[cfg_attr(test, test)]
    fn plaintext_database_rejects_old_ciphertext_instead_of_exporting_it() {
        let fixture = Fixture::new("plain");
        fixture.db.conn.lock().unwrap().execute("UPDATE providers SET settings_config='lpenc1:old-ciphertext' WHERE id='aggregate-fixture'",[]).unwrap();
        fixture.reject("secret.unsupported_format");
    }
    #[cfg_attr(test, test)]
    fn plaintext_auth_file_rejects_old_ciphertext_instead_of_silent_omission() {
        let fixture = Fixture::new("plain");
        std::fs::write(
            CredentialFile::Codex.path(&fixture.db.secrets),
            b"lpenc1:old-auth-ciphertext",
        )
        .unwrap();
        fixture.reject("secret.unsupported_format");
    }
    #[cfg_attr(test, test)]
    fn excluded_derived_qoder_value_is_still_authenticated_before_export() {
        let fixture = Fixture::new("plain");
        fixture.db.conn.lock().unwrap().execute("UPDATE providers SET qoder_http_secret='lpenc1:old-derived-ciphertext' WHERE id='aggregate-fixture'",[]).unwrap();
        fixture.reject("secret.unsupported_format");
    }
    #[cfg_attr(test, test)]
    fn invalid_encrypted_auth_file_is_not_silently_omitted() {
        let fixture = Fixture::new("text");
        std::fs::write(
            CredentialFile::Codex.path(&fixture.db.secrets),
            b"not-a-vault-envelope",
        )
        .unwrap();
        fixture.reject("secret.unsupported_format");
    }
    #[cfg_attr(test, test)]
    fn foreign_plain_database_is_not_exported_under_old_session() {
        let fixture = Fixture::new("plain");
        let foreign = Fixture::with_context(StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: "33333333-3333-4333-8333-333333333333".into(),
            generation_id: "44444444-4444-4444-8444-444444444444".into(),
            revision: 1,
        }));
        foreign
            .db
            .set_setting("common_config_claude", "foreign-plain-database-canary")
            .unwrap();
        drop(foreign.db);
        std::fs::copy(
            foreign.root.path().join(crate::config::DB_FILE_NAME),
            fixture.root.path().join(crate::config::DB_FILE_NAME),
        )
        .unwrap();
        fixture.reject("protection.database_identity_mismatch");
    }
    #[cfg_attr(test, test)]
    fn plaintext_export_requires_existing_explicit_choice_record() {
        let fixture = Fixture::new("plain");
        std::fs::remove_file(fixture.root.path().join(crate::secrets::protection::FILE)).unwrap();
        fixture.reject("protection.record_missing");
    }
    #[cfg_attr(test, test)]
    fn user_text_export_requires_existing_explicit_choice_record() {
        let fixture = Fixture::new("text");
        std::fs::remove_file(fixture.root.path().join(crate::secrets::protection::FILE)).unwrap();
        fixture.reject("protection.record_missing");
    }
    #[cfg_attr(test, test)]
    fn signed_pending_cleanup_prevents_export() {
        let fixture = Fixture::new("text");
        let context = fixture.db.secrets.read().unwrap();
        let record = ProtectionRecord::encrypted(
            context.encrypted().unwrap(),
            Some("synthetic-pending-cleanup"),
        )
        .unwrap();
        drop(context);
        std::fs::write(
            fixture.root.path().join(crate::secrets::protection::FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        fixture.reject("protection.cleanup_required");
    }
    #[cfg(unix)]
    #[cfg_attr(test, test)]
    fn plaintext_export_refuses_file_and_root_aliases_without_reading_external_canaries() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new("plain");
        let external = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let canary = external.path().join("synthetic-canary");
        std::fs::write(&canary, b"synthetic-external-canary-never-export").unwrap();
        let file = CredentialFile::Codex.path(&fixture.db.secrets);
        std::fs::remove_file(&file).unwrap();
        symlink(&canary, &file).unwrap();
        let before = fixture.image();
        let leaf_result = build_plaintext_export(&fixture.db.secrets);
        assert!(fixture.image() == before);
        assert_eq!(
            std::fs::read(&canary).unwrap(),
            b"synthetic-external-canary-never-export"
        );
        std::fs::remove_file(&file).unwrap();
        CredentialFile::Codex
            .write(&fixture.db.secrets, AUTH)
            .unwrap();
        let alias_root = external.path().join("root-alias");
        symlink(fixture.root.path(), &alias_root).unwrap();
        let aliased = SecretSession::from_storage_context(
            alias_root.clone(),
            fixture.db.secrets.read().unwrap().clone(),
        );
        let before = fixture.image();
        let root_result = build_plaintext_export(&aliased);
        assert!(fixture.image() == before);
        assert_eq!(
            std::fs::read_link(&alias_root).unwrap(),
            fixture.root.path()
        );
        // Exercise both boundaries before assertions so a legacy acceptance of
        // the leaf cannot conceal the independent root-alias result.
        let leaf_rejected = matches!(leaf_result, Err(AppError::Io { source, .. }) if source.kind() == std::io::ErrorKind::InvalidData);
        let root_rejected = matches!(root_result, Err(AppError::Config(code)) if code == "secret.invalid_storage_path");
        assert!(
            leaf_rejected && root_rejected,
            "alias admission: leaf_rejected={leaf_rejected}, root_rejected={root_rejected}"
        );
    }
    #[cfg(feature = "test-hooks")]
    pub(crate) fn run_case(case: &str) {
        match case {
            "plain" => explicit_plain_storage_aggregate_export_preserves_original_registry(),
            "text" => user_text_storage_aggregate_export_preserves_original_registry(),
            "legacy" => legacy_storage_aggregate_export_preserves_original_registry(),
            "blocked" => blocked_session_cannot_export_any_storage_values(),
            "plain_db_cipher" => {
                plaintext_database_rejects_old_ciphertext_instead_of_exporting_it()
            }
            "plain_auth_cipher" => {
                plaintext_auth_file_rejects_old_ciphertext_instead_of_silent_omission()
            }
            "qoder_cipher" => excluded_derived_qoder_value_is_still_authenticated_before_export(),
            "invalid_encrypted_auth" => invalid_encrypted_auth_file_is_not_silently_omitted(),
            "foreign_plain_db" => foreign_plain_database_is_not_exported_under_old_session(),
            "missing_plain_record" => plaintext_export_requires_existing_explicit_choice_record(),
            "missing_text_record" => user_text_export_requires_existing_explicit_choice_record(),
            "cleanup_pending" => signed_pending_cleanup_prevents_export(),
            #[cfg(unix)]
            "auth_alias" => {
                plaintext_export_refuses_file_and_root_aliases_without_reading_external_canaries()
            }
            _ => panic!("unknown aggregate storage export case"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn plaintext_export_covers_protected_locations_and_skips_plain_settings() {
        let root = crate::secrets::testing::tempdir().unwrap();
        let store = crate::secrets::testing::MemoryKeyStore::default();
        let session = SecretSession::open(root.path(), &store, None).unwrap();

        // 手工建库：一行密文 providers + 一行非保护 settings（不该进导出）。
        let conn =
            rusqlite::Connection::open(root.path().join(crate::config::DB_FILE_NAME)).unwrap();
        conn.execute_batch(
            "CREATE TABLE providers (id TEXT, app_type TEXT, settings_config TEXT, meta TEXT, PRIMARY KEY(id,app_type));
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
        )
        .unwrap();
        let vault = session.read().unwrap();
        crate::database::storage::stamp(&conn, &vault).unwrap();
        super::super::session::write_metadata(
            root.path(),
            &super::super::session::completed_metadata(vault.encrypted().unwrap(), false).unwrap(),
        )
        .unwrap();
        let sealed = inventory::seal_db(
            &vault,
            "providers",
            "settings_config",
            &["first", "codex"],
            r#"{"api_key":"canary-credential"}"#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO providers VALUES ('first','codex',?1,'')",
            [sealed],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings VALUES ('ui.language','not-a-secret')",
            [],
        )
        .unwrap();
        drop(conn);

        let export = build_plaintext_export(&session).unwrap();
        assert!(export.contains("canary-credential"));
        assert!(
            !export.contains("not-a-secret"),
            "非保护 settings 键不该进明文导出"
        );
        // 身份列随行带出，读的人知道这条密文属于谁。
        assert!(export.contains("\"app_type\": \"codex\""));
    }
}
