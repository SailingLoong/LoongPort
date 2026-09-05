use crate::{
    relay::model_verification::{
        target,
        types::{
            RunFailureKind, StartRunResponse, TargetKey, TargetScope, VerificationHistoryEntry,
            VerificationReport,
        },
        verdict::report_precedes,
    },
    AppState,
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationScopeSummary {
    pub provider_id: String,
    pub app_type: String,
    pub badge_verdict: Option<crate::relay::model_verification::types::Verdict>,
    pub representative_report: VerificationReport,
}

#[tauri::command]
pub async fn list_verification_models(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    app_type: String,
) -> Result<Vec<crate::relay::model_verification::types::VerificationModelOption>, String> {
    target::list_models(&state.db, &provider_id, &app_type)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_model_verification_diagnostics(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    app_type: String,
    model: String,
) -> Result<Vec<crate::relay::model_verification::types::ProbeDiagnostic>, String> {
    crate::relay::model_verification::store::active_diagnostics(
        &state.db,
        &provider_id,
        &app_type,
        &model,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn start_model_verification(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    app_type: String,
    model: String,
) -> Result<StartRunResponse, RunFailureKind> {
    start_model_verification_impl(&state, provider_id, app_type, model).await
}

async fn start_model_verification_impl(
    state: &AppState,
    provider_id: String,
    app_type: String,
    model: String,
) -> Result<StartRunResponse, RunFailureKind> {
    state
        .model_verification
        .start(TargetKey::new(provider_id, app_type, model))
        .await
}

#[tauri::command]
pub fn cancel_model_verification(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), RunFailureKind> {
    cancel_model_verification_impl(&state, run_id)
}

fn cancel_model_verification_impl(state: &AppState, run_id: String) -> Result<(), RunFailureKind> {
    state.model_verification.cancel(&run_id)
}

#[tauri::command]
pub fn get_model_verification_summaries(
    state: tauri::State<'_, AppState>,
    provider_ids: Vec<String>,
    app_type: String,
) -> Result<Vec<VerificationScopeSummary>, RunFailureKind> {
    get_model_verification_summaries_impl(&state, provider_ids, app_type)
}

fn get_model_verification_summaries_impl(
    state: &AppState,
    provider_ids: Vec<String>,
    app_type: String,
) -> Result<Vec<VerificationScopeSummary>, RunFailureKind> {
    let reports = state
        .model_verification
        .list_results_for_app(&app_type, &provider_ids)?;
    let mut summaries = Vec::<VerificationScopeSummary>::new();
    for report in reports {
        if let Some(summary) = summaries
            .iter_mut()
            .find(|summary| summary.provider_id == report.target.provider_id)
        {
            if report_precedes(&report, &summary.representative_report) {
                summary.badge_verdict = badge_verdict(report.verdict);
                summary.representative_report = report;
            }
        } else {
            summaries.push(VerificationScopeSummary {
                provider_id: report.target.provider_id.clone(),
                app_type: report.target.app_type.clone(),
                badge_verdict: badge_verdict(report.verdict),
                representative_report: report,
            });
        }
    }
    Ok(summaries)
}

const fn badge_verdict(
    verdict: crate::relay::model_verification::types::Verdict,
) -> Option<crate::relay::model_verification::types::Verdict> {
    use crate::relay::model_verification::types::Verdict;
    match verdict {
        Verdict::Trusted | Verdict::Suspicious | Verdict::Anomaly => Some(verdict),
        Verdict::Inconclusive => None,
    }
}

#[tauri::command]
pub fn get_model_verification_history(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    app_type: String,
) -> Result<Vec<VerificationHistoryEntry>, RunFailureKind> {
    state
        .model_verification
        .list_history(&TargetScope::new(provider_id, app_type))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        database::Database,
        relay::model_verification::{
            coordinator::{
                ActiveVerifier, ModelVerificationCoordinator, PreparedVerification, ProbeProgress,
            },
            store::upsert_active,
            types::{
                EvidenceCode, EvidenceFact, EvidenceLevel, EvidenceOutcome, RunFailureKind,
                TargetKey, Verdict, VerificationReport, RULES_VERSION,
            },
        },
        AppState,
    };

    use super::{
        cancel_model_verification_impl, get_model_verification_summaries_impl,
        start_model_verification_impl, VerificationScopeSummary,
    };

    struct RejectingVerifier {
        target: Mutex<Option<TargetKey>>,
    }

    impl ActiveVerifier for RejectingVerifier {
        fn prepare(
            &self,
            target: TargetKey,
            _progress: ProbeProgress,
        ) -> Result<PreparedVerification, RunFailureKind> {
            *self.target.lock().unwrap() = Some(target);
            Err(RunFailureKind::Authentication)
        }
    }

    fn state_with_verifier(verifier: Arc<dyn ActiveVerifier>) -> AppState {
        let db = Arc::new(Database::memory().unwrap());
        let mut state = AppState::new(db.clone());
        state.model_verification =
            Arc::new(ModelVerificationCoordinator::with_verifier(db, verifier));
        state
    }

    #[tokio::test]
    async fn start_boundary_passes_only_identifiers_and_returns_a_finite_failure() {
        let verifier = Arc::new(RejectingVerifier {
            target: Mutex::new(None),
        });
        let state = state_with_verifier(verifier.clone());

        let failure = start_model_verification_impl(
            &state,
            "provider-a".into(),
            "codex".into(),
            "gpt-5.6-sol".into(),
        )
        .await
        .unwrap_err();

        assert_eq!(failure, RunFailureKind::Authentication);
        assert_eq!(
            verifier.target.lock().unwrap().as_ref(),
            Some(&TargetKey::new("provider-a", "codex", "gpt-5.6-sol"))
        );
        let serialized = serde_json::to_string(&failure).unwrap();
        for sentinel in ["URL", "KEY", "PROMPT", "OUTPUT", "THINKING", "SIGNATURE"] {
            assert!(!serialized.contains(sentinel));
        }
    }

    #[tokio::test]
    async fn cancel_unknown_run_and_empty_result_query_are_idempotent() {
        let verifier = Arc::new(RejectingVerifier {
            target: Mutex::new(None),
        });
        let state = state_with_verifier(verifier);

        cancel_model_verification_impl(&state, "unknown-run".into()).unwrap();
        assert!(
            get_model_verification_summaries_impl(&state, Vec::new(), "codex".into())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn result_boundary_preserves_persisted_finite_evidence() {
        let verifier = Arc::new(RejectingVerifier {
            target: Mutex::new(None),
        });
        let state = state_with_verifier(verifier);
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config) VALUES (?1, ?2, ?3, '{}')",
                rusqlite::params!["provider-a", "codex", "provider-a"],
            )
            .unwrap();
        let fact = EvidenceFact {
            code: EvidenceCode::ModelMatch,
            outcome: EvidenceOutcome::Passed,
        };
        upsert_active(
            &state.db,
            &VerificationReport {
                target: TargetKey::new("provider-a", "codex", "gpt-a"),
                verdict: Verdict::Trusted,
                evidence_level: EvidenceLevel::ProtocolBehavior,
                facts: vec![fact.clone()],
                rules_version: RULES_VERSION,
                checked_at: 1_700_000_000,
            },
        )
        .unwrap();

        let results = get_model_verification_summaries_impl(
            &state,
            vec!["provider-a".into()],
            "codex".into(),
        )
        .unwrap();

        assert_eq!(
            results,
            vec![VerificationScopeSummary {
                provider_id: "provider-a".into(),
                app_type: "codex".into(),
                badge_verdict: Some(Verdict::Trusted),
                representative_report: VerificationReport {
                    target: TargetKey::new("provider-a", "codex", "gpt-a"),
                    verdict: Verdict::Trusted,
                    evidence_level: EvidenceLevel::ProtocolBehavior,
                    facts: vec![fact],
                    rules_version: RULES_VERSION,
                    checked_at: 1_700_000_000,
                },
            }]
        );
    }

    #[test]
    fn result_boundary_aggregates_scope_severity_and_uses_the_newest_tie() {
        let verifier = Arc::new(RejectingVerifier {
            target: Mutex::new(None),
        });
        let state = state_with_verifier(verifier);
        state
            .db
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO providers (id, app_type, name, settings_config) VALUES (?1, ?2, ?3, '{}')",
                rusqlite::params!["provider-a", "codex", "provider-a"],
            )
            .unwrap();
        for (model, verdict, checked_at) in [
            ("trusted", Verdict::Trusted, 30),
            ("old-anomaly", Verdict::Anomaly, 10),
            ("new-anomaly", Verdict::Anomaly, 20),
        ] {
            upsert_active(
                &state.db,
                &VerificationReport {
                    target: TargetKey::new("provider-a", "codex", model),
                    verdict,
                    evidence_level: EvidenceLevel::ProtocolBehavior,
                    facts: Vec::new(),
                    rules_version: RULES_VERSION,
                    checked_at,
                },
            )
            .unwrap();
        }

        let summaries = get_model_verification_summaries_impl(
            &state,
            vec!["provider-a".into()],
            "codex".into(),
        )
        .unwrap();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].badge_verdict, Some(Verdict::Anomaly));
        assert_eq!(
            summaries[0].representative_report.target.model,
            "new-anomaly"
        );
    }
}
