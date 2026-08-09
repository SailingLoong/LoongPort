use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::model_verification::types::{TargetScope, VerificationReport, VerificationSummary},
};
use rusqlite::{params, params_from_iter, Connection};

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
) -> Result<Vec<VerificationSummary>, AppError> {
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
                .map(|report| report.summary())
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
