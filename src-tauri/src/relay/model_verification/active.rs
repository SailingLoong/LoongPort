use std::{str::FromStr, sync::Arc};

use crate::{
    app_config::AppType,
    database::Database,
    relay::model_verification::{
        capability_profiles::CapabilityProfile,
        coordinator::{ActiveVerifier, VerificationFuture},
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
    fn prepare(&self, target: TargetKey) -> Result<VerificationFuture, RunFailureKind> {
        let app_type = AppType::from_str(&target.app_type)
            .ok()
            .filter(|app_type| matches!(app_type, AppType::Codex | AppType::Claude))
            .ok_or(RunFailureKind::InvalidResponse)?;
        let resolved = ResolvedTarget::resolve(&self.db, target.clone())
            .map_err(|_| RunFailureKind::InvalidResponse)?;
        let profile = CapabilityProfile::for_target(&app_type, &target.model);
        let client = self.client.clone();

        Ok(Box::pin(async move {
            let facts = match app_type {
                AppType::Codex => {
                    protocols::openai_responses::run_balanced(&client, &resolved, &profile).await
                }
                AppType::Claude => {
                    protocols::anthropic::run_balanced(&client, &resolved, &profile).await
                }
                _ => unreachable!("supported app type checked before preparing the run"),
            }
            .map_err(map_protocol_failure)?;
            let (verdict, evidence_level) = verdict::evaluate(app_type, &profile, &facts);
            Ok(VerificationReport {
                target,
                verdict,
                evidence_level,
                facts,
                rules_version: RULES_VERSION,
                checked_at: chrono::Utc::now().timestamp(),
            })
        }))
    }
}

fn map_protocol_failure(failure: RunFailure) -> RunFailureKind {
    match failure {
        RunFailure::Authentication => RunFailureKind::Authentication,
        RunFailure::RateLimited => RunFailureKind::RateLimited,
        RunFailure::InsufficientBalance => RunFailureKind::InsufficientBalance,
        RunFailure::Upstream => RunFailureKind::Upstream,
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

        let unsupported = verifier.prepare(TargetKey::new(
            "loongport-0123456789abcdef",
            "gemini",
            "gemini-2.5",
        ));
        assert!(matches!(unsupported, Err(RunFailureKind::InvalidResponse)));
        let unmanaged = verifier.prepare(TargetKey::new("manual-provider", "codex", "gpt-5.6-sol"));
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
            .prepare(TargetKey::new(
                "loongport-0123456789abcdef",
                "codex",
                "missing-model",
            ))
            .unwrap()
            .await
            .unwrap_err();

        server.abort();
        assert_eq!(failure, RunFailureKind::ModelUnavailable);
        assert_eq!(paths.lock().unwrap().as_slice(), ["/v1/responses"]);
        let serialized = serde_json::to_string(&failure).unwrap();
        for sentinel in ["URL", "KEY", "PROMPT", "OUTPUT", "SIGNATURE"] {
            assert!(!serialized.contains(sentinel));
        }
    }
}
