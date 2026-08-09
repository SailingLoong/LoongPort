use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::model_verification::types::{
        ProxyLease, RuntimeVerificationSetting, TargetScope, VerificationReport,
    },
};
use rusqlite::{params, params_from_iter, Connection};
use std::time::{SystemTime, UNIX_EPOCH};

const RESULTS_TABLE: &str = "model_verification_results";

pub(crate) fn create_results_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS model_verification_results (
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            active_report_json TEXT,
            passive_aggregate_json TEXT,
            verdict TEXT NOT NULL,
            evidence_level TEXT NOT NULL,
            rules_version INTEGER NOT NULL,
            active_checked_at INTEGER,
            passive_observed_at INTEGER,
            updated_at INTEGER NOT NULL,
            notified_fingerprints_json TEXT NOT NULL DEFAULT '[]',
            PRIMARY KEY (provider_id, app_type, model),
            FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
        )",
        [],
    )
    .map_err(|error| AppError::Database(format!("创建 {RESULTS_TABLE} 表失败: {error}")))?;
    Ok(())
}

const SETTINGS_TABLE: &str = "model_verification_settings";
const LEASES_TABLE: &str = "model_verification_proxy_leases";

pub(crate) fn create_runtime_tables(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS model_verification_settings (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            runtime_auto_enabled INTEGER NOT NULL DEFAULT 0 CHECK (runtime_auto_enabled IN (0, 1)),
            updated_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|error| AppError::Database(format!("创建 {SETTINGS_TABLE} 表失败: {error}")))?;
    conn.execute(
        "INSERT OR IGNORE INTO model_verification_settings
         (singleton, runtime_auto_enabled, updated_at) VALUES (1, 0, ?1)",
        [unix_seconds()],
    )
    .map_err(|error| AppError::Database(format!("初始化 {SETTINGS_TABLE} 表失败: {error}")))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS model_verification_proxy_leases (
            app_type TEXT PRIMARY KEY CHECK (app_type IN ('codex', 'claude')),
            acquired_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|error| AppError::Database(format!("创建 {LEASES_TABLE} 表失败: {error}")))?;
    Ok(())
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn validate_lease_app(app_type: &str) -> Result<(), AppError> {
    match app_type {
        "codex" | "claude" => Ok(()),
        _ => Err(AppError::InvalidInput(format!(
            "unsupported model verification app type: {app_type}"
        ))),
    }
}

pub fn get_runtime_setting(db: &Database) -> Result<RuntimeVerificationSetting, AppError> {
    let conn = lock_conn!(db.conn);
    conn.query_row(
        "SELECT runtime_auto_enabled, updated_at
         FROM model_verification_settings WHERE singleton = 1",
        [],
        |row| {
            let enabled: i64 = row.get(0)?;
            Ok(RuntimeVerificationSetting {
                runtime_auto_enabled: enabled != 0,
                updated_at: row.get(1)?,
            })
        },
    )
    .map_err(|error| AppError::Database(format!("读取运行时验证设置失败: {error}")))
}

pub fn set_runtime_setting(
    db: &Database,
    runtime_auto_enabled: bool,
) -> Result<RuntimeVerificationSetting, AppError> {
    let updated_at = unix_seconds();
    let conn = lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO model_verification_settings
         (singleton, runtime_auto_enabled, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(singleton) DO UPDATE SET
           runtime_auto_enabled = excluded.runtime_auto_enabled,
           updated_at = excluded.updated_at",
        params![runtime_auto_enabled as i64, updated_at],
    )
    .map_err(|error| AppError::Database(format!("保存运行时验证设置失败: {error}")))?;
    Ok(RuntimeVerificationSetting {
        runtime_auto_enabled,
        updated_at,
    })
}

pub fn list_leases(db: &Database) -> Result<Vec<ProxyLease>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(
            "SELECT app_type, acquired_at FROM model_verification_proxy_leases
             ORDER BY app_type",
        )
        .map_err(|error| AppError::Database(format!("查询模型验证代理租约失败: {error}")))?;
    let leases = statement
        .query_map([], |row| {
            Ok(ProxyLease {
                app_type: row.get(0)?,
                acquired_at: row.get(1)?,
            })
        })
        .map_err(|error| AppError::Database(format!("读取模型验证代理租约失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证代理租约失败: {error}")))?;
    Ok(leases)
}

