use crate::{
    database::{lock_conn, Database},
    error::AppError,
    relay::model_verification::{
        passive::{
            reduce_batch, resolve_with_active, AnomalyFingerprint, EvidenceBatch, PassiveAggregate,
        },
        types::{
            EvidenceLevel, ProxyLease, RuntimeAppType, RuntimeVerificationSetting, TargetKey,
            TargetScope, Verdict, VerificationReport, RULES_VERSION,
        },
        verdict::{self, MergedReport},
    },
};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
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

fn validate_lease_app(app_type: &str) -> Result<RuntimeAppType, AppError> {
    RuntimeAppType::try_from(app_type).map_err(|_| {
        AppError::InvalidInput(format!(
            "unsupported model verification app type: {app_type}"
        ))
    })
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
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| AppError::Database(format!("读取模型验证代理租约失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证代理租约失败: {error}")))?;
    rows.into_iter()
        .map(|(app_type, acquired_at)| {
            RuntimeAppType::try_from(app_type.as_str())
                .map(|app_type| ProxyLease {
                    app_type,
                    acquired_at,
                })
                .map_err(|_| {
                    AppError::Database(format!(
                        "model verification lease has unsupported app type: {app_type}"
                    ))
                })
        })
        .collect()
}

pub fn insert_lease(db: &Database, app_type: &str, acquired_at: i64) -> Result<(), AppError> {
    let app_type = validate_lease_app(app_type)?;
    let conn = lock_conn!(db.conn);
    conn.execute(
        "INSERT INTO model_verification_proxy_leases (app_type, acquired_at)
         VALUES (?1, ?2)
         ON CONFLICT(app_type) DO UPDATE SET acquired_at = excluded.acquired_at",
        params![app_type.as_str(), acquired_at],
    )
    .map_err(|error| AppError::Database(format!("保存模型验证代理租约失败: {error}")))?;
    Ok(())
}

pub fn delete_lease(db: &Database, app_type: &str) -> Result<(), AppError> {
    let app_type = validate_lease_app(app_type)?;
    let conn = lock_conn!(db.conn);
    conn.execute(
        "DELETE FROM model_verification_proxy_leases WHERE app_type = ?1",
        [app_type.as_str()],
    )
    .map_err(|error| AppError::Database(format!("删除模型验证代理租约失败: {error}")))?;
    Ok(())
}

