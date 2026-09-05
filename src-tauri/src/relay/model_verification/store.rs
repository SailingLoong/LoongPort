use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::model_verification::{
        history,
        types::{
            verdict_severity, ProbeDiagnostic, TargetKey, TargetScope, Verdict, VerificationReport,
            VerificationSource, RULES_VERSION,
        },
        verdict::{merge_passive_over, report_precedes},
        MODEL_VERIFICATION_ENABLED,
    },
};
use rusqlite::{params, params_from_iter, Connection};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

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
            active_diagnostics_json TEXT,
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
    if !MODEL_VERIFICATION_ENABLED {
        return Ok(());
    }
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
            rules_version, active_checked_at, updated_at, active_diagnostics_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)
        ON CONFLICT(provider_id, app_type, model) DO UPDATE SET
            active_report_json = excluded.active_report_json,
            verdict = excluded.verdict,
            evidence_level = excluded.evidence_level,
            rules_version = excluded.rules_version,
            active_checked_at = excluded.active_checked_at,
            updated_at = excluded.updated_at,
            active_diagnostics_json = NULL",
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

/// 被动异常报告落库（写 passive 列；主动列只归手动验真）。
///
/// 防降级：现有合并 verdict 比新被动信号更严重（如已存 Anomaly、来的是
/// Suspicious）时整笔跳过（含 history）——异常一旦立住，不被次级信号稀释。
/// verdict 列同步维护为两源合并后的对外判定（当前无独立读者，保持列语义
/// 不撒谎，供将来直接 join）。
pub fn upsert_passive(db: &Database, report: &VerificationReport) -> Result<bool, AppError> {
    if !MODEL_VERIFICATION_ENABLED {
        return Ok(false);
    }
    let passive_report_json = serde_json::to_string(report)
        .map_err(|error| AppError::Config(format!("序列化验证报告失败: {error}")))?;
    let mut conn = lock_conn!(db.conn);
    let transaction = conn
        .transaction()
        .map_err(|error| AppError::Database(format!("开始保存被动验证结果失败: {error}")))?;

    // rules_version 过滤：旧规则下的存量报告视为不存在（升版即作废），
    // 防降级比较也不会拿旧规则的重判定压制新信号。
    let (active_json, existing_passive_json): (Option<String>, Option<String>) = transaction
        .query_row(
            "SELECT active_report_json, passive_aggregate_json
             FROM model_verification_results
             WHERE provider_id = ?1 AND app_type = ?2 AND model = ?3
               AND rules_version = ?4",
            params![
                report.target.provider_id,
                report.target.app_type,
                report.target.model,
                RULES_VERSION,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((None, None));
    let parse = |json: Option<String>| -> Option<VerificationReport> {
        json.and_then(|value| serde_json::from_str(&value).ok())
    };
    let active = parse(active_json);
    let existing_passive = parse(existing_passive_json);
    let merged =
        merge_passive_over(active.as_ref(), Some(report)).expect("有被动输入时合并必有结果");
    if let Some(before) = merge_passive_over(active.as_ref(), existing_passive.as_ref()) {
        if verdict_severity(merged.verdict) < verdict_severity(before.verdict) {
            return Ok(false);
        }
    }

    let verdict = serde_json::to_string(&merged.verdict)
        .map_err(|error| AppError::Config(format!("序列化验证结论失败: {error}")))?;
    let evidence_level = serde_json::to_string(&merged.evidence_level)
        .map_err(|error| AppError::Config(format!("序列化证据等级失败: {error}")))?;
    transaction
        .execute(
            "INSERT INTO model_verification_results (
            provider_id, app_type, model, passive_aggregate_json, verdict, evidence_level,
            rules_version, passive_observed_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(provider_id, app_type, model) DO UPDATE SET
            passive_aggregate_json = excluded.passive_aggregate_json,
            verdict = excluded.verdict,
            evidence_level = excluded.evidence_level,
            rules_version = excluded.rules_version,
            passive_observed_at = excluded.passive_observed_at,
            updated_at = excluded.updated_at",
            params![
                report.target.provider_id,
                report.target.app_type,
                report.target.model,
                passive_report_json,
                verdict.trim_matches('"'),
                evidence_level.trim_matches('"'),
                report.rules_version,
                report.checked_at,
                report.checked_at,
            ],
        )
        .map_err(|error| AppError::Database(format!("保存被动验证结果失败: {error}")))?;
    history::insert(&transaction, VerificationSource::Passive, report)?;
    history::prune(
        &transaction,
        &report.target.provider_id,
        &report.target.app_type,
    )?;
    transaction
        .commit()
        .map_err(|error| AppError::Database(format!("提交被动验证结果失败: {error}")))?;
    Ok(true)
}

/// 两源 JSON 列 → 合并后的单份报告（严重者赢，同severity 被动赢）。
fn merge_json_reports(
    active_json: Option<String>,
    passive_json: Option<String>,
) -> Result<Option<VerificationReport>, AppError> {
    let active = active_json
        .map(|json| serde_json::from_str::<VerificationReport>(&json))
        .transpose()
        .map_err(|error| AppError::Config(format!("解析验证报告失败: {error}")))?;
    let passive = passive_json
        .map(|json| serde_json::from_str::<VerificationReport>(&json))
        .transpose()
        .map_err(|error| AppError::Config(format!("解析验证报告失败: {error}")))?;
    Ok(merge_passive_over(active.as_ref(), passive.as_ref()).cloned())
}

pub fn list_for_providers(
    db: &Database,
    app_type: &str,
    provider_ids: &[String],
) -> Result<Vec<VerificationReport>, AppError> {
    if !MODEL_VERIFICATION_ENABLED || provider_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; provider_ids.len()].join(", ");
    let sql = format!(
        "SELECT active_report_json, passive_aggregate_json FROM model_verification_results
         WHERE app_type = ? AND provider_id IN ({placeholders}) AND rules_version = ?
         ORDER BY provider_id, model"
    );
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Database(format!("查询模型验证结果失败: {error}")))?;
    let mut bindings = vec![app_type.to_string()];
    bindings.extend(provider_ids.iter().cloned());
    bindings.push(RULES_VERSION.to_string());
    let report_pairs = statement
        .query_map(params_from_iter(bindings), |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(|error| AppError::Database(format!("读取模型验证结果失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证结果失败: {error}")))?;

    report_pairs
        .into_iter()
        .filter_map(|(active_json, passive_json)| {
            merge_json_reports(active_json, passive_json).transpose()
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
    if !MODEL_VERIFICATION_ENABLED || provider_ids.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; provider_ids.len()].join(", ");
    let sql = format!(
        "SELECT active_report_json, passive_aggregate_json FROM model_verification_results
         WHERE provider_id IN ({placeholders}) AND rules_version = ?
         ORDER BY provider_id, app_type, model"
    );
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Database(format!("查询模型验证结果失败: {error}")))?;
    let mut bindings = provider_ids.to_vec();
    bindings.push(RULES_VERSION.to_string());
    let report_pairs = statement
        .query_map(params_from_iter(bindings), |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(|error| AppError::Database(format!("读取模型验证结果失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证结果失败: {error}")))?;

    report_pairs
        .into_iter()
        .filter_map(|(active_json, passive_json)| {
            merge_json_reports(active_json, passive_json).transpose()
        })
        .collect()
}

/// 把失败腿诊断挂到刚落库的 active 报告行上（新一次 upsert 会先清空）。
/// 诊断是 debug 边车：不参与判定，不进合并语义，passive 写入不触碰此列。
pub fn attach_diagnostics(
    db: &Database,
    target: &TargetKey,
    diagnostics: &[ProbeDiagnostic],
) -> Result<(), AppError> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string(diagnostics)
        .map_err(|error| AppError::Config(format!("序列化验真诊断失败: {error}")))?;
    let conn = lock_conn!(db.conn);
    conn.execute(
        "UPDATE model_verification_results SET active_diagnostics_json = ?1
         WHERE provider_id = ?2 AND app_type = ?3 AND model = ?4",
        params![json, target.provider_id, target.app_type, target.model],
    )
    .map_err(|error| AppError::Database(format!("保存验真诊断失败: {error}")))?;
    Ok(())
}

/// 读取某条 active 报告的诊断边车（无诊断返回空）。
pub fn active_diagnostics(
    db: &Database,
    provider_id: &str,
    app_type: &str,
    model: &str,
) -> Result<Vec<ProbeDiagnostic>, AppError> {
    let conn = lock_conn!(db.conn);
    let json: Option<String> = conn
        .query_row(
            "SELECT active_diagnostics_json FROM model_verification_results
             WHERE provider_id = ?1 AND app_type = ?2 AND model = ?3",
            params![provider_id, app_type, model],
            |row| row.get(0),
        )
        .unwrap_or(None);
    Ok(json
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default())
}

/// 档位（provider）跨模型的读侧聚合：每个 provider 取最严重判定
/// （严重者赢、同级取更新，规则见 `verdict::report_precedes`）。
/// 看板等消费方只调这一条，不各自展开验真合并规则。
pub fn worst_verdict_by_provider(
    db: &Database,
    provider_ids: &[String],
) -> Result<HashMap<String, Verdict>, AppError> {
    let mut worst: HashMap<String, VerificationReport> = HashMap::new();
    for report in list_for_provider_ids(db, provider_ids)? {
        match worst.get(&report.target.provider_id) {
            Some(current) => {
                if report_precedes(&report, current) {
                    worst.insert(report.target.provider_id.clone(), report);
                }
            }
            None => {
                worst.insert(report.target.provider_id.clone(), report);
            }
        }
    }
    Ok(worst
        .into_iter()
        .map(|(provider_id, report)| (provider_id, report.verdict))
        .collect())
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