pub fn insert_lease(db: &Database, app_type: &str, acquired_at: i64) -> Result<(), AppError> {
    validate_lease_app(app_type)?;
    let conn = lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO model_verification_proxy_leases (app_type, acquired_at)
         VALUES (?1, ?2)
         ON CONFLICT(app_type) DO UPDATE SET acquired_at = excluded.acquired_at",
        params![app_type, acquired_at],
    )
    .map_err(|error| AppError::Database(format!("保存模型验证代理租约失败: {error}")))?;
    Ok(())
}

pub fn delete_lease(db: &Database, app_type: &str) -> Result<(), AppError> {
    validate_lease_app(app_type)?;
    let conn = lock_conn!(db.conn);
    conn.execute(
        "DELETE FROM model_verification_proxy_leases WHERE app_type = ?1",
        [app_type],
    )
    .map_err(|error| AppError::Database(format!("删除模型验证代理租约失败: {error}")))?;
    Ok(())
}

pub fn has_lease(db: &Database, app_type: &str) -> Result<bool, AppError> {
    validate_lease_app(app_type)?;
    let conn = lock_conn!(db.conn);
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM model_verification_proxy_leases WHERE app_type = ?1
            )",
            [app_type],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Database(format!("查询模型验证代理租约失败: {error}")))?;
    Ok(exists != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::model_verification::types::{RuntimeAppReason, RuntimeAppStatus};

    #[test]
    fn runtime_setting_defaults_off() {
        let db = Database::memory().unwrap();
        let setting = get_runtime_setting(&db).unwrap();
        assert!(!setting.runtime_auto_enabled);
        assert!(setting.updated_at > 0);
    }

    #[test]
    fn runtime_setting_updates_are_persistent() {
        let db = Database::memory().unwrap();
        set_runtime_setting(&db, true).unwrap();
        assert!(get_runtime_setting(&db).unwrap().runtime_auto_enabled);
        set_runtime_setting(&db, false).unwrap();
        assert!(!get_runtime_setting(&db).unwrap().runtime_auto_enabled);
    }

    #[test]
    fn lease_is_ownership_not_proxy_state() {
        let db = Database::memory().unwrap();
        insert_lease(&db, "codex", 1_786_214_400).unwrap();
        assert!(has_lease(&db, "codex").unwrap());
        let proxy = futures::executor::block_on(db.get_proxy_config_for_app("codex")).unwrap();
        assert!(!proxy.enabled);
    }

    #[test]
    fn lease_upsert_keeps_one_row_per_supported_app() {
        let db = Database::memory().unwrap();
        insert_lease(&db, "codex", 10).unwrap();
        insert_lease(&db, "codex", 20).unwrap();
        insert_lease(&db, "claude", 30).unwrap();
        assert_eq!(list_leases(&db).unwrap().len(), 2);
        assert_eq!(
            list_leases(&db)
                .unwrap()
                .into_iter()
                .find(|lease| lease.app_type == "codex")
                .unwrap()
                .acquired_at,
            20
        );
    }

    #[test]
    fn unsupported_lease_app_is_rejected_before_sql() {
        let db = Database::memory().unwrap();
        assert!(insert_lease(&db, "gemini", 10).is_err());
        assert!(delete_lease(&db, "gemini").is_err());
        assert!(has_lease(&db, "gemini").is_err());
        assert!(list_leases(&db).unwrap().is_empty());
    }

    #[test]
    fn runtime_types_use_finite_camel_case_values() {
        assert_eq!(
            serde_json::to_value(RuntimeAppStatus::Active).unwrap(),
            serde_json::json!("active")
        );
        assert_eq!(
            serde_json::to_value(RuntimeAppReason::CurrentProviderUnsupported).unwrap(),
            serde_json::json!("currentProviderUnsupported")
        );
    }
}

