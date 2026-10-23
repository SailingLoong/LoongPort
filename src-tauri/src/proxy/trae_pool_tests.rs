//! Actual original router, selected original TRAE owner and downstream Body.
use super::*;
use crate::provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta};
use crate::trae_accounts::{inference::FixtureTransport, store::Realm};
use axum::{http::StatusCode, response::IntoResponse};
use serde_json::{json, Value};
use std::time::Duration;
struct Fixture {
    db: Arc<Database>,
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<std::sync::Mutex<Vec<Value>>>,
    upstream: tokio::task::JoinHandle<()>,
    _serial: tokio::sync::OwnedMutexGuard<()>,
    _home: crate::secrets::testing::TestHome,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        Self::with_denial(false).await
    }
    async fn with_denial(deny: bool) -> Self {
        Self::with_reply(deny, None).await
    }
    async fn with_reply(deny: bool, reply: Option<&'static str>) -> Self {
        Self::with_response(deny, "text/event-stream", reply).await
    }
    async fn with_response(
        deny: bool,
        content_type: &'static str,
        reply: Option<&'static str>,
    ) -> Self {
        let home = crate::secrets::testing::TestHome::new().unwrap();
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        let _ = rustls::crypto::ring::default_provider().install_default();
        let serial = FixtureTransport::serial().await;
        let db = Arc::new(Database::memory().unwrap());
        let owner = FixtureTransport::seed_accounts(
            db.secret_session(),
            &[("synthetic-a", Realm::Sg), ("synthetic-b", Realm::Us)],
        )
        .await
        .unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = calls.clone();
        let app=axum::Router::new().route("/{region}/api/agent/v3/llm_utils_chat", axum::routing::post(move |axum::Json(body):axum::Json<Value>|{
   let captured=captured.clone(); async move {
    captured.lock().unwrap().push(body);
                    if deny { return (StatusCode::FORBIDDEN, axum::Json(json!({"code":11004}))).into_response(); }
                    if let Some(reply) = reply { return (StatusCode::OK, [("content-type", content_type)], reply).into_response(); }
    (StatusCode::OK,[("content-type","text/event-stream")], "event: output\ndata: {\"response\":\"synthetic TRAE answer\"}\n\nevent: done\ndata: {}\n\n").into_response()
   }
  }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let upstream = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let transport = FixtureTransport::loopback(&owner, &origin).unwrap();
        for app in ["codex", "claude"] {
            for (i, account_id) in owner.account_ids().iter().enumerate() {
                let id = format!("trae-{i}");
                let mut p = Provider::with_id(
                    id.clone(),
                    id.clone(),
                    json!({"model":"synthetic-model","env":{"ANTHROPIC_MODEL":"synthetic-model"}}),
                    None,
                );
                p.sort_index = Some(i);
                p.meta = Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: AuthBindingSource::ManagedAccount,
                        auth_provider: Some("trae".into()),
                        account_id: Some(account_id.clone()),
                    }),
                    api_format: Some("anthropic".into()),
                    ..Default::default()
                });
                db.save_provider(app, &p).unwrap();
                db.add_to_failover_queue(app, &id).unwrap();
            }
            db.set_current_provider(app, "trae-0").unwrap();
            let mut config = db.get_proxy_config_for_app(app).await.unwrap();
            config.auto_failover_enabled = true;
            config.max_retries = 2;
            config.streaming_first_byte_timeout = 3;
            config.streaming_idle_timeout = 3;
            config.non_streaming_timeout = 5;
            db.update_proxy_config_for_app(config).await.unwrap();
        }
        let proxy = ProxyServer::new(
            ProxyConfig::default(),
            db.clone(),
            #[cfg(feature = "gui")]
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
        );
        Self {
            _home: home,
            db,
            proxy,
            transport,
            calls,
            upstream,
            _serial: serial,
        }
    }
    async fn request(&self, path: &str, body: Value) -> axum::response::Response {
        self.request_with(path, body, Some(self.transport.clone()))
            .await
    }
    async fn request_with(
        &self,
        path: &str,
        body: Value,
        transport: Option<FixtureTransport>,
    ) -> axum::response::Response {
        use tower::ServiceExt;
        let mut router = self.proxy.build_router();
        if let Some(t) = transport {
            router = router.layer(axum::Extension(t));
        }
        tokio::time::timeout(
            Duration::from_secs(8),
            router.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer untrusted-client")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .expect("original router admission timed out")
        .unwrap()
    }
}
#[tokio::test]
#[serial_test::serial]
async fn trae_original_chat_claude_and_responses_text_consumers() {
    for (path, body) in [
        (
            "/v1/chat/completions",
            json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}]}),
        ),
        (
            "/v1/messages",
            json!({"model":"synthetic-model","stream":true,"max_tokens":32,"messages":[{"role":"user","content":"hello"}]}),
        ),
        (
            "/v1/responses",
            json!({"model":"synthetic-model","stream":true,"input":"hello"}),
        ),
    ] {
        let f = Fixture::new().await;
        let response = f.request(path, body).await;
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        let bytes = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            String::from_utf8_lossy(&bytes).contains("synthetic TRAE answer"),
            "{path}"
        );
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["function"], "chat_v3");
        assert_eq!(calls[0]["model"], "synthetic-model");
    }
}
#[tokio::test]
#[serial_test::serial]
async fn trae_tools_and_explicit_usage_are_excluded_before_account_attempt() {
    let f = Fixture::new().await;
    for extra in [
        json!({"tools":[{"type":"function","function":{"name":"x"}}]}),
        json!({"stream_options":{"include_usage":true}}),
    ] {
        let mut body = json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}]});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response = f.request("/v1/chat/completions", body).await;
        assert!(!response.status().is_success());
        let _ = axum::body::to_bytes(response.into_body(), 1024 * 1024).await;
    }
    assert!(f.calls.lock().unwrap().is_empty());
}
#[tokio::test]
#[serial_test::serial]
async fn trae_missing_fixture_is_closed_before_http() {
    let f = Fixture::new().await;
    let response=f.request_with("/v1/chat/completions",json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}]}),None).await;
    assert!(!response.status().is_success());
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn trae_final_body_rechecks_authorization_after_prime() {
    use futures::StreamExt;
    let f = Fixture::new().await;
    let response = f
        .request(
            "/v1/responses",
            json!({"model":"synthetic-model","stream":true,"input":"hello"}),
        )
        .await;
    assert!(response.status().is_success());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.db.secret_session().set_blocked(true);
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap();
    assert!(
        matches!(first, Some(Err(_))),
        "primed bytes must not escape after owner lock"
    );
    drop(body);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn trae_explicit_refusal_is_terminal_at_all_original_entries() {
    for (path, body) in [
        (
            "/v1/chat/completions",
            json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}]}),
        ),
        (
            "/v1/messages",
            json!({"model":"synthetic-model","stream":true,"max_tokens":32,"messages":[{"role":"user","content":"hello"}]}),
        ),
        (
            "/v1/responses",
            json!({"model":"synthetic-model","stream":true,"input":"hello"}),
        ),
    ] {
        let f = Fixture::with_denial(true).await;
        let response = f.request(path, body).await;
        assert!(!response.status().is_success(), "{path}");
        let bytes = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&bytes).contains("access_denied"),
            "{path}"
        );
        assert_eq!(
            f.calls.lock().unwrap().len(),
            1,
            "denied owner must not trigger another account"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn trae_unknown_success_response_never_tries_second_account() {
    for reply in [
        "",
        "event: output\ndata: invalid-json\n\n",
        "event: output\ndata: {\"response\":\"partial\"}\n\n",
    ] {
        let f = Fixture::with_reply(false, Some(reply)).await;
        let response = f
            .request(
                "/v1/responses",
                json!({"model":"synthetic-model","stream":false,"input":"hello"}),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            f.calls.lock().unwrap().len(),
            1,
            "an accepted but unknown response cannot select the next account"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn trae_non_sse_partial_then_error_never_replays() {
    let f = Fixture::with_response(false, "text/plain", Some(
        "event: output\ndata: {\"response\":\"partial\"}\n\nevent: error\ndata: {\"code\":429}\n\n",
    )).await;
    let response = f
        .request(
            "/v1/responses",
            json!({"model":"synthetic-model","stream":false,"input":"hello"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        f.calls.lock().unwrap().len(),
        1,
        "successful headers and partial work cannot become a replayable HTTP429"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn trae_success_headers_explicit_denial_stays_terminal() {
    for content_type in ["text/event-stream", "text/plain"] {
        let f = Fixture::with_response(
            false,
            content_type,
            Some("event: error\ndata: {\"code\":11004}\n\n"),
        )
        .await;
        let response = f
            .request(
                "/v1/responses",
                json!({"model":"synthetic-model","stream":false,"input":"hello"}),
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(f.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
#[serial_test::serial]
async fn trae_success_headers_business_error_never_tries_second_account() {
    for content_type in ["text/event-stream", "text/plain"] {
        for reply in [
            "event: error\ndata: {\"code\":401}\n\n",
            "event: error\ndata: {\"code\":429}\n\n",
        ] {
            let f = Fixture::with_response(false, content_type, Some(reply)).await;
            let response = f
                .request(
                    "/v1/responses",
                    json!({"model":"synthetic-model","stream":false,"input":"hello"}),
                )
                .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            assert_eq!(f.calls.lock().unwrap().len(), 1);
        }
    }
}

fn automatic_semantic_case(case: &str) -> (&'static str, Value) {
    let schema = json!({"type":"json_schema","name":"synthetic_shape","strict":true,"schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}});
    match case {
        "responses_format" => (
            "/v1/responses",
            json!({"model":"synthetic-model","stream":true,"input":"hello","text":{"format":schema}}),
        ),
        "responses_cursor" => (
            "/v1/responses",
            json!({"model":"synthetic-model","stream":true,"input":"hello","previous_response_id":"synthetic-vendor-bound-response"}),
        ),
        "chat_format" => (
            "/v1/chat/completions",
            json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}],"response_format":{"type":"json_schema","json_schema":{"name":"synthetic_shape","strict":true,"schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}}}}),
        ),
        _ => panic!("unknown synthetic semantic case"),
    }
}

async fn assert_automatic_semantics_excluded(case: &str) {
    let f = Fixture::new().await;
    // Both original accounts are in the applied automatic chain, including the
    // current first entry. No new scheduler or substitute admission harness.
    assert!(
        f.db.get_proxy_config_for_app("codex")
            .await
            .unwrap()
            .auto_failover_enabled
    );
    let (path, body) = automatic_semantic_case(case);
    let response = f.request(path, body).await;
    let status = response.status();
    let _body = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        f.calls.lock().unwrap().len(),
        0,
        "{case}: incompatible automatic candidate must not receive a request"
    );
    assert_eq!(
        f.transport.attempts(),
        0,
        "excluded candidate must not enter owner transport"
    );
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{case}: no compatible candidate must be explicit, not lossy success"
    );
    assert!(
        f.db.provider_attempt_error_rates("codex")
            .unwrap()
            .is_empty(),
        "capability exclusion is not an account attempt"
    );
    for id in ["trae-0", "trae-1"] {
        let health = f.db.get_provider_health(id, "codex").await.unwrap();
        assert!(health.is_healthy);
        assert_eq!(health.consecutive_failures, 0);
        assert!(health.last_success_at.is_none());
        assert!(health.last_failure_at.is_none());
        assert!(health.last_error.is_none());
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.proxy.get_status().await.active_connections != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("excluded original request lease must release");
    // Reuse the same original owners immediately after exclusion: no account
    // capacity or response permit may be stranded by capability admission.
    let response = f
        .request(
            "/v1/responses",
            json!({"model":"synthetic-model","stream":true,"input":"hello"}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("synthetic TRAE answer"));
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert_eq!(f.transport.attempts(), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn trae_automatic_responses_format_excludes_every_candidate_before_send() {
    assert_automatic_semantics_excluded("responses_format").await;
}

#[tokio::test]
#[serial_test::serial]
async fn trae_automatic_responses_cursor_excludes_every_candidate_before_send() {
    assert_automatic_semantics_excluded("responses_cursor").await;
}

#[tokio::test]
#[serial_test::serial]
async fn trae_automatic_chat_format_excludes_every_candidate_before_send() {
    assert_automatic_semantics_excluded("chat_format").await;
}

#[tokio::test]
#[serial_test::serial]
async fn trae_manual_semantic_compatibility_remains_outside_automatic_gate() {
    for case in ["responses_format", "responses_cursor", "chat_format"] {
        let f = Fixture::new().await;
        for app in ["codex", "claude"] {
            let mut config = f.db.get_proxy_config_for_app(app).await.unwrap();
            config.auto_failover_enabled = false;
            f.db.update_proxy_config_for_app(config).await.unwrap();
        }
        let (path, body) = automatic_semantic_case(case);
        let response = f.request(path, body).await;
        let expected_status = if case != "responses_cursor" {
            // Structured-format conversion now preserves Responses text.format
            // as well as native Chat response_format. The original strict TRAE
            // wire must reject both, not silently return an unconstrained answer.
            // This intentionally corrects the old Responses false-success while
            // keeping manual selection separate from automatic eligibility.
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::OK
        };
        assert_eq!(response.status(), expected_status, "{case}");
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        if case != "responses_cursor" {
            assert!(!String::from_utf8_lossy(&body).contains("synthetic TRAE answer"));
            assert_eq!(f.calls.lock().unwrap().len(), 0, "{case}");
        } else {
            assert!(String::from_utf8_lossy(&body).contains("synthetic TRAE answer"));
            assert_eq!(f.calls.lock().unwrap().len(), 1, "{case}");
        }
    }
    // The cursor's legacy manual behavior remains out of the automatic gate;
    // its successful text answer is NOT proof of state portability. Structured
    // formats instead explicitly fail now that conversion preserves their intent.
}

#[tokio::test]
#[serial_test::serial]
async fn trae_automatic_eligibility_uses_the_original_selection_mode_snapshot() {
    use crate::{app_config::AppType, proxy::handler_context::RequestContext};
    for selected_automatic in [true, false] {
        let f = Fixture::new().await;
        let mut config = f.db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = selected_automatic;
        f.db.update_proxy_config_for_app(config.clone())
            .await
            .unwrap();
        let (_, body) = automatic_semantic_case("responses_cursor");
        let headers = http::HeaderMap::new();
        let context = RequestContext::new(
            &f.proxy.state,
            &body,
            &headers,
            AppType::Codex,
            "Codex",
            "codex",
        )
        .await
        .unwrap();
        assert_eq!(
            context.get_providers().len(),
            if selected_automatic { 2 } else { 1 }
        );
        // Deterministically change the setting after original selection but
        // before creating/using its forwarder. Never infer mode from chain size.
        config.auto_failover_enabled = !selected_automatic;
        f.db.update_proxy_config_for_app(config).await.unwrap();
        let mut extensions = http::Extensions::new();
        extensions.insert(f.transport.clone());
        let forwarder = context.create_forwarder(&f.proxy.state);
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            forwarder.forward_with_retry(
                &AppType::Codex,
                http::Method::POST,
                "/responses",
                body,
                headers,
                extensions,
                context.get_providers(),
            ),
        )
        .await
        .expect("snapshot dispatch must complete within the original request budget");
        if selected_automatic {
            assert!(matches!(
                result,
                Err(crate::proxy::forwarder::ForwardError {
                    error: ProxyError::InvalidRequest(_),
                    ..
                })
            ));
            assert_eq!(f.calls.lock().unwrap().len(), 0);
            assert_eq!(f.transport.attempts(), 0);
            assert!(f
                .db
                .provider_attempt_error_rates("codex")
                .unwrap()
                .is_empty());
        } else {
            let result = result
                .unwrap_or_else(|_| panic!("later toggle must not reinterpret the manual request"));
            assert_eq!(result.response.status(), StatusCode::OK);
            let bytes = tokio::time::timeout(
                Duration::from_secs(5),
                result.response.bytes_with_limit(1024 * 1024),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("synthetic TRAE answer"));
            assert_eq!(f.calls.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn trae_explicit_unconstrained_formats_respect_original_wire_limits() {
    for automatic in [true, false] {
        for (path, body) in [
            (
                "/v1/responses",
                json!({"model":"synthetic-model","stream":true,"input":"hello","text":{"format":{"type":"text"}}}),
            ),
            (
                "/v1/responses",
                json!({"model":"synthetic-model","stream":true,"input":"hello","response_format":null}),
            ),
            (
                "/v1/chat/completions",
                json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}],"response_format":{"type":"text"}}),
            ),
            (
                "/v1/chat/completions",
                json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}],"response_format":null}),
            ),
        ] {
            let f = Fixture::new().await;
            let mut config = f.db.get_proxy_config_for_app("codex").await.unwrap();
            config.auto_failover_enabled = automatic;
            f.db.update_proxy_config_for_app(config).await.unwrap();
            let response = f.request(path, body).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                axum::body::to_bytes(response.into_body(), 1024 * 1024),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(f.calls.lock().unwrap().len(), 0);
            assert_eq!(f.transport.attempts(), 0);
            if automatic {
                assert!(f
                    .db
                    .provider_attempt_error_rates("codex")
                    .unwrap()
                    .is_empty());
            }
        }
    }
    // These values are not structured constraints. They are excluded because
    // TRAE's original strict wire accepts no response_format field at all.
    // Unadorned plain text remains covered by the original three-entry control.
}
