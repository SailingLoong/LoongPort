pub mod store;
pub mod types;

#[cfg(test)]
mod tests {
    use super::{
        store::{clear_scope, list_for_providers, upsert_active},
        types::{
            EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, RunFailureKind, RunState,
            TargetKey, TargetScope, Verdict, VerificationReport, VerificationSummary,
            RULES_VERSION,
        },
    };
    use crate::{database::Database, error::AppError};

    fn report(
        provider_id: &str,
        app_type: &str,
        model: &str,
        verdict: Verdict,
    ) -> VerificationReport {
        VerificationReport {
            target: TargetKey::new(provider_id, app_type, model),
            verdict,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts: vec![EvidenceFact {
                code: EvidenceCode::ModelMatch,
                outcome: EvidenceOutcome::Passed,
            }],
            rules_version: RULES_VERSION,
            checked_at: 1_700_000_000,
        }
    }

    fn insert_provider(db: &Database, provider_id: &str, app_type: &str) -> Result<(), AppError> {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Database(error.to_string()))?;
        conn.execute(
            "INSERT INTO providers (id, app_type, name, settings_config) VALUES (?1, ?2, ?3, '{}')",
            rusqlite::params![provider_id, app_type, provider_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    #[test]
    fn evidence_fact_has_no_free_form_payload() {
        let fact = EvidenceFact {
            code: EvidenceCode::ModelMatch,
            outcome: EvidenceOutcome::Passed,
        };
        assert_eq!(
            serde_json::to_value(fact).unwrap(),
            serde_json::json!({"code":"modelMatch","outcome":"passed"})
        );
    }

    #[test]
    fn report_summary_and_run_statuses_serialize_as_finite_camel_case_values() {
        let summary: VerificationSummary =
            report("provider-a", "codex", "gpt-a", Verdict::Trusted).summary();

        assert_eq!(summary.target.model, "gpt-a");
        assert_eq!(
            serde_json::to_value(RunFailureKind::InvalidResponse).unwrap(),
            serde_json::json!("invalidResponse")
        );
        assert_eq!(
            serde_json::to_value(RunState::Completed).unwrap(),
            serde_json::json!("completed")
        );
    }

    #[test]
    fn upsert_replaces_one_model_without_merging_other_models() -> Result<(), AppError> {
        let db = Database::memory()?;
        insert_provider(&db, "provider-a", "codex")?;

        upsert_active(
            &db,
            &report("provider-a", "codex", "gpt-a", Verdict::Anomaly),
        )?;
        upsert_active(
            &db,
            &report("provider-a", "codex", "gpt-b", Verdict::Trusted),
        )?;
        upsert_active(
            &db,
            &report("provider-a", "codex", "gpt-a", Verdict::Suspicious),
        )?;

        let reports = list_for_providers(&db, "codex", &["provider-a".into()])?;
        assert_eq!(reports.len(), 2);
        assert_eq!(
            reports
                .iter()
                .find(|summary| summary.target.model == "gpt-a")
                .unwrap()
                .verdict,
            Verdict::Suspicious
        );
        assert_eq!(
            reports
                .iter()
                .find(|summary| summary.target.model == "gpt-b")
                .unwrap()
                .verdict,
            Verdict::Trusted
        );
        Ok(())
    }

    #[test]
    fn clear_scope_does_not_touch_another_provider() -> Result<(), AppError> {
        let db = Database::memory()?;
        insert_provider(&db, "provider-a", "codex")?;
        insert_provider(&db, "provider-b", "codex")?;
        upsert_active(
            &db,
            &report("provider-a", "codex", "gpt-a", Verdict::Anomaly),
        )?;
        upsert_active(
            &db,
            &report("provider-b", "codex", "gpt-a", Verdict::Suspicious),
        )?;

        clear_scope(&db, &TargetScope::new("provider-a", "codex"))?;

        assert!(list_for_providers(&db, "codex", &["provider-a".into()])?.is_empty());
        assert_eq!(
            list_for_providers(&db, "codex", &["provider-b".into()])?.len(),
            1
        );
        Ok(())
    }

    #[test]
    fn list_for_providers_returns_empty_for_no_provider_ids() -> Result<(), AppError> {
        let db = Database::memory()?;
        assert!(list_for_providers(&db, "codex", &[])?.is_empty());
        Ok(())
    }
}