pub fn upsert_active(db: &Database, report: &VerificationReport) -> Result<(), AppError> {
    let active_report_json = serde_json::to_string(report)
        .map_err(|error| AppError::Config(format!("序列化验证报告失败: {error}")))?;
    let verdict = serde_json::to_string(&report.verdict)
        .map_err(|error| AppError::Config(format!("序列化验证结论失败: {error}")))?;
    let evidence_level = serde_json::to_string(&report.evidence_level)
        .map_err(|error| AppError::Config(format!("序列化证据等级失败: {error}")))?;
    let conn = lock_conn!(db.conn);

    conn.execute(
        "INSERT INTO model_verification_results (
            provider_id, app_type, model, active_report_json, verdict, evidence_level,
            rules_version, active_checked_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(provider_id, app_type, model) DO UPDATE SET
            active_report_json = excluded.active_report_json,
            verdict = excluded.verdict,
            evidence_level = excluded.evidence_level,
            rules_version = excluded.rules_version,
            active_checked_at = excluded.active_checked_at,
            updated_at = excluded.updated_at",
        params![
            report.target.provider_id,
            report.target.app_type,
            report.target.model,
            active_report_json,
            verdict.trim_matches('"'),
            evidence_level.trim_matches('"'),
            report.rules_version,
            report.checked_at,
            report.checked_at,
        ],
    )
    .map_err(|error| AppError::Database(format!("保存模型验证结果失败: {error}")))?;
    Ok(())
}

pub fn list_for_providers(
    db: &Database,
    app_type: &str,
    provider_ids: &[String],
) -> Result<Vec<VerificationReport>, AppError> {
    if provider_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; provider_ids.len()].join(", ");
    let sql = format!(
        "SELECT active_report_json FROM model_verification_results
         WHERE app_type = ? AND provider_id IN ({placeholders})
         ORDER BY provider_id, model"
    );
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Database(format!("查询模型验证结果失败: {error}")))?;
    let report_jsons = statement
        .query_map(
            params_from_iter(
                std::iter::once(app_type).chain(provider_ids.iter().map(String::as_str)),
            ),
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(|error| AppError::Database(format!("读取模型验证结果失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证结果失败: {error}")))?;

    report_jsons
        .into_iter()
        .flatten()
        .map(|report_json| {
            serde_json::from_str::<VerificationReport>(&report_json)
                .map_err(|error| AppError::Config(format!("解析验证报告失败: {error}")))
        })
        .collect()
}

/// Lists the latest sanitized active reports for the requested providers across every app.
///
/// Only placeholder count is formatted into the SQL. Provider IDs are always bound values.
pub fn list_for_provider_ids(
    db: &Database,
    provider_ids: &[String],
) -> Result<Vec<VerificationReport>, AppError> {
    if provider_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; provider_ids.len()].join(", ");
    let sql = format!(
        "SELECT active_report_json FROM model_verification_results
         WHERE provider_id IN ({placeholders})
         ORDER BY provider_id, app_type, model"
    );
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Database(format!("查询模型验证结果失败: {error}")))?;
    let report_jsons = statement
        .query_map(params_from_iter(provider_ids.iter()), |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(|error| AppError::Database(format!("读取模型验证结果失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证结果失败: {error}")))?;

    report_jsons
        .into_iter()
        .flatten()
        .map(|report_json| {
            serde_json::from_str::<VerificationReport>(&report_json)
                .map_err(|error| AppError::Config(format!("解析验证报告失败: {error}")))
        })
        .collect()
}

pub fn clear_scope(db: &Database, scope: &TargetScope) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    conn.execute(
        "DELETE FROM model_verification_results WHERE provider_id = ?1 AND app_type = ?2",
        params![scope.provider_id, scope.app_type],
    )
    .map_err(|error| AppError::Database(format!("清除模型验证结果失败: {error}")))?;
    Ok(())
}
