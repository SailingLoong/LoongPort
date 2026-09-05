use std::{str::FromStr, sync::Arc};

use crate::{
    app_config::AppType,
    database::Database,
    relay::model_verification::{
        capability_profiles::CapabilityProfile,
        coordinator::{ActiveVerifier, PreparedVerification, ProbeProgress},
        protocols::{self, RunFailure},
        target::ResolvedTarget,
        types::{RunFailureKind, TargetKey, VerificationReport, RULES_VERSION},
        verdict,
    },
};

pub struct BalancedActiveVerifier {
    db: Arc<Database>,
    client: reqwest::Client,
}

impl BalancedActiveVerifier {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            client: reqwest::Client::new(),
        }
    }
}

impl ActiveVerifier for BalancedActiveVerifier {
    fn prepare(
        &self,
        target: TargetKey,
        progress: ProbeProgress,
    ) -> Result<PreparedVerification, RunFailureKind> {
        let app_type = AppType::from_str(&target.app_type)
            .ok()
            .filter(|app_type| matches!(app_type, AppType::Codex | AppType::Claude))
            .ok_or(RunFailureKind::InvalidResponse)?;
        let resolved = ResolvedTarget::resolve(&self.db, target.clone())
            .map_err(|_| RunFailureKind::InvalidResponse)?;
        let profile = CapabilityProfile::for_target(&app_type, &target.model);
        let total_checks = profile.active_probe_count();
        let client = self.client.clone();

        let future = Box::pin(async move {
            let mut completed_checks = 0_u8;
            let mut probe_completed = || {
                completed_checks = completed_checks.saturating_add(1);
                progress(completed_checks);
            };
            let (facts, diagnostics) = match app_type {
                AppType::Codex => {
                    protocols::openai_responses::run_balanced_with_progress(
                        &client,
                        &resolved,
                        &profile,
                        &mut probe_completed,
                    )
                    .await
                }
                AppType::Claude => {
                    protocols::anthropic::run_balanced_with_progress(
                        &client,
                        &resolved,
                        &profile,
                        &mut probe_completed,
                    )
                    .await
                }
                _ => unreachable!("supported app type checked before preparing the run"),
            }
            .map_err(|failure| map_protocol_failure(&target, &app_type, failure))?;
            // 多腿探针会产出同名证据事实（如 core/stream 各一条 ModelMatch），
            // 进判定前先按码合并，报告里每个证据码只有一条。诊断按码过滤，
            // 只保留最终仍呈 Failed 的事实对应的腿。
            let facts = verdict::dedupe_facts(facts);
            let diagnostics: Vec<_> = diagnostics
                .into_iter()
                .filter(|diagnostic| {
                    facts.iter().any(|fact| {
                        fact.code == diagnostic.code
                            && fact.outcome
                                == crate::relay::model_verification::types::EvidenceOutcome::Failed
                    })
                })
                .collect();
            let (verdict, evidence_level) = verdict::evaluate(app_type, &profile, &facts);
            Ok((
                VerificationReport {
                    target,
                    verdict,
                    evidence_level,
                    facts,
                    rules_version: RULES_VERSION,
                    checked_at: chrono::Utc::now().timestamp(),
                },
                diagnostics,
            ))
        });
        Ok(PreparedVerification {
            total_checks,
            future,
        })
    }
}

