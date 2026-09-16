//! Storage identities shared by adapters and controlled migration.

pub(crate) fn is_protected_setting(key: &str) -> bool {
    matches!(
        key,
        "universal_providers" | "global_proxy_url" | "claude_desktop_gateway_token"
    ) || key
        .strip_prefix("common_config_")
        .is_some_and(|app| crate::app_config::AppType::all().any(|kind| kind.as_str() == app))
}

use super::VaultContext;
use crate::error::AppError;
use rusqlite::{params_from_iter, types::Value, Connection};

pub(crate) struct ProtectedTable {
    pub name: &'static str,
    pub identity_columns: &'static [&'static str],
    pub columns: &'static [&'static str],
    reset: ResetPolicy,
}

enum ResetPolicy {
    EmptyObject,
    EmptyStrings,
    RemoveRows,
    RemoveProtectedSettings,
}

pub(crate) const TABLES: &[ProtectedTable] = &[
    ProtectedTable {
        name: "provider_endpoints",
        identity_columns: &["id", "provider_id", "app_type"],
        columns: &["url"],
        reset: ResetPolicy::RemoveRows,
    },
    ProtectedTable {
        name: "providers",
        reset: ResetPolicy::EmptyObject,
        identity_columns: &["id", "app_type"],
        columns: &["settings_config", "meta"],
    },
    ProtectedTable {
        name: "settings",
        reset: ResetPolicy::RemoveProtectedSettings,
        identity_columns: &["key"],
        columns: &["value"],
    },
    ProtectedTable {
        name: "mcp_servers",
        reset: ResetPolicy::EmptyObject,
        identity_columns: &["id"],
        columns: &["server_config"],
    },
    ProtectedTable {
        name: "loongport_relay",
        reset: ResetPolicy::EmptyStrings,
        identity_columns: &["id"],
        columns: &["auth_token", "refresh_token", "cf_clearance"],
    },
    ProtectedTable {
        name: "loongport_vendor",
        reset: ResetPolicy::EmptyStrings,
        identity_columns: &["id"],
        columns: &["auth_token", "api_key"],
    },
    ProtectedTable {
        name: "proxy_live_backup",
        reset: ResetPolicy::RemoveRows,
        identity_columns: &["app_type"],
        columns: &["original_config"],
    },
];

pub(crate) fn secret_error(error: super::SecretError) -> AppError {
    AppError::Config(error.code().to_owned())
}

pub(crate) fn seal_db(
    vault: &VaultContext,
    table: &str,
    column: &str,
    keys: &[&str],
    value: &str,
) -> Result<String, AppError> {
    // NULL and empty strings denote absent credentials in existing table contracts.
    if value.is_empty() {
        return Ok(String::new());
    }
    let mut identity = vec!["db", table, column];
    identity.extend_from_slice(keys);
    vault
        .seal(&identity, value.as_bytes())
        .map_err(secret_error)
}

pub(crate) fn open_db(
    vault: &VaultContext,
    table: &str,
    column: &str,
    keys: &[&str],
    value: &str,
) -> Result<String, AppError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let mut identity = vec!["db", table, column];
    identity.extend_from_slice(keys);
    let bytes = vault.open(&identity, value).map_err(secret_error)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| AppError::Config("secret.invalid_text".into()))
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

/// Only lifecycle-owned staging may call this with a plaintext source.
/// The savepoint makes authentication failure leave even the staging state intact.
pub(crate) fn transform_database(
    conn: &Connection,
    source: Option<&VaultContext>,
    target: &VaultContext,
) -> Result<(), AppError> {
    conn.execute_batch("SAVEPOINT loongport_secret_transform")
        .map_err(database_error)?;
    let result = transform_values(conn, source, target);
    match result {
        Ok(()) => conn
            .execute_batch("RELEASE loongport_secret_transform")
            .map_err(database_error),
        Err(error) => {
            conn.execute_batch(
                "ROLLBACK TO loongport_secret_transform; RELEASE loongport_secret_transform",
            )
            .map_err(database_error)?;
            Err(error)
        }
    }
}

