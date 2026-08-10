use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::model_verification::{
        history,
        types::{TargetScope, VerificationReport, VerificationSource},
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

/// Creates tables used by releases that supported passive runtime verification.
///
/// Kept only so databases can advance through the historical v6 migration. The current
/// runtime no longer uses these tables; unresolved leases are consumed by startup cleanup.
pub(crate) fn create_legacy_runtime_tables(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS model_verification_settings (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            runtime_auto_enabled INTEGER NOT NULL DEFAULT 0 CHECK (runtime_auto_enabled IN (0, 1)),
            updated_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|error| AppError::Database(format!("创建旧模型验证设置表失败: {error}")))?;
    conn.execute(
        "INSERT OR IGNORE INTO model_verification_settings
         (singleton, runtime_auto_enabled, updated_at) VALUES (1, 0, ?1)",
        [unix_seconds()],
    )
    .map_err(|error| AppError::Database(format!("初始化旧模型验证设置表失败: {error}")))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS model_verification_proxy_leases (
            app_type TEXT PRIMARY KEY CHECK (app_type IN ('codex', 'claude')),
            acquired_at INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|error| AppError::Database(format!("创建旧模型验证代理租约表失败: {error}")))?;
    Ok(())
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

pub fn upsert_active(db: &Database, report: &VerificationReport) -> Result<(), AppError> {
    let active_report_json = serde_json::to_string(report)
        .map_err(|error| AppError::Config(format!("序列化验证报告失败: {error}")))?;
    let verdict = serde_json::to_string(&report.verdict)
        .map_err(|error| AppError::Config(format!("序列化验证结论失败: {error}")))?;
    let evidence_level = serde_json::to_string(&report.evidence_level)
        .map_err(|error| AppError::Config(format!("序列化证据等级失败: {error}")))?;
    let mut conn = lock_conn!(db.conn);
    let transaction = conn
        .transaction()
        .map_err(|error| AppError::Database(format!("开始保存模型验证结果失败: {error}")))?;

    transaction
        .execute(
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
    history::insert(&transaction, VerificationSource::Active, report)?;
    history::prune(
        &transaction,
        &report.target.provider_id,
        &report.target.app_type,
    )?;
    transaction
        .commit()
        .map_err(|error| AppError::Database(format!("提交模型验证结果失败: {error}")))?;
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
    let mut conn = lock_conn!(db.conn);
    let transaction = conn
        .transaction()
        .map_err(|error| AppError::Database(format!("开始清除模型验证结果失败: {error}")))?;
    transaction
        .execute(
            "DELETE FROM model_verification_results WHERE provider_id = ?1 AND app_type = ?2",
            params![scope.provider_id, scope.app_type],
        )
        .map_err(|error| AppError::Database(format!("清除模型验证结果失败: {error}")))?;
    history::clear(&transaction, scope)?;
    transaction
        .commit()
        .map_err(|error| AppError::Database(format!("提交清除模型验证结果失败: {error}")))?;
    Ok(())
}
