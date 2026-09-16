//! 显式明文导出：用户主动触发的**所有权出口**——一次性动作，不是存储模式。
//!
//! 为什么有它：加密是强制的（正常运行无明文路径），但用户应始终能拿回自己的
//! 明文凭据（迁移到其他工具、核对、留存）。导出物含全部敏感值，仅经用户在
//! UI 确认后触发，落盘 0600 私密权限。受保护清单与迁移共用 `inventory::TABLES`
//! 唯源——加密保护了哪些列，导出就覆盖哪些列，不会分叉。

use super::files::AUTH_FILES;
use super::inventory;
use super::session::SecretSession;
use crate::error::AppError;
use serde_json::{json, Map, Value};

pub(crate) fn build_plaintext_export(session: &SecretSession) -> Result<String, AppError> {
    let vault = session.read()?;
    let db_path = session.root().join(crate::config::DB_FILE_NAME);
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(e.to_string()))?;

    // database = {表名: [{身份列…, 受保护列: 明文}, …]}；行按身份列去重聚合
    //（同一行的多列秘密合并进同一个对象，读的人一眼一行）。BTreeMap 让键序稳定。
    let mut database: std::collections::BTreeMap<
        String,
        Vec<std::collections::BTreeMap<String, String>>,
    > = std::collections::BTreeMap::new();
    inventory::collect_plaintext_values(
        &conn,
        &vault,
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

    // OAuth 凭据文件（磁盘上是密文）：一并解码为明文内容。
    let mut files = Map::new();
    for file in AUTH_FILES {
        if let Ok(bytes) = std::fs::read(file.path(session)) {
            if let Ok(plaintext) = file.decode(&vault, &bytes) {
                files.insert(
                    file.filename().to_string(),
                    Value::String(String::from_utf8_lossy(&plaintext).into_owned()),
                );
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn plaintext_export_covers_protected_locations_and_skips_plain_settings() {
        let root = tempfile::tempdir().unwrap();
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