pub fn has_lease(db: &Database, app_type: &str) -> Result<bool, AppError> {
    let app_type = validate_lease_app(app_type)?;
    let conn = lock_conn!(db.conn);
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM model_verification_proxy_leases WHERE app_type = ?1
            )",
            [app_type.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Database(format!("查询模型验证代理租约失败: {error}")))?;
    Ok(exists != 0)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::relay::model_verification::types::{
        EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, ProxyLease, RuntimeAppReason,
        RuntimeAppState, RuntimeAppStatus, RuntimeAppType, TargetKey, Verdict, VerificationReport,
        RULES_VERSION,
    };

    use crate::relay::model_verification::passive::EvidenceBatch;

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
                .find(|lease| lease.app_type == RuntimeAppType::Codex)
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
    fn list_leases_rejects_unsupported_persisted_app() -> Result<(), AppError> {
        let db = Database::memory().unwrap();
        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "PRAGMA ignore_check_constraints = ON;
                 INSERT INTO model_verification_proxy_leases (app_type, acquired_at)
                 VALUES ('gemini', 10);
                 PRAGMA ignore_check_constraints = OFF;",
            )
            .unwrap();
        }

        assert!(list_leases(&db).is_err());
        Ok(())
    }

    #[test]
    fn runtime_types_serialize_as_finite_camel_case_values() {
        assert_eq!(
            serde_json::to_value(RuntimeAppType::Codex).unwrap(),
            serde_json::json!("codex")
        );
        assert_eq!(
            serde_json::to_value(RuntimeAppType::Claude).unwrap(),
            serde_json::json!("claude")
        );
        assert!(serde_json::from_str::<RuntimeAppType>("\"gemini\"").is_err());
        assert_eq!(RuntimeAppType::Codex.as_str(), "codex");
        let state = RuntimeAppState {
            app_type: RuntimeAppType::Claude,
            status: RuntimeAppStatus::Waiting,
            reason: Some(RuntimeAppReason::CurrentProviderUnsupported),
        };
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::json!({
                "appType": "claude",
                "status": "waiting",
                "reason": "currentProviderUnsupported"
            })
        );
        assert_eq!(
            serde_json::to_value(ProxyLease {
                app_type: RuntimeAppType::Codex,
                acquired_at: 10,
            })
            .unwrap(),
            serde_json::json!({"appType": "codex", "acquiredAt": 10})
        );
        assert!(
            serde_json::from_value::<RuntimeAppState>(serde_json::json!({
                "appType": "gemini",
                "status": "active",
                "reason": null
            }))
            .is_err()
        );
        assert!(serde_json::from_value::<ProxyLease>(serde_json::json!({
            "appType": "gemini",
            "acquiredAt": 10
        }))
        .is_err());
        assert_eq!(
            serde_json::to_value(RuntimeAppStatus::Active).unwrap(),
            serde_json::json!("active")
        );
        assert_eq!(
            serde_json::to_value(RuntimeAppReason::CurrentProviderUnsupported).unwrap(),
            serde_json::json!("currentProviderUnsupported")
        );
    }

    #[test]
    fn passive_upsert_persists_the_merged_verdict_and_active_pass_resolves_it(
    ) -> Result<(), AppError> {
        let db = Database::memory().unwrap();
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config)
                 VALUES ('provider-a', 'codex', 'Provider', '{}')",
                [],
            )
            .unwrap();
        }
        let target = TargetKey::new("provider-a", "codex", "gpt-5.6-sol");
        let batch = EvidenceBatch::new(
            target.clone(),
            0,
            true,
            vec![EvidenceFact {
                code: EvidenceCode::ForeignProtocol,
                outcome: EvidenceOutcome::Failed,
            }],
            100,
        );

        assert_eq!(
            upsert_passive(&db, &batch).unwrap().verdict,
            Verdict::Anomaly
        );
        assert_eq!(
            list_for_provider_ids(&db, &["provider-a".into()])?[0].verdict,
            Verdict::Anomaly
        );
        let active = VerificationReport {
            target,
            verdict: Verdict::Trusted,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts: vec![EvidenceFact {
                code: EvidenceCode::ForeignProtocol,
                outcome: EvidenceOutcome::Passed,
            }],
            rules_version: RULES_VERSION,
            checked_at: 101,
        };
        upsert_active(&db, &active).unwrap();

        let conn = lock_conn!(db.conn);
        let verdict: String = conn
            .query_row(
                "SELECT verdict FROM model_verification_results
                 WHERE provider_id = 'provider-a' AND app_type = 'codex' AND model = 'gpt-5.6-sol'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(verdict, "trusted");
        drop(conn);
        assert_eq!(
            list_for_provider_ids(&db, &["provider-a".into()])?[0].verdict,
            Verdict::Trusted
        );
        Ok(())
    }
}

pub fn upsert_active(db: &Database, report: &VerificationReport) -> Result<(), AppError> {
    let active_report_json = serde_json::to_string(report)
        .map_err(|error| AppError::Config(format!("序列化验证报告失败: {error}")))?;
    let conn = lock_conn!(db.conn);
    let (mut passive, notified) = load_passive_state(&conn, &report.target)?;
    let cleared = passive
        .as_mut()
        .map(|aggregate| resolve_with_active(aggregate, report))
        .unwrap_or_default();
    let notified: Vec<_> = notified
        .into_iter()
        .filter(|fingerprint| !cleared.contains(fingerprint))
        .collect();
    let merged = verdict::merge(Some(report), passive.as_ref());
    let passive_aggregate_json = serialize_optional(&passive)?;

    conn.execute(
        "INSERT INTO model_verification_results (
            provider_id, app_type, model, active_report_json, passive_aggregate_json,
            verdict, evidence_level, rules_version, active_checked_at, updated_at,
            notified_fingerprints_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(provider_id, app_type, model) DO UPDATE SET
            active_report_json = excluded.active_report_json,
            passive_aggregate_json = excluded.passive_aggregate_json,
            verdict = excluded.verdict,
            evidence_level = excluded.evidence_level,
            rules_version = excluded.rules_version,
            active_checked_at = excluded.active_checked_at,
            updated_at = excluded.updated_at,
            notified_fingerprints_json = excluded.notified_fingerprints_json",
        params![
            &report.target.provider_id,
            &report.target.app_type,
            &report.target.model,
            active_report_json,
            passive_aggregate_json,
            verdict_name(merged.verdict),
            evidence_level_name(merged.evidence_level),
            report.rules_version,
            report.checked_at,
            report.checked_at,
            serialize_fingerprints(&notified)?,
        ],
    )
    .map_err(|error| AppError::Database(format!("保存模型验证结果失败: {error}")))?;
    Ok(())
}

