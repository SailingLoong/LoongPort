use rusqlite::{params, Connection};

use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::model_verification::types::{
        TargetKey, TargetScope, VerificationHistoryEntry, VerificationReport, VerificationSource,
    },
};

const TABLE: &str = "model_verification_history";
const LIMIT: i64 = 5;

pub(crate) fn create_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS model_verification_history (
            id INTEGER PRIMARY KEY,
            provider_id TEXT NOT NULL,
            app_type TEXT NOT NULL,
            model TEXT NOT NULL,
            source TEXT NOT NULL CHECK (source IN ('active', 'runtime')),
            verdict TEXT NOT NULL,
            evidence_level TEXT NOT NULL,
            facts_json TEXT NOT NULL,
            rules_version INTEGER NOT NULL,
            checked_at INTEGER NOT NULL,
            FOREIGN KEY (provider_id, app_type) REFERENCES providers(id, app_type) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_model_verification_history_scope_order
        ON model_verification_history (provider_id, app_type, checked_at DESC, id DESC);",
    )
    .map_err(|error| AppError::Database(format!("创建 {TABLE} 表失败: {error}")))?;
    Ok(())
}

pub fn list(db: &Database, scope: &TargetScope) -> Result<Vec<VerificationHistoryEntry>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(
            "SELECT provider_id, app_type, model, source, verdict, evidence_level,
                    facts_json, rules_version, checked_at
             FROM model_verification_history
             WHERE provider_id = ?1 AND app_type = ?2
             ORDER BY checked_at DESC, id DESC
             LIMIT ?3",
        )
        .map_err(|error| AppError::Database(format!("查询模型验证历史失败: {error}")))?;
    let rows = statement
        .query_map(params![scope.provider_id, scope.app_type, LIMIT], |row| {
            Ok((
                TargetKey::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ),
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i32>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })
        .map_err(|error| AppError::Database(format!("读取模型验证历史失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证历史失败: {error}")))?;

    rows.into_iter()
        .map(
            |(target, source, verdict, evidence_level, facts_json, rules_version, checked_at)| {
                let facts = serde_json::from_str(&facts_json).map_err(|error| {
                    AppError::Config(format!("解析模型验证历史依据失败: {error}"))
                })?;
                Ok(VerificationHistoryEntry {
                    source: VerificationSource::try_from(source.as_str())
                        .map_err(|_| AppError::Config("解析模型验证来源失败".into()))?,
                    report: VerificationReport {
                        target,
                        verdict: crate::relay::model_verification::types::Verdict::try_from(
                            verdict.as_str(),
                        )
                        .map_err(|_| AppError::Config("解析验证结论失败".into()))?,
                        evidence_level:
                            crate::relay::model_verification::types::EvidenceLevel::try_from(
                                evidence_level.as_str(),
                            )
                            .map_err(|_| AppError::Config("解析验证证据等级失败".into()))?,
                        facts,
                        rules_version,
                        checked_at,
                    },
                })
            },
        )
        .collect()
}

pub(super) fn insert(
    conn: &Connection,
    source: VerificationSource,
    report: &VerificationReport,
) -> Result<(), AppError> {
    let facts_json = serde_json::to_string(&report.facts)
        .map_err(|error| AppError::Config(format!("序列化模型验证历史依据失败: {error}")))?;
    conn.execute(
        "INSERT INTO model_verification_history (
            provider_id, app_type, model, source, verdict, evidence_level,
            facts_json, rules_version, checked_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &report.target.provider_id,
            &report.target.app_type,
            &report.target.model,
            source.as_str(),
            report.verdict.as_str(),
            report.evidence_level.as_str(),
            facts_json,
            report.rules_version,
            report.checked_at,
        ],
    )
    .map_err(|error| AppError::Database(format!("保存模型验证历史失败: {error}")))?;
    Ok(())
}

pub(super) fn prune(conn: &Connection, provider_id: &str, app_type: &str) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM model_verification_history
         WHERE provider_id = ?1 AND app_type = ?2
           AND id NOT IN (
             SELECT id FROM model_verification_history
             WHERE provider_id = ?1 AND app_type = ?2
             ORDER BY checked_at DESC, id DESC
             LIMIT ?3
           )",
        params![provider_id, app_type, LIMIT],
    )
    .map_err(|error| AppError::Database(format!("裁剪模型验证历史失败: {error}")))?;
    Ok(())
}

pub(super) fn clear(conn: &Connection, scope: &TargetScope) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM model_verification_history WHERE provider_id = ?1 AND app_type = ?2",
        params![scope.provider_id, scope.app_type],
    )
    .map_err(|error| AppError::Database(format!("清除模型验证历史失败: {error}")))?;
    Ok(())
}