/// 按表名查身份列（导出投影给键起名用）；未知表返回空。
pub(crate) fn table_identity_columns(table: &str) -> &'static [&'static str] {
    TABLES
        .iter()
        .find(|entry| entry.name == table)
        .map(|entry| entry.identity_columns)
        .unwrap_or(&[])
}

/// 明文导出投影（只读）：按 `TABLES` 唯源遍历，把每个受保护值解密成明文交给
/// 调用方收集；不写库、不改密文。与迁移/轮换走同一个 walker，覆盖面不分叉。
pub(crate) fn collect_plaintext_values(
    conn: &Connection,
    vault: &VaultContext,
    mut collect: impl FnMut(&str, &str, Vec<String>, String),
) -> Result<(), AppError> {
    process_values(conn, |table, column, keys: &[&str], value| {
        let owned: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();
        let plaintext = open_db(vault, table, column, keys, value)?;
        collect(table, column, owned, plaintext);
        Ok(None)
    })
}

/// Authenticate every protected value without changing SQLite state or ciphertext.
pub(crate) fn validate_database(conn: &Connection, vault: &VaultContext) -> Result<(), AppError> {
    process_values(conn, |table, column, keys, value| {
        open_db(vault, table, column, keys, value)?;
        Ok(None)
    })
}

/// Explicit reset only: discard protected contents without an old key.
pub(crate) fn reset_database(conn: &Connection, target: &VaultContext) -> Result<usize, AppError> {
    let mut cleared = 0;
    process_values(conn, |table, column, keys, value| {
        if !value.is_empty() {
            cleared += 1;
        }
        let policy = &TABLES
            .iter()
            .find(|entry| entry.name == table)
            .ok_or_else(|| AppError::Config("secret.invalid_identity".into()))?
            .reset;
        match policy {
            ResetPolicy::EmptyObject => Ok(Some(seal_db(target, table, column, keys, "{}")?)),
            ResetPolicy::EmptyStrings => Ok(Some(String::new())),
            ResetPolicy::RemoveRows | ResetPolicy::RemoveProtectedSettings => Ok(None),
        }
    })?;
    for table in TABLES {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table.name],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if !exists {
            continue;
        }
        match table.reset {
            ResetPolicy::RemoveRows => {
                conn.execute(&format!("DELETE FROM \"{}\"", table.name), [])
                    .map_err(database_error)?;
            }
            ResetPolicy::RemoveProtectedSettings => {
                let mut stmt = conn
                    .prepare("SELECT key FROM settings")
                    .map_err(database_error)?;
                let keys = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(database_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(database_error)?;
                drop(stmt);
                for key in keys.into_iter().filter(|key| is_protected_setting(key)) {
                    conn.execute("DELETE FROM settings WHERE key=?1", [key])
                        .map_err(database_error)?;
                }
            }
            _ => {}
        }
    }
    Ok(cleared)
}

fn transform_values(
    conn: &Connection,
    source: Option<&VaultContext>,
    target: &VaultContext,
) -> Result<(), AppError> {
    process_values(conn, |table, column, keys, value| {
        let plaintext = match source {
            Some(source) => open_db(source, table, column, keys, value)?,
            None if value.starts_with("lpenc") => {
                return Err(AppError::Config("secret.source_key_required".into()))
            }
            None => value.to_owned(),
        };
        Ok(Some(seal_db(target, table, column, keys, &plaintext)?))
    })
}