fn map_protocol_failure(
    target: &TargetKey,
    app_type: &AppType,
    failure: RunFailure,
) -> RunFailureKind {
    match failure {
        RunFailure::Authentication => RunFailureKind::Authentication,
        RunFailure::RateLimited => RunFailureKind::RateLimited,
        RunFailure::InsufficientBalance => RunFailureKind::InsufficientBalance,
        RunFailure::Upstream { status } => {
            log::warn!(
                "[model-verification] upstream failure: provider_id={:?} app_type={} model={:?} status={status}",
                target.provider_id,
                app_type.as_str(),
                target.model,
            );
            RunFailureKind::Upstream
        }
        RunFailure::Network => RunFailureKind::Network,
        RunFailure::Timeout => RunFailureKind::Timeout,
        RunFailure::ModelUnavailable => RunFailureKind::ModelUnavailable,
        RunFailure::InvalidResponse | RunFailure::ResponseTooLarge => {
            RunFailureKind::InvalidResponse
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        extract::OriginalUri, http::StatusCode, response::IntoResponse, routing::post, Router,
    };
    use serde_json::json;

    use crate::{
        database::Database,
        provider::{Provider, ProviderMeta},
        relay::model_verification::{
            coordinator::ActiveVerifier,
            types::{RunFailureKind, TargetKey},
        },
    };

    use super::BalancedActiveVerifier;

    fn managed_codex_db(endpoint: &str, api_key: &str) -> Arc<Database> {
        let db = Database::memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO loongport_relay (site_origin, site_name, api_base_url, account_id, account_label, login_identifier, auth_token, sort_index) VALUES (?1, 'Test', ?1, 7, 'test', 'test', 'token', 0)",
                [endpoint],
            )
            .unwrap();
        }
        db.save_provider(
            "codex",
            &Provider {
                id: "loongport-0123456789abcdef".into(),
                name: "Test tier".into(),
                settings_config: json!({"auth": {"OPENAI_API_KEY": api_key}}),
                website_url: Some(endpoint.into()),
                category: Some("aggregator".into()),
                created_at: None,
                sort_index: None,
                notes: None,
                meta: Some(ProviderMeta {
                    loongport_account_id: Some(7),
                    ..Default::default()
                }),
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .unwrap();
        Arc::new(db)
    }

    #[test]
    fn unsupported_and_unmanaged_targets_collapse_to_finite_failures() {
        let db = Arc::new(Database::memory().unwrap());
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config) VALUES ('manual-provider', 'codex', 'Manual', '{}')",
                [],
            )
            .unwrap();
        }
        let verifier = BalancedActiveVerifier::new(db);

        let unsupported = verifier.prepare(
            TargetKey::new("loongport-0123456789abcdef", "gemini", "gemini-2.5"),
            Arc::new(|_| {}),
        );
        assert!(matches!(unsupported, Err(RunFailureKind::InvalidResponse)));
        let unmanaged = verifier.prepare(
            TargetKey::new("manual-provider", "codex", "gpt-5.6-sol"),
            Arc::new(|_| {}),
        );
        assert!(matches!(unmanaged, Err(RunFailureKind::InvalidResponse)));
    }

    #[tokio::test]
    async fn codex_target_selects_responses_and_maps_missing_model_without_leaks() {
        async fn unavailable(
            axum::extract::State(paths): axum::extract::State<Arc<Mutex<Vec<String>>>>,
            OriginalUri(uri): OriginalUri,
        ) -> impl IntoResponse {
            paths.lock().unwrap().push(uri.path().to_string());
            (
                StatusCode::NOT_FOUND,
                "SENTINEL_URL SENTINEL_KEY SENTINEL_PROMPT SENTINEL_OUTPUT SENTINEL_SIGNATURE",
            )
        }

        let paths = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/responses", post(unavailable))
            .route("/v1/messages", post(unavailable))
            .with_state(paths.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let verifier = BalancedActiveVerifier::new(managed_codex_db(&endpoint, "SENTINEL_KEY"));

        let failure = verifier
            .prepare(
                TargetKey::new("loongport-0123456789abcdef", "codex", "missing-model"),
                Arc::new(|_| {}),
            )
            .unwrap()
            .future
            .await
            .unwrap_err();

        server.abort();
        assert_eq!(failure, RunFailureKind::ModelUnavailable);
        assert_eq!(
            paths.lock().unwrap().as_slice(),
            ["/v1/responses", "/v1/responses", "/v1/responses"]
        );
        let serialized = serde_json::to_string(&failure).unwrap();
        for sentinel in ["URL", "KEY", "PROMPT", "OUTPUT", "SIGNATURE"] {
            assert!(!serialized.contains(sentinel));
        }
    }
}
