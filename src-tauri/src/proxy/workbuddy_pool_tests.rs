//! Public ProxyServer pause matrix. No WorkBuddy request is permitted now.
//! Historical wire/refusal/stream assertions remain in inference_tests as private
//! protocol regression; these tests no longer claim to reach a paused upstream.
use super::*;
use crate::provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta};
use crate::workbuddy::inference::FixtureTransport;
use axum::http::StatusCode;
use bytes::Bytes;
use serde_json::{json, Value};
use std::time::Duration;
struct Fixture {
    db: Arc<Database>,
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    origin: String,
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
        static SERIAL: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> =
            std::sync::OnceLock::new();
        let serial = SERIAL
            .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
            .lock_owned()
            .await;
        let home = crate::secrets::testing::TestHome::new().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let accounts =
            FixtureTransport::seed_accounts(db.secret_session(), &["synthetic-a", "synthetic-b"])
                .await
                .unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits = calls.clone();
        let app = axum::Router::new().fallback(move || {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { StatusCode::BAD_REQUEST }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let upstream = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let transport = FixtureTransport::loopback(db.secret_session(), &origin).unwrap();
        for app in ["codex", "claude"] {
            for (position, account_id) in accounts.iter().enumerate() {
                let id = format!("workbuddy-{}", if position == 0 { "a" } else { "b" });
                let mut provider = Provider::with_id(
                    id.clone(),
                    id.clone(),
                    json!({"model":"synthetic-wb-model","env":{"ANTHROPIC_BASE_URL":"https://ignored.invalid","ANTHROPIC_AUTH_TOKEN":"ignored-editable-key"}}),
                    None,
                );
                provider.sort_index = Some(position);
                provider.meta = Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: AuthBindingSource::ManagedAccount,
                        auth_provider: Some("workbuddy".into()),
                        account_id: Some(account_id.clone()),
                    }),
                    // Deliberately wrong editable metadata: the acquired owner must
                    // force Chat protocol and its own public fixed origin.
                    api_format: Some("anthropic".into()),
                    ..Default::default()
                });
                db.save_provider(app, &provider).unwrap();
                db.add_to_failover_queue(app, &id).unwrap();
            }
            db.set_current_provider(app, "workbuddy-a").unwrap();
            let mut config = db.get_proxy_config_for_app(app).await.unwrap();
            config.auto_failover_enabled = true;
            config.max_retries = 2;
            config.non_streaming_timeout = 5;
            config.streaming_first_byte_timeout = 3;
            config.streaming_idle_timeout = 3;
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
            origin,
            upstream,
            _serial: serial,
            _home: home,
        }
    }
    async fn assert_paused(&self, response: axum::response::Response) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = collect(response).await;
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("workbuddy.inference_paused"), "{text}");
        assert!(!text.contains("fixture-access"));
        assert!(!text.contains("message_stop") && !text.contains("response.completed"));
        assert_eq!(self.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(self.transport.attempts(), 0);
        self.idle().await;
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
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.proxy.get_status().await.active_connections != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("final connection guard not released");
    }
}
fn chat(stream: bool) -> Value {
    json!({"model":"synthetic-wb-model","stream":stream,"messages":[{"role":"system","content":"Codex Claude Code preserved instruction"},{"role":"user","content":"synthetic prompt"}]})
}
async fn collect(response: axum::response::Response) -> Bytes {
    tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap()
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_all_account_auto_pool_is_paused_without_retry() {
    let f = Fixture::new().await;
    f.assert_paused(f.request("/v1/chat/completions", chat(true)).await)
        .await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_manual_selection_never_switches_account() {
    let f = Fixture::new().await;
    let mut c = f.db.get_proxy_config_for_app("codex").await.unwrap();
    c.auto_failover_enabled = false;
    f.db.update_proxy_config_for_app(c).await.unwrap();
    f.assert_paused(f.request("/v1/chat/completions", chat(true)).await)
        .await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_nonstream_is_paused_without_post() {
    let f = Fixture::new().await;
    f.assert_paused(f.request("/v1/chat/completions", chat(false)).await)
        .await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_paused_response_contains_no_buffered_upstream_bytes() {
    let f = Fixture::new().await;
    let r = f.request("/v1/chat/completions", chat(true)).await;
    f.db.secret_session().set_blocked(true);
    f.assert_paused(r).await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_repeated_requests_never_open_stream_or_change_accounts() {
    let f = Fixture::new().await;
    for _ in 0..2 {
        f.assert_paused(f.request("/v1/chat/completions", chat(true)).await)
            .await;
    }
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_claude_tools_input_is_paused_before_conversion_dispatch() {
    let f = Fixture::new().await;
    let body = json!({"model":"synthetic-wb-model","stream":true,"max_tokens":64,"system":"preserved system","tools":[{"name":"lookup","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"synthetic"}]});
    f.assert_paused(f.request("/v1/messages", body).await).await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_missing_or_foreign_fixture_has_zero_http() {
    let f = Fixture::new().await;
    f.assert_paused(
        f.request_with("/v1/chat/completions", chat(true), None)
            .await,
    )
    .await;
    let foreign = Database::memory().unwrap();
    FixtureTransport::seed_accounts(foreign.secret_session(), &["synthetic-foreign"])
        .await
        .unwrap();
    let wrong = FixtureTransport::loopback(foreign.secret_session(), &f.origin).unwrap();
    f.assert_paused(
        f.request_with("/v1/chat/completions", chat(true), Some(wrong.clone()))
            .await,
    )
    .await;
    assert_eq!(wrong.attempts(), 0);
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_codex_responses_tools_input_is_paused_before_dispatch() {
    let f = Fixture::new().await;
    let body = json!({"model":"synthetic-wb-model","stream":true,"instructions":"preserved Codex instruction","tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],"input":"synthetic prompt"});
    f.assert_paused(f.request("/v1/responses", body).await)
        .await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_empty_input_is_never_model_permission() {
    let f = Fixture::new().await;
    f.assert_paused(
        f.request(
            "/v1/chat/completions",
            json!({"model":"synthetic-wb-model","stream":true,"messages":[]}),
        )
        .await,
    )
    .await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_all_three_public_entries_pause_stream_and_nonstream() {
    let f = Fixture::new().await;
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        for stream in [true, false] {
            let body = if path.ends_with("responses") {
                json!({"model":"synthetic-wb-model","stream":stream,"input":"synthetic"})
            } else {
                json!({"model":"synthetic-wb-model","stream":stream,"max_tokens":32,"messages":[{"role":"user","content":"synthetic"}]})
            };
            f.assert_paused(f.request(path, body).await).await;
        }
    }
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_pause_does_not_rewrite_provider_or_priority() {
    let f = Fixture::new().await;
    let before = serde_json::to_value(f.db.get_all_providers("codex").unwrap()).unwrap();
    let queue = serde_json::to_value(f.db.get_failover_queue("codex").unwrap()).unwrap();
    f.assert_paused(f.request("/v1/chat/completions", chat(true)).await)
        .await;
    assert_eq!(
        serde_json::to_value(f.db.get_all_providers("codex").unwrap()).unwrap(),
        before
    );
    assert_eq!(
        serde_json::to_value(f.db.get_failover_queue("codex").unwrap()).unwrap(),
        queue
    );
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_free_only_stays_terminal_without_any_model_request() {
    let f = Fixture::new().await;
    let mut body = chat(true);
    body["model"] = json!("openrouter/free");
    let r = f.request("/v1/chat/completions", body).await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&collect(r).await).contains("Free-only route unavailable"));
    assert_eq!(f.transport.attempts(), 0);
    assert_eq!(f.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_replaced_grant_cannot_resurrect_paused_route() {
    let f = Fixture::new().await;
    FixtureTransport::mutate_fixture_grant(f.db.secret_session(), "synthetic-a", "replace")
        .await
        .unwrap();
    f.assert_paused(f.request("/v1/chat/completions", chat(true)).await)
        .await;
}
#[cfg_attr(test, tokio::test)]
#[cfg_attr(test, serial_test::serial)]
async fn workbuddy_retired_first_owner_does_not_enable_second_paused_owner() {
    let f = Fixture::new().await;
    FixtureTransport::retire_fixture_grant(f.db.secret_session(), "synthetic-a")
        .await
        .unwrap();
    f.assert_paused(f.request("/v1/chat/completions", chat(true)).await)
        .await;
}

#[cfg_attr(test, test)]
fn workbuddy_managed_protocol_is_mutually_exclusive_despite_editable_metadata() {
    use crate::proxy::providers::{
        codex_provider_uses_anthropic, codex_provider_uses_chat_completions, get_claude_api_format,
    };
    // Downstream client names/ordinary BYOK endpoints are not upstream accounts.
    for name in ["WorkBuddy", "CodeBuddy"] {
        let p = Provider::with_id(
            "synthetic-downstream".into(),
            name.into(),
            json!({"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:9","ANTHROPIC_AUTH_TOKEN":"synthetic-byok"}}),
            None,
        );
        assert!(!crate::proxy::managed_accounts::uses_workbuddy_model_route(
            &p
        ));
    }
    for kind in ["qoder_sdk", "qoder_cn", "qoder_cn_device"] {
        let mut p = Provider::with_id(
            "synthetic-qoder".into(),
            "Original Qoder".into(),
            json!({"workbuddy_acp":{}}),
            None,
        );
        p.meta = Some(ProviderMeta {
            provider_type: Some(kind.into()),
            ..Default::default()
        });
        assert!(crate::proxy::qoder_http::enabled(&p));
        assert!(!crate::proxy::managed_accounts::uses_workbuddy_model_route(
            &p
        ));
    }
    for (auth_provider, expected) in [
        ("workbuddy", "openai_chat"),
        ("minimax_code", "anthropic"),
        ("zcode_start_plan", "anthropic"),
    ] {
        for editable in [
            "anthropic",
            "openai_chat",
            "openai_responses",
            "gemini_native",
        ] {
            let mut provider = Provider::with_id(
                "synthetic-route".into(),
                "Synthetic".into(),
                json!({"api_format":editable}),
                None,
            );
            provider.meta = Some(ProviderMeta {
                auth_binding: Some(AuthBinding {
                    source: AuthBindingSource::ManagedAccount,
                    auth_provider: Some(auth_provider.into()),
                    account_id: Some("opaque-original-account".into()),
                }),
                api_format: Some(editable.into()),
                ..Default::default()
            });
            assert_eq!(
                codex_provider_uses_chat_completions(&provider),
                expected == "openai_chat",
                "{auth_provider}/{editable}"
            );
            assert_eq!(
                codex_provider_uses_anthropic(&provider),
                expected == "anthropic",
                "{auth_provider}/{editable}"
            );
            assert_eq!(
                get_claude_api_format(&provider),
                expected,
                "{auth_provider}/{editable}"
            );
        }
    }
}

/// Same original public consumer cases, exposed only to the synthetic test target.
#[cfg(all(feature = "gui", feature = "test-hooks", not(test)))]
pub(crate) async fn verify(case: &str) {
    match case {
        "workbuddy_all_account_auto_pool_is_paused_without_retry" => {
            workbuddy_all_account_auto_pool_is_paused_without_retry().await
        }
        "workbuddy_manual_selection_never_switches_account" => {
            workbuddy_manual_selection_never_switches_account().await
        }
        "workbuddy_nonstream_is_paused_without_post" => {
            workbuddy_nonstream_is_paused_without_post().await
        }
        "workbuddy_paused_response_contains_no_buffered_upstream_bytes" => {
            workbuddy_paused_response_contains_no_buffered_upstream_bytes().await
        }
        "workbuddy_repeated_requests_never_open_stream_or_change_accounts" => {
            workbuddy_repeated_requests_never_open_stream_or_change_accounts().await
        }
        "workbuddy_claude_tools_input_is_paused_before_conversion_dispatch" => {
            workbuddy_claude_tools_input_is_paused_before_conversion_dispatch().await
        }
        "workbuddy_missing_or_foreign_fixture_has_zero_http" => {
            workbuddy_missing_or_foreign_fixture_has_zero_http().await
        }
        "workbuddy_codex_responses_tools_input_is_paused_before_dispatch" => {
            workbuddy_codex_responses_tools_input_is_paused_before_dispatch().await
        }
        "workbuddy_empty_input_is_never_model_permission" => {
            workbuddy_empty_input_is_never_model_permission().await
        }
        "workbuddy_all_three_public_entries_pause_stream_and_nonstream" => {
            workbuddy_all_three_public_entries_pause_stream_and_nonstream().await
        }
        "workbuddy_pause_does_not_rewrite_provider_or_priority" => {
            workbuddy_pause_does_not_rewrite_provider_or_priority().await
        }
        "workbuddy_free_only_stays_terminal_without_any_model_request" => {
            workbuddy_free_only_stays_terminal_without_any_model_request().await
        }
        "workbuddy_replaced_grant_cannot_resurrect_paused_route" => {
            workbuddy_replaced_grant_cannot_resurrect_paused_route().await
        }
        "workbuddy_retired_first_owner_does_not_enable_second_paused_owner" => {
            workbuddy_retired_first_owner_does_not_enable_second_paused_owner().await
        }
        "protocol" => workbuddy_managed_protocol_is_mutually_exclusive_despite_editable_metadata(),
        _ => panic!("unknown public pause case"),
    }
}
