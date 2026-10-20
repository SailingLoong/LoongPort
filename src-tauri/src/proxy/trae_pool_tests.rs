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
async fn trae_missing_fixture_is_closed_before_http() {
    let f = Fixture::new().await;
    let response=f.request_with("/v1/chat/completions",json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}]}),None).await;
    assert!(!response.status().is_success());
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
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
