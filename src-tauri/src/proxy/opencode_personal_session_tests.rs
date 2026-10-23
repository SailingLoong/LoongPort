//! Original Personal Go vendor binding -> RequestContext -> header consumer.
//! Only credentials and upstream replies are synthetic; no public endpoint is used.
use super::*;
use crate::{
    app_config::AppType,
    proxy::opencode_vendor_fixture::{FixtureOwner, FixtureTransport},
    services::ProviderService,
};
use axum::{
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
};
use serde_json::{json, Value};
use std::{sync::Mutex, time::Duration};
use tower::ServiceExt;

struct PersonalFixture {
    owner: FixtureOwner,
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<Mutex<Vec<(HeaderMap, Value, String, Method)>>>,
    upstream: tokio::task::JoinHandle<()>,
    app: AppType,
}
impl Drop for PersonalFixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl PersonalFixture {
    async fn new(app: AppType) -> Self {
        let owner = FixtureOwner::personal_go();
        let state = owner.state();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let capture = calls.clone();
        let upstream_router = axum::Router::new().fallback(axum::routing::post(
            move |method: Method, uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let calls = capture.clone();
                async move {
                    calls.lock().unwrap().push((headers, body, uri.path().to_owned(), method));
                    if uri.path() == "/inference/go/anthropic/v1/messages" {
                        axum::Json(json!({"id":"msg_fixture","type":"message","role":"assistant","model":"deepseek-v4-pro","content":[{"type":"text","text":"synthetic personal answer"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":2}})).into_response()
                    } else if uri.path() == "/inference/go/openai/v1/chat/completions" {
                        axum::Json(json!({"id":"chatcmpl_fixture","object":"chat.completion","model":"glm-5.2","choices":[{"index":0,"message":{"role":"assistant","content":"synthetic personal answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}})).into_response()
                    } else {
                        (StatusCode::NOT_FOUND, "unexpected synthetic endpoint").into_response()
                    }
                }
            }
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = owner.transport(listener.local_addr().unwrap()).unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, upstream_router).await.unwrap();
        });
        let mut config = state.db.get_proxy_config().await.unwrap();
        config.listen_address = "127.0.0.1".into();
        config.listen_port = 0;
        state.db.update_proxy_config(config).await.unwrap();
        let switching = state.clone();
        let selected = owner.ids()[0].clone();
        let switching_app = app.clone();
        tokio::task::spawn_blocking(move || {
            ProviderService::switch(&switching, switching_app, &selected).unwrap()
        })
        .await
        .unwrap();
        state
            .proxy_service
            .set_takeover_for_app(app.as_str(), true)
            .await
            .unwrap();
        let mut config = state
            .db
            .get_proxy_config_for_app(app.as_str())
            .await
            .unwrap();
        config.auto_failover_enabled = false;
        config.non_streaming_timeout = 5;
        state.db.update_proxy_config_for_app(config).await.unwrap();
        let proxy = ProxyServer::new(
            ProxyConfig::default(),
            state.db.clone(),
            None,
            crate::relay::model_verification::passive::PassiveIngress::channel(1).0,
            Arc::new(crate::proxy::model_alignment::ModelAlignmentAlerts::new()),
        );
        Self {
            owner,
            proxy,
            transport,
            calls,
            upstream,
            app,
        }
    }
    async fn request(
        &self,
        session_fields: Value,
        headers: &[(&str, &str)],
        transport: Option<FixtureTransport>,
    ) -> axum::response::Response {
        let mut body = if self.app == AppType::Claude {
            json!({"model":"deepseek-v4-pro","messages":[{"role":"user","content":"synthetic prompt"}],"max_tokens":32,"stream":false})
        } else {
            json!({"model":"glm-5.2","input":"synthetic prompt","stream":false})
        };
        body.as_object_mut()
            .unwrap()
            .extend(session_fields.as_object().unwrap().clone());
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri(if self.app == AppType::Claude {
                "/v1/messages"
            } else {
                "/v1/responses"
            })
            .header("content-type", "application/json")
            .header("authorization", "Bearer synthetic-client-not-upstream");
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        let mut router = self.proxy.build_router();
        if let Some(transport) = transport {
            router = router.layer(axum::Extension(transport));
        }
        tokio::time::timeout(
            Duration::from_secs(8),
            router.oneshot(
                request
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }
    async fn consume(&self, response: axum::response::Response, expected: StatusCode) {
        assert_eq!(response.status(), expected);
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        if expected == StatusCode::OK {
            assert!(String::from_utf8_lossy(&body).contains("synthetic personal answer"));
        }
    }
    async fn stop(&self) {
        self.owner
            .state()
            .proxy_service
            .set_takeover_for_app(self.app.as_str(), false)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn personal_go_original_responses_preserves_real_prompt_cache_session() {
    let f = PersonalFixture::new(AppType::Codex).await;
    for turn in 0..2 {
        let input = if turn == 0 {
            json!("synthetic prompt")
        } else {
            json!([
                {"role":"user","content":"synthetic prompt"},
                {"role":"assistant","content":"synthetic personal answer"},
                {"role":"user","content":"synthetic follow-up"}
            ])
        };
        let response = f
            .request(
                json!({"prompt_cache_key":"ses_fixture_prompt_cache_123456","input":input}),
                &[],
                Some(f.transport.clone()),
            )
            .await;
        f.consume(response, StatusCode::OK).await;
    }
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for (headers, body, path, method) in calls.iter() {
        assert_eq!(method, &Method::POST);
        assert_eq!(path, "/inference/go/openai/v1/chat/completions");
        assert_eq!(body["model"], "glm-5.2");
        assert_eq!(
            headers["x-opencode-session"],
            "ses_fixture_prompt_cache_123456"
        );
        assert_eq!(
            headers["authorization"],
            format!("Bearer {}", f.owner.synthetic_key(0))
        );
        assert!(headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("LoongPort/"));
    }
    let second_messages = calls[1].1["messages"].as_array().unwrap();
    let projected = second_messages
        .iter()
        .map(|message| {
            let content = match &message["content"] {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .map(|part| {
                        assert_eq!(part["type"], "text");
                        part["text"].as_str().unwrap()
                    })
                    .collect::<Vec<_>>()
                    .concat(),
                _ => panic!("original plain-text history must remain plain text"),
            };
            (message["role"].as_str().unwrap().to_owned(), content)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        projected,
        vec![
            ("user".to_owned(), "synthetic prompt".to_owned()),
            (
                "assistant".to_owned(),
                "synthetic personal answer".to_owned()
            ),
            ("user".to_owned(), "synthetic follow-up".to_owned()),
        ],
        "preserve exact ordered history without dropping, duplicating or adding turns"
    );
    drop(calls);
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn personal_go_original_claude_preserves_real_json_user_session() {
    let f = PersonalFixture::new(AppType::Claude).await;
    for (turn, expected) in [
        "ses_fixture_json_123456",
        "ses_fixture_json_123456",
        "ses_fixture_json_other",
    ]
    .into_iter()
    .enumerate()
    {
        let user_id = json!({"device_id":"synthetic-device","session_id":expected}).to_string();
        let messages = if turn == 1 {
            json!([
                {"role":"user","content":"synthetic prompt"},
                {"role":"assistant","content":"synthetic personal answer"},
                {"role":"user","content":"synthetic follow-up"}
            ])
        } else {
            json!([{"role":"user","content":"synthetic prompt"}])
        };
        let response = f
            .request(
                json!({"metadata":{"user_id":user_id},"messages":messages}),
                &[],
                Some(f.transport.clone()),
            )
            .await;
        f.consume(response, StatusCode::OK).await;
        assert_eq!(
            f.calls.lock().unwrap().last().unwrap().0["x-opencode-session"],
            expected
        );
    }
    {
        let calls = f.calls.lock().unwrap();
        for (headers, body, path, method) in calls.iter() {
            assert_eq!(method, &Method::POST);
            assert_eq!(path, "/inference/go/anthropic/v1/messages");
            assert_eq!(body["model"], "deepseek-v4-pro");
            assert_eq!(
                headers["authorization"],
                format!("Bearer {}", f.owner.synthetic_key(0))
            );
            assert!(headers["user-agent"]
                .to_str()
                .unwrap()
                .starts_with("LoongPort/"));
        }
        assert_eq!(
            calls[1].1["messages"],
            json!([
                {"role":"user","content":"synthetic prompt"},
                {"role":"assistant","content":"synthetic personal answer"},
                {"role":"user","content":"synthetic follow-up"}
            ])
        );
    }
    assert_eq!(f.calls.lock().unwrap().len(), 3);
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn personal_go_original_legacy_session_and_native_header_priority_remain() {
    for (app, fields, headers, expected) in [
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"user_fixture_session_legacy-session"}}),
            vec![],
            "legacy-session",
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"{\"session_id\":\"json-lower-priority\"}"}}),
            vec![("x-claude-code-session-id", "explicit-claude-session")],
            "explicit-claude-session",
        ),
        (
            AppType::Codex,
            json!({"prompt_cache_key":"lower-priority"}),
            vec![("x-session-id", "explicit-codex-session-123456")],
            "codex_explicit-codex-session-123456",
        ),
    ] {
        let f = PersonalFixture::new(app).await;
        let response = f.request(fields, &headers, Some(f.transport.clone())).await;
        f.consume(response, StatusCode::OK).await;
        assert_eq!(f.calls.lock().unwrap()[0].0["x-opencode-session"], expected);
        f.stop().await;
    }
}

#[tokio::test]
#[serial_test::serial]
async fn personal_go_invalid_session_sources_never_invent_client_identity_or_send() {
    for (app, fields) in [
        (
            AppType::Codex,
            json!({"prompt_cache_key":"s".repeat(65536)}),
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":json!({"session_id":"s".repeat(65536)}).to_string()}}),
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"{\"session_id\":[]}"}}),
        ),
        (AppType::Codex, json!({"prompt_cache_key":"   "})),
        (AppType::Codex, json!({"prompt_cache_key":42})),
        (
            AppType::Codex,
            json!({"prompt_cache_key":"invalid\r\nsession"}),
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"{\"account_uuid\":\"user_account_session_not_a_conversation\"}"}}),
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"{malformed-json"}}),
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"{\"session_id\":42}"}}),
        ),
    ] {
        let f = PersonalFixture::new(app).await;
        let response = f.request(fields, &[], Some(f.transport.clone())).await;
        assert!(!response.status().is_success());
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(f.calls.lock().unwrap().is_empty());
        f.stop().await;
    }
}