fn process_values(
    conn: &Connection,
    mut process: impl FnMut(&str, &str, &[&str], &str) -> Result<Option<String>, AppError>,
) -> Result<(), AppError> {
    for table in TABLES {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table.name],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if !exists {
            continue;
        }
        let where_clause = table
            .identity_columns
            .iter()
            .enumerate()
            .map(|(i, key)| format!("\"{key}\" = ?{}", i + 2))
            .collect::<Vec<_>>()
            .join(" AND ");
        for column in table.columns {
            let mut fields = table.identity_columns.to_vec();
            fields.push(column);
            let select = fields
                .iter()
                .map(|field| format!("\"{field}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("SELECT {select} FROM \"{}\"", table.name);
            let mut stmt = conn.prepare(&sql).map_err(database_error)?;
            let rows = stmt
                .query_map([], |row| {
                    (0..fields.len())
                        .map(|i| row.get::<_, Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(database_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(database_error)?;
            drop(stmt);
            for mut row in rows {
                let value = row
                    .pop()
                    .ok_or_else(|| AppError::Config("secret.invalid_row".into()))?;
                let keys = row
                    .iter()
                    .map(|value| match value {
                        Value::Text(text) => Ok(text.clone()),
                        Value::Integer(number) => Ok(number.to_string()),
                        _ => Err(AppError::Config("secret.invalid_identity".into())),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if table.name == "settings" && !is_protected_setting(&keys[0]) {
                    continue;
                }
                let value = match value {
                    Value::Null => continue,
                    Value::Text(value) => value,
                    _ => return Err(AppError::Config("secret.invalid_text".into())),
                };
                let key_refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
                if let Some(replacement) = process(table.name, column, &key_refs, &value)? {
                    let mut params = vec![Value::Text(replacement)];
                    params.extend(row);
                    conn.execute(
                        &format!(
                            "UPDATE \"{}\" SET \"{column}\" = ?1 WHERE {where_clause}",
                            table.name
                        ),
                        params_from_iter(params),
                    )
                    .map_err(database_error)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_encrypts_real_storage_and_preserves_plaintext_behavior() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE providers (id TEXT, app_type TEXT, settings_config TEXT, meta TEXT, PRIMARY KEY(id,app_type)); INSERT INTO providers VALUES ('first','codex','{\"api_key\":\"canary-credential\"}','{}');").unwrap();
        let vault = super::super::VaultContext::generate().unwrap();
        transform_database(&conn, None, &vault).unwrap();
        let raw: String = conn
            .query_row("SELECT settings_config FROM providers", [], |r| r.get(0))
            .unwrap();
        assert!(!raw.contains("canary-credential"));
        let opened = vault
            .open(
                &["db", "providers", "settings_config", "first", "codex"],
                &raw,
            )
            .unwrap();
        assert_eq!(&**opened, br#"{"api_key":"canary-credential"}"#);
        let other = super::super::VaultContext::generate().unwrap();
        transform_database(&conn, Some(&vault), &other).unwrap();
        let moved: String = conn
            .query_row("SELECT settings_config FROM providers", [], |r| r.get(0))
            .unwrap();
        assert!(vault
            .open(
                &["db", "providers", "settings_config", "first", "codex"],
                &moved
            )
            .is_err());
        assert_eq!(
            &**other
                .open(
                    &["db", "providers", "settings_config", "first", "codex"],
                    &moved
                )
                .unwrap(),
            &**opened
        );
    }

    #[test]
    fn validation_is_read_only_and_rejects_plaintext_and_wrong_keys() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT); INSERT INTO settings VALUES ('common_config_claude','validation-canary'),('official_providers_seeded','1')").unwrap();
        let vault = VaultContext::generate().unwrap();
        assert!(validate_database(&conn, &vault).is_err());
        transform_database(&conn, None, &vault).unwrap();
        let before: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='common_config_claude'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute_batch("PRAGMA query_only=ON").unwrap();
        validate_database(&conn, &vault).unwrap();
        assert!(validate_database(&conn, &VaultContext::generate().unwrap()).is_err());
        let after: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key='common_config_claude'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        assert!(!after.contains("validation-canary"));
    }

    #[test]
    fn credential_settings_are_selected_without_encrypting_control_flags() {
        for key in [
            "universal_providers",
            "global_proxy_url",
            "claude_desktop_gateway_token",
            "common_config_claude",
            "common_config_codex",
            "common_config_gemini",
        ] {
            assert!(
                is_protected_setting(key),
                "unprotected credential setting: {key}"
            );
        }
        for key in [
            "common_config_claude_cleared",
            "common_config_legacy_migrated_v1",
            "official_providers_seeded",
        ] {
            assert!(!is_protected_setting(key), "control flag selected: {key}");
        }
    }
}