/// Persists a bounded aggregate and its policy-owned merged verdict for one target.
pub fn upsert_passive(db: &Database, batch: &EvidenceBatch) -> Result<MergedReport, AppError> {
    upsert_passive_with_notifications(db, batch).map(|(merged, _)| merged)
}

pub fn upsert_passive_with_notifications(
    db: &Database,
    batch: &EvidenceBatch,
) -> Result<(MergedReport, Vec<AnomalyFingerprint>), AppError> {
    let conn = lock_conn!(db.conn);
    let (existing, notified) = load_passive_state(&conn, &batch.target)?;
    let mut aggregate = existing.unwrap_or_default();
    reduce_batch(&mut aggregate, batch);
    let active = load_active_report(&conn, &batch.target)?;
    let merged = verdict::merge(active.as_ref(), Some(&aggregate));
    let newly_claimed = aggregate
        .unresolved_fingerprints()
        .iter()
        .copied()
        .filter(|fingerprint| verdict::is_high_confidence_anomaly(fingerprint.code()))
        .filter(|fingerprint| !notified.contains(fingerprint))
        .collect::<Vec<_>>();
    let mut notified = notified;
    notified.extend(newly_claimed.iter().copied());
    let passive_aggregate_json = serde_json::to_string(&aggregate)
        .map_err(|error| AppError::Config(format!("序列化被动验证聚合失败: {error}")))?;

    conn.execute(
        "INSERT INTO model_verification_results (
            provider_id, app_type, model, passive_aggregate_json, verdict, evidence_level,
            rules_version, passive_observed_at, updated_at, notified_fingerprints_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(provider_id, app_type, model) DO UPDATE SET
            passive_aggregate_json = excluded.passive_aggregate_json,
            verdict = excluded.verdict,
            evidence_level = excluded.evidence_level,
            rules_version = excluded.rules_version,
            passive_observed_at = excluded.passive_observed_at,
            updated_at = excluded.updated_at,
            notified_fingerprints_json = excluded.notified_fingerprints_json",
        params![
            &batch.target.provider_id,
            &batch.target.app_type,
            &batch.target.model,
            passive_aggregate_json,
            verdict_name(merged.verdict),
            evidence_level_name(merged.evidence_level),
            RULES_VERSION,
            batch.observed_at,
            batch.observed_at,
            serialize_fingerprints(&notified)?,
        ],
    )
    .map_err(|error| AppError::Database(format!("保存被动模型验证结果失败: {error}")))?;
    Ok((merged, newly_claimed))
}

fn load_passive_state(
    conn: &Connection,
    target: &TargetKey,
) -> Result<(Option<PassiveAggregate>, Vec<AnomalyFingerprint>), AppError> {
    let state = conn
        .query_row(
            "SELECT passive_aggregate_json, notified_fingerprints_json
             FROM model_verification_results
             WHERE provider_id = ?1 AND app_type = ?2 AND model = ?3",
            params![&target.provider_id, &target.app_type, &target.model],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| AppError::Database(format!("读取被动模型验证状态失败: {error}")))?;
    state
        .map(|(aggregate_json, notified_json)| {
            let aggregate = aggregate_json
                .map(|value| {
                    serde_json::from_str(&value)
                        .map_err(|error| AppError::Config(format!("解析被动验证聚合失败: {error}")))
                })
                .transpose()?;
            let notified = serde_json::from_str(&notified_json)
                .map_err(|error| AppError::Config(format!("解析验证通知指纹失败: {error}")))?;
            Ok((aggregate, notified))
        })
        .transpose()
        .map(|state| state.unwrap_or((None, Vec::new())))
}

