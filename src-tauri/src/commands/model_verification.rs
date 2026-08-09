use crate::{
    relay::model_verification::{
        target,
        types::{
            RunFailureKind, RuntimeAppState, RuntimeVerificationSetting, StartRunResponse,
            TargetKey, VerificationReport,
        },
    },
    AppState,
};

#[tauri::command]
pub async fn list_verification_models(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    app_type: String,
) -> Result<Vec<String>, String> {
    target::list_models(&state.db, &provider_id, &app_type)
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVerificationSnapshot {
    pub setting: RuntimeVerificationSetting,
    pub apps: Vec<RuntimeAppState>,
}

#[tauri::command]
pub async fn get_runtime_verification_setting(
    state: tauri::State<'_, AppState>,
) -> Result<RuntimeVerificationSnapshot, String> {
    let (setting, apps) = state
        .model_verification
        .runtime_status(&state.proxy_service)
        .await?;
    Ok(RuntimeVerificationSnapshot { setting, apps })
}

#[tauri::command]
pub async fn set_runtime_verification_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<RuntimeVerificationSnapshot, String> {
    let (setting, apps) = state
        .model_verification
        .set_runtime_enabled(&state.proxy_service, enabled)
        .await?;
    Ok(RuntimeVerificationSnapshot { setting, apps })
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
pub fn get_model_verification_results(
    state: tauri::State<'_, AppState>,
    provider_ids: Vec<String>,
) -> Result<Vec<VerificationReport>, RunFailureKind> {
    get_model_verification_results_impl(&state, provider_ids)
}

fn get_model_verification_results_impl(
    state: &AppState,
    provider_ids: Vec<String>,
) -> Result<Vec<VerificationReport>, RunFailureKind> {
    state.model_verification.list_results(&provider_ids)
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
        cancel_model_verification_impl, get_model_verification_results_impl,
        start_model_verification_impl,
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
        assert!(get_model_verification_results_impl(&state, Vec::new())
            .unwrap()
            .is_empty());
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

        let results =
            get_model_verification_results_impl(&state, vec!["provider-a".into()]).unwrap();

        assert_eq!(
            results,
            vec![VerificationReport {
                target: TargetKey::new("provider-a", "codex", "gpt-a"),
                verdict: Verdict::Trusted,
                evidence_level: EvidenceLevel::ProtocolBehavior,
                facts: vec![fact],
                rules_version: RULES_VERSION,
                checked_at: 1_700_000_000,
            }]
        );
    }
}