#[tokio::test]
#[serial_test::serial]
async fn personal_go_session_never_authorizes_missing_transport_or_other_account_binding() {
    let f = PersonalFixture::new(AppType::Codex).await;
    let headers = [("x-session-id", "explicit-codex-session-123456")];
    let response = f.request(json!({}), &headers, None).await;
    assert!(!response.status().is_success());
    drop(response);
    let db = &f.owner.state().db;
    let mut provider = db
        .get_all_providers("codex")
        .unwrap()
        .get(&f.owner.ids()[0])
        .unwrap()
        .clone();
    let original = provider.clone();
    provider
        .meta
        .as_mut()
        .unwrap()
        .auth_binding
        .as_mut()
        .unwrap()
        .account_id = Some("702".into());
    db.save_provider("codex", &provider).unwrap();
    let response = f
        .request(json!({}), &headers, Some(f.transport.clone()))
        .await;
    assert!(!response.status().is_success());
    drop(response);
    assert!(f.calls.lock().unwrap().is_empty());
    db.save_provider("codex", &original).unwrap();
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn personal_go_new_body_sources_never_replace_explicit_opencode_identity() {
    for (app, fields) in [
        (
            AppType::Codex,
            json!({"prompt_cache_key":"body-fallback-session"}),
        ),
        (
            AppType::Claude,
            json!({"metadata":{"user_id":"{\"session_id\":\"body-fallback-session\"}"}}),
        ),
    ] {
        let f = PersonalFixture::new(app).await;
        let response = f
            .request(
                fields.clone(),
                &[("x-opencode-session", "explicit-opencode-session")],
                Some(f.transport.clone()),
            )
            .await;
        f.consume(response, StatusCode::OK).await;
        assert_eq!(
            f.calls.lock().unwrap()[0].0["x-opencode-session"],
            "explicit-opencode-session"
        );
        let response = f
            .request(
                fields,
                &[("x-opencode-session", "   ")],
                Some(f.transport.clone()),
            )
            .await;
        assert!(
            !response.status().is_success(),
            "an invalid explicit identity must not be silently replaced by a body fallback"
        );
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(f.calls.lock().unwrap().len(), 1);
        f.stop().await;
    }
}