fn load_active_report(
    conn: &Connection,
    target: &TargetKey,
) -> Result<Option<VerificationReport>, AppError> {
    conn.query_row(
        "SELECT active_report_json FROM model_verification_results
         WHERE provider_id = ?1 AND app_type = ?2 AND model = ?3",
        params![&target.provider_id, &target.app_type, &target.model],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .map_err(|error| AppError::Database(format!("读取主动模型验证报告失败: {error}")))?
    .flatten()
    .map(|value| {
        serde_json::from_str(&value)
            .map_err(|error| AppError::Config(format!("解析主动验证报告失败: {error}")))
    })
    .transpose()
}

fn serialize_optional(aggregate: &Option<PassiveAggregate>) -> Result<Option<String>, AppError> {
    aggregate
        .as_ref()
        .map(|aggregate| {
            serde_json::to_string(aggregate)
                .map_err(|error| AppError::Config(format!("序列化被动验证聚合失败: {error}")))
        })
        .transpose()
}

fn serialize_fingerprints(fingerprints: &[AnomalyFingerprint]) -> Result<String, AppError> {
    serde_json::to_string(fingerprints)
        .map_err(|error| AppError::Config(format!("序列化验证通知指纹失败: {error}")))
}

fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Trusted => "trusted",
        Verdict::Suspicious => "suspicious",
        Verdict::Anomaly => "anomaly",
        Verdict::Inconclusive => "inconclusive",
    }
}

fn evidence_level_name(evidence_level: EvidenceLevel) -> &'static str {
    match evidence_level {
        EvidenceLevel::Cryptographic => "cryptographic",
        EvidenceLevel::ProtocolBehavior => "protocolBehavior",
        EvidenceLevel::Insufficient => "insufficient",
    }
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
        "SELECT provider_id, app_type, model, active_report_json, verdict, evidence_level,
                rules_version, COALESCE(active_checked_at, passive_observed_at, updated_at)
         FROM model_verification_results
         WHERE app_type = ? AND provider_id IN ({placeholders})
         ORDER BY provider_id, model"
    );
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Database(format!("查询模型验证结果失败: {error}")))?;
    let rows = statement
        .query_map(
            params_from_iter(
                std::iter::once(app_type).chain(provider_ids.iter().map(String::as_str)),
            ),
            result_row_to_report,
        )
        .map_err(|error| AppError::Database(format!("读取模型验证结果失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证结果失败: {error}")))?;

    rows.into_iter().map(result_row_into_report).collect()
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
        "SELECT provider_id, app_type, model, active_report_json, verdict, evidence_level,
                rules_version, COALESCE(active_checked_at, passive_observed_at, updated_at)
         FROM model_verification_results
         WHERE provider_id IN ({placeholders})
         ORDER BY provider_id, app_type, model"
    );
    let conn = lock_conn!(db.conn);
    let mut statement = conn
        .prepare(&sql)
        .map_err(|error| AppError::Database(format!("查询模型验证结果失败: {error}")))?;
    let rows = statement
        .query_map(params_from_iter(provider_ids.iter()), result_row_to_report)
        .map_err(|error| AppError::Database(format!("读取模型验证结果失败: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AppError::Database(format!("解析模型验证结果失败: {error}")))?;

    rows.into_iter().map(result_row_into_report).collect()
}

type StoredResultRow = (TargetKey, Option<String>, String, String, i32, i64);

fn result_row_to_report(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredResultRow> {
    Ok((
        TargetKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ),
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn result_row_into_report(row: StoredResultRow) -> Result<VerificationReport, AppError> {
    let (target, active_report_json, verdict, evidence_level, rules_version, checked_at) = row;
    let mut report = active_report_json
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|error| AppError::Config(format!("解析主动验证报告失败: {error}")))
        })
        .transpose()?
        .unwrap_or(VerificationReport {
            target: target.clone(),
            verdict: Verdict::Inconclusive,
            evidence_level: EvidenceLevel::Insufficient,
            facts: Vec::new(),
            rules_version,
            checked_at,
        });
    report.target = target;
    report.verdict = parse_verdict(&verdict)?;
    report.evidence_level = parse_evidence_level(&evidence_level)?;
    report.rules_version = rules_version;
    report.checked_at = checked_at;
    Ok(report)
}

fn parse_verdict(value: &str) -> Result<Verdict, AppError> {
    match value {
        "trusted" => Ok(Verdict::Trusted),
        "suspicious" => Ok(Verdict::Suspicious),
        "anomaly" => Ok(Verdict::Anomaly),
        "inconclusive" => Ok(Verdict::Inconclusive),
        _ => Err(AppError::Config("解析验证结论失败".into())),
    }
}

fn parse_evidence_level(value: &str) -> Result<EvidenceLevel, AppError> {
    match value {
        "cryptographic" => Ok(EvidenceLevel::Cryptographic),
        "protocolBehavior" => Ok(EvidenceLevel::ProtocolBehavior),
        "insufficient" => Ok(EvidenceLevel::Insufficient),
        _ => Err(AppError::Config("解析验证证据等级失败".into())),
    }
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
