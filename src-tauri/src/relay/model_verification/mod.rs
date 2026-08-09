pub mod capability_profiles;
pub mod store;
pub(crate) mod target;
pub mod types;
pub mod verdict;

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
    use crate::{
        app_config::AppType,
        database::Database,
        error::AppError,
        provider::{Provider, ProviderMeta},
    };

    use std::{
        str::FromStr,
        sync::{Arc, Mutex},
    };

    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
        Json, Router,
    };

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

    fn seed_relay(
        db: &Database,
        site_origin: &str,
        api_base_url: &str,
        account_id: Option<i64>,
    ) -> Result<(), AppError> {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Database(error.to_string()))?;
        conn.execute(
            "INSERT INTO loongport_relay (site_origin, site_name, api_base_url, account_id, account_label, login_identifier, auth_token, sort_index) \
             VALUES (?1, 'Example', ?2, ?3, 'example@example.test', 'example@example.test', 'token', 0)",
            rusqlite::params![site_origin, api_base_url, account_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    fn managed_provider(
        provider_id: &str,
        app_type: AppType,
        site_origin: Option<&str>,
        account_id: Option<i64>,
        api_key: Option<&str>,
    ) -> Provider {
        let mut settings_config = serde_json::json!({});
        if let Some(api_key) = api_key {
            settings_config = match app_type {
                AppType::Codex => serde_json::json!({"auth": {"OPENAI_API_KEY": api_key}}),
                AppType::Claude => {
                    serde_json::json!({"env": {"ANTHROPIC_AUTH_TOKEN": api_key}})
                }
                _ => unreachable!("test fixture only supports verification apps"),
            };
        }
        Provider {
            id: provider_id.to_string(),
            name: "Managed tier".to_string(),
            settings_config,
            website_url: site_origin.map(str::to_string),
            category: Some("aggregator".to_string()),
            created_at: None,
            sort_index: None,
            notes: None,
            meta: Some(ProviderMeta {
                loongport_account_id: account_id,
                ..Default::default()
            }),
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn seeded_db_with_relay_and_managed_provider(
        app_type: AppType,
        account_id: i64,
    ) -> Result<Database, AppError> {
        let db = Database::memory()?;
        let site_origin = "https://api.example.test";
        seed_relay(
            &db,
            site_origin,
            "https://api.example.test/v1",
            Some(account_id),
        )?;
        db.save_provider(
            app_type.as_str(),
            &managed_provider(
                "loongport-0123456789abcdef",
                app_type.clone(),
                Some(site_origin),
                Some(account_id),
                Some("sk-secret"),
            ),
        )?;
        Ok(db)
    }

    fn seeded_db_with_endpoint(endpoint: &str, api_key: &str) -> Result<Database, AppError> {
        let db = Database::memory()?;
        seed_relay(&db, endpoint, endpoint, Some(7))?;
        db.save_provider(
            "claude",
            &managed_provider(
                "loongport-0123456789abcdef",
                AppType::Claude,
                Some(endpoint),
                Some(7),
                Some(api_key),
            ),
        )?;
        Ok(db)
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

    #[test]
    fn resolves_exact_account_and_protocol_base() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Claude, 7).unwrap();
        let target = super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-0123456789abcdef", "claude", "claude-sonnet-5"),
        )
        .unwrap();

        assert_eq!(target.api_root(), "https://api.example.test");
        assert_eq!(target.protocol_base(), "https://api.example.test");
        assert_eq!(target.api_key(), "sk-secret");
    }

    #[test]
    fn resolves_managed_codex_provider_for_its_account() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 9).unwrap();
        let target = super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-0123456789abcdef", "codex", "gpt-5.6"),
        )
        .unwrap();

        assert_eq!(target.protocol_base(), "https://api.example.test/v1");
    }

    #[test]
    fn rejects_non_managed_provider() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 7).unwrap();
        db.save_provider(
            "codex",
            &managed_provider(
                "manual-provider",
                AppType::Codex,
                Some("https://api.example.test"),
                Some(7),
                Some("sk-secret"),
            ),
        )
        .unwrap();

        let error = match super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("manual-provider", "codex", "gpt-5.6"),
        ) {
            Ok(_) => panic!("a non-managed provider must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("LoongPort 托管"));
    }

    #[test]
    fn rejects_provider_without_relay_ownership() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 7).unwrap();
        db.save_provider(
            "codex",
            &managed_provider(
                "loongport-fedcba9876543210",
                AppType::Codex,
                Some("https://missing.example.test"),
                Some(7),
                Some("sk-secret"),
            ),
        )
        .unwrap();

        let error = match super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-fedcba9876543210", "codex", "gpt-5.6"),
        ) {
            Ok(_) => panic!("an unowned provider must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("已经不在列表里"));
    }

    #[test]
    fn rejects_provider_owned_by_another_account_at_the_same_site() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 7).unwrap();
        db.save_provider(
            "codex",
            &managed_provider(
                "loongport-fedcba9876543210",
                AppType::Codex,
                Some("https://api.example.test"),
                Some(8),
                Some("sk-secret"),
            ),
        )
        .unwrap();

        let error = match super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-fedcba9876543210", "codex", "gpt-5.6"),
        ) {
            Ok(_) => panic!("a provider owned by another account must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("某个账号"));
    }

    #[test]
    fn resolves_legacy_provider_when_its_site_has_one_relay_account() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 7).unwrap();
        db.save_provider(
            "codex",
            &managed_provider(
                "loongport-fedcba9876543210",
                AppType::Codex,
                Some("https://api.example.test"),
                None,
                Some("sk-secret"),
            ),
        )
        .unwrap();

        let target = super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-fedcba9876543210", "codex", "gpt-5.6"),
        )
        .unwrap();

        assert_eq!(target.api_root(), "https://api.example.test");
    }

    #[test]
    fn rejects_ambiguous_legacy_provider_ownership() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 7).unwrap();
        seed_relay(
            &db,
            "https://api.example.test",
            "https://api.example.test/v1",
            Some(8),
        )
        .unwrap();
        db.save_provider(
            "codex",
            &managed_provider(
                "loongport-fedcba9876543210",
                AppType::Codex,
                Some("https://api.example.test"),
                None,
                Some("sk-secret"),
            ),
        )
        .unwrap();

        let error = match super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-fedcba9876543210", "codex", "gpt-5.6"),
        ) {
            Ok(_) => panic!("an ambiguously owned legacy provider must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("多个账号"));
    }

    #[test]
    fn rejects_provider_without_api_key() {
        let db = seeded_db_with_relay_and_managed_provider(AppType::Codex, 7).unwrap();
        db.save_provider(
            "codex",
            &managed_provider(
                "loongport-fedcba9876543210",
                AppType::Codex,
                Some("https://api.example.test"),
                Some(7),
                None,
            ),
        )
        .unwrap();

        let error = match super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-fedcba9876543210", "codex", "gpt-5.6"),
        ) {
            Ok(_) => panic!("a provider without an API key must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("读不出密钥"));
    }

    #[test]
    fn target_resolution_rejects_unsupported_app_type() {
        let db = Database::memory().unwrap();
        let error = match super::target::ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-0123456789abcdef", "gemini", "gemini-2.5"),
        ) {
            Ok(_) => panic!("an unsupported app type must be rejected"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("仅支持 codex 和 claude"));
        assert_eq!(AppType::from_str("gemini").unwrap(), AppType::Gemini);
    }

    #[tokio::test]
    async fn list_models_sends_bearer_auth_and_returns_sorted_unique_ids() {
        async fn models(
            State(observed_authorization): State<Arc<Mutex<Option<String>>>>,
            headers: HeaderMap,
        ) -> impl IntoResponse {
            *observed_authorization.lock().unwrap() = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            Json(serde_json::json!({
                "object": "list",
                "data": [{"id": "z-model"}, {"id": "a-model"}, {"id": "z-model"}]
            }))
        }

        let observed_authorization = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/v1/models", get(models))
            .with_state(observed_authorization.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let db = seeded_db_with_endpoint(&endpoint, "sk-secret").unwrap();

        let models = super::target::list_models(&db, "loongport-0123456789abcdef", "claude")
            .await
            .unwrap();

        server.abort();
        assert_eq!(models, vec!["a-model", "z-model"]);
        assert_eq!(
            observed_authorization.lock().unwrap().as_deref(),
            Some("Bearer sk-secret")
        );
    }

    #[tokio::test]
    async fn list_models_makes_unauthorized_access_visible_without_leaking_credentials() {
        async fn unauthorized(_: HeaderMap) -> impl IntoResponse {
            (StatusCode::UNAUTHORIZED, "response-body-sentinel")
        }

        let app = Router::new().route("/v1/models", get(unauthorized));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let db = seeded_db_with_endpoint(&endpoint, "sk-secret").unwrap();

        let error =
            match super::target::list_models(&db, "loongport-0123456789abcdef", "claude").await {
                Ok(_) => panic!("an unauthorized model-list request must be visible"),
                Err(error) => error,
            };

        server.abort();
        let message = error.to_string();
        assert!(message.contains("无法读取这个分组的模型列表"));
        assert!(!message.contains("sk-secret"));
        assert!(!message.contains("response-body-sentinel"));
    }

    #[tokio::test]
    async fn list_models_sanitizes_upstream_response_bodies() {
        async fn server_error(_: HeaderMap) -> impl IntoResponse {
            (StatusCode::INTERNAL_SERVER_ERROR, "response-body-sentinel")
        }

        let app = Router::new().route("/v1/models", get(server_error));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let db = seeded_db_with_endpoint(&endpoint, "sk-secret").unwrap();

        let error =
            match super::target::list_models(&db, "loongport-0123456789abcdef", "claude").await {
                Ok(_) => panic!("a server failure must be visible"),
                Err(error) => error,
            };

        server.abort();
        let message = error.to_string();
        assert!(message.contains("无法读取这个分组的模型列表"));
        assert!(!message.contains("sk-secret"));
        assert!(!message.contains("response-body-sentinel"));
    }
}
