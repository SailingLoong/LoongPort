//! Original ProxyServer -> selected pool -> WorkBuddy owner -> final Body tests.
//! Only the upstream is synthetic. No replacement router or public-network fallback.
use super::*;
use crate::provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta};
use crate::workbuddy::inference::FixtureTransport;
use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

const TEXT: &str = "synthetic WorkBuddy answer";
const FIRST:&str="data: {\"id\":\"chat-synthetic\",\"object\":\"chat.completion.chunk\",\"model\":\"synthetic-wb-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"synthetic WorkBuddy answer\"},\"finish_reason\":null}]}\n\n";
const DONE:&str="data: {\"id\":\"chat-synthetic\",\"object\":\"chat.completion.chunk\",\"model\":\"synthetic-wb-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\ndata: [DONE]\n\n";
#[derive(Clone, Copy)]
enum Behavior {
    Complete,
    FirstRateLimited,
    Refused(u16, i64),
    StreamRefusal,
    StreamFailure,
    UnicodeFirst,
    StructuredStreamDenial(i64, bool),
}
#[derive(Clone)]
struct Captured {
    headers: HeaderMap,
    body: Value,
}
struct Fixture {
    db: Arc<Database>,
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<std::sync::Mutex<Vec<Captured>>>,
    release: Arc<tokio::sync::Notify>,
    origin: String,
    upstream: tokio::task::JoinHandle<()>,
    _serial: tokio::sync::OwnedMutexGuard<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl Fixture {
    async fn new(behavior: Behavior) -> Self {
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        static SERIAL: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> =
            std::sync::OnceLock::new();
        let serial = SERIAL
            .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
            .lock_owned()
            .await;
        let db = Arc::new(Database::memory().unwrap());
        let accounts =
            FixtureTransport::seed_accounts(db.secret_session(), &["synthetic-a", "synthetic-b"])
                .await
                .unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let upstream_calls = calls.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let upstream_release = release.clone();
        let app=axum::Router::new().route("/v2/chat/completions",axum::routing::post(move |headers:HeaderMap,axum::Json(body):axum::Json<Value>|{
            let calls=upstream_calls.clone();let release=upstream_release.clone();async move {
                let first=headers.get("authorization").unwrap()=="Bearer fixture-access-synthetic-a";
                calls.lock().unwrap().push(Captured{headers,body});
                if first&&matches!(behavior,Behavior::FirstRateLimited){return(StatusCode::TOO_MANY_REQUESTS,[("retry-after","0")],axum::Json(json!({"code":429,"msg":"fixture-access-synthetic-a"}))).into_response();}
                if let Behavior::Refused(status,code)=behavior{return(StatusCode::from_u16(status).unwrap(),axum::Json(json!({"error":{"data":{"code":code,"msg":"fixture-access-synthetic-a"}}}))).into_response();}
                if first {
                    if let Behavior::StructuredStreamDenial(code, late) = behavior {
                        let stream = async_stream::stream! {
                            if late {
                                yield Ok::<Bytes, std::io::Error>(Bytes::from_static(FIRST.as_bytes()));
                                release.notified().await;
                            }
                            // Deliberately lacks event:error / top-level error. Exact
                            // upstream numeric code still establishes a refusal.
                            yield Ok::<Bytes, std::io::Error>(Bytes::from(format!("data: {{\"data\":{{\"code\":{code}}}}}\n\n")));
                            yield Ok(Bytes::from_static(DONE.as_bytes()));
                        };
                        return ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response();
                    }
                }
                if first && matches!(behavior, Behavior::UnicodeFirst) {
                    let encoded: String = "fixture-access-synthetic-a".chars().map(|c| format!("\\u{:04x}", c as u32)).collect();
                    let wire = format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{encoded}\"}}}}]}}\n\ndata: [DONE]\n\n");
                    return ([("content-type", "text/event-stream")], wire).into_response();
                }
                if matches!(behavior,Behavior::StreamRefusal|Behavior::StreamFailure){
                    let stream=async_stream::stream!{
                        yield Ok::<Bytes,std::io::Error>(Bytes::from_static(FIRST.as_bytes()));
                        release.notified().await;
                        if matches!(behavior,Behavior::StreamFailure){yield Err(std::io::Error::other("fixture-access-synthetic-a synthetic failure"));}
                        else{yield Ok(Bytes::from_static(b"data: {\"error\":{\"type\":\"access_denied\",\"code\":11140,\"message\":\"WorkBuddy restricted this account\"}}\n\n"));}
                    };
                    return ([("content-type","text/event-stream")],axum::body::Body::from_stream(stream)).into_response();
                }
                ([("content-type","text/event-stream")],format!("{FIRST}{DONE}")).into_response()
            }
        }));
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
            release,
            origin,
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
fn wire(call: &Captured, account: &str) {
    assert_eq!(
        call.headers["authorization"],
        format!("Bearer fixture-access-{account}")
    );
    assert_eq!(call.headers["x-user-id"], account);
    assert_eq!(call.headers["x-domain"], "www.workbuddy.cn");
    assert_eq!(call.headers["user-agent"], "WorkBuddy/5.5.6");
    assert_eq!(call.headers["accept-encoding"], "identity");
    assert_eq!(call.body["stream"], true);
    assert!(!call.headers.contains_key("x-api-key"));
}

#[tokio::test]
async fn workbuddy_chat_original_pool_uses_second_account_after_technical_429() {
    let f = Fixture::new(Behavior::FirstRateLimited).await;
    let request = chat(true);
    let response = f.request("/v1/chat/completions", request.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = collect(response).await;
    assert!(String::from_utf8_lossy(&bytes).contains(TEXT));
    assert!(!String::from_utf8_lossy(&bytes).contains("fixture-access"));
    let calls = f.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    wire(&calls[0], "synthetic-a");
    wire(&calls[1], "synthetic-b");
    assert_eq!(calls[1].body["messages"], request["messages"]);
    assert_eq!(f.transport.attempts(), 2);
    f.idle().await;
}
#[tokio::test]
async fn workbuddy_exact_denials_are_terminal_before_account_failover() {
    for (status, code) in [(403, 11004), (403, 11140), (401, 11140), (200, 11004)] {
        let f = Fixture::new(Behavior::Refused(status, code)).await;
        let response = f.request("/v1/chat/completions", chat(true)).await;
        assert!(!response.status().is_success());
        let bytes = collect(response).await;
        assert!(!String::from_utf8_lossy(&bytes).contains("fixture-access"));
        assert!(String::from_utf8_lossy(&bytes).contains(&format!("workbuddy_{code}")));
        assert_eq!(f.calls.lock().unwrap().len(), 1);
        assert_eq!(f.transport.attempts(), 1);
        f.idle().await;
    }
}
#[tokio::test]
async fn workbuddy_nonstream_request_is_explicitly_unsupported_and_zero_post() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request("/v1/chat/completions", chat(false)).await;
    assert!(!response.status().is_success());
    let _ = collect(response).await;
    assert_eq!(f.transport.attempts(), 0);
    assert!(f.calls.lock().unwrap().is_empty());
    f.idle().await;
}
#[tokio::test]
async fn workbuddy_final_body_rejects_retired_session_after_original_prime() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request("/v1/chat/completions", chat(true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    f.db.secret_session().set_blocked(true);
    assert!(tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
    )
    .await
    .unwrap()
    .is_err());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.idle().await;
}
#[tokio::test]
async fn workbuddy_midstream_failure_and_refusal_never_replay_or_switch_account() {
    for behavior in [Behavior::StreamFailure, Behavior::StreamRefusal] {
        let f = Fixture::new(behavior).await;
        let response = f.request("/v1/chat/completions", chat(true)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let mut emitted = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&emitted).contains(TEXT) {
                emitted.extend_from_slice(&stream.next().await.expect("output absent").unwrap());
            }
        })
        .await
        .unwrap();
        f.release.notify_one();
        let mut failed = false;
        let mut failure = String::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => emitted.extend_from_slice(&bytes),
                    Err(error) => {
                        failed = true;
                        failure = error.to_string();
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        drop(stream);
        let text = String::from_utf8_lossy(&emitted);
        assert_eq!(text.matches(TEXT).count(), 1);
        assert!(!text.contains("fixture-access"));
        if matches!(behavior, Behavior::StreamRefusal) {
            assert!(text.contains("11140") || failure.contains("11140"));
        } else {
            assert!(failed || text.contains("error"));
        }
        assert_eq!(f.calls.lock().unwrap().len(), 1);
        assert_eq!(f.transport.attempts(), 1);
        f.idle().await;
    }
}
#[tokio::test]
async fn workbuddy_claude_original_entry_converts_tools_once_into_chat() {
    let f = Fixture::new(Behavior::Complete).await;
    let response=f.request("/v1/messages",json!({"model":"synthetic-wb-model","stream":true,"max_tokens":64,"system":"preserved system","tools":[{"name":"lookup","description":"test","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],"tool_choice":{"type":"auto"},"messages":[{"role":"user","content":"synthetic prompt"}]})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = collect(response).await;
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(TEXT));
    assert!(text.contains("content_block_delta"));
    let calls = f.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    wire(&calls[0], "synthetic-a");
    assert_eq!(calls[0].body["tools"][0]["type"], "function");
    assert_eq!(calls[0].body["tools"][0]["function"]["name"], "lookup");
    assert!(calls[0].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "system" && m["content"] == "preserved system"));
    f.idle().await;
}
#[tokio::test]
async fn workbuddy_missing_or_foreign_fixture_is_zero_http_without_public_fallback() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f
        .request_with("/v1/chat/completions", chat(true), None)
        .await;
    assert!(!response.status().is_success());
    let _ = collect(response).await;
    assert_eq!(f.transport.attempts(), 0);
    for id in ["workbuddy-a", "workbuddy-b"] {
        f.proxy.reset_provider_circuit_breaker(id, "codex").await;
    }
    let foreign = Database::memory().unwrap();
    FixtureTransport::seed_accounts(foreign.secret_session(), &["synthetic-foreign"])
        .await
        .unwrap();
    let wrong = FixtureTransport::loopback(foreign.secret_session(), &f.origin).unwrap();
    let response = f
        .request_with("/v1/chat/completions", chat(true), Some(wrong.clone()))
        .await;
    assert!(!response.status().is_success());
    let _ = collect(response).await;
    assert_eq!(wrong.attempts(), 0);
    assert!(f.calls.lock().unwrap().is_empty());
    f.idle().await;
}

#[tokio::test]
async fn workbuddy_codex_responses_original_entry_roundtrips_text_and_tools_via_chat() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request("/v1/responses", json!({
        "model":"synthetic-wb-model", "stream":true, "instructions":"preserved Codex instruction",
        "tools":[{"type":"function","name":"lookup","description":"test","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}],
        "input":"synthetic prompt"
    })).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = collect(response).await;
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("response.output_text.delta"));
    assert!(text.contains("response.completed"));
    assert!(text.contains(TEXT));
    let calls = f.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    wire(&calls[0], "synthetic-a");
    assert_eq!(calls[0].body["tools"][0]["type"], "function");
    assert_eq!(calls[0].body["tools"][0]["function"]["name"], "lookup");
    assert!(calls[0].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            matches!(message["role"].as_str(), Some("system" | "developer"))
                && message["content"] == "preserved Codex instruction"
        }));
    f.idle().await;
}
#[tokio::test]
async fn workbuddy_unsafe_first_event_is_terminal_and_never_uses_second_account() {
    for endpoint in ["/v1/chat/completions", "/v1/responses"] {
        let f = Fixture::new(Behavior::UnicodeFirst).await;
        let body = if endpoint.ends_with("responses") {
            json!({"model":"synthetic-wb-model","stream":true,"input":"synthetic prompt"})
        } else {
            chat(true)
        };
        let response = f.request(endpoint, body).await;
        assert!(!response.status().is_success());
        let bytes = collect(response).await;
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("fixture-access"));
        assert!(!text.contains("\\u0066"));
        assert!(!text.contains(TEXT));
        assert_eq!(f.calls.lock().unwrap().len(), 1);
        assert_eq!(f.transport.attempts(), 1);
        f.idle().await;
    }
}

#[tokio::test]
async fn workbuddy_structured_sse_refusal_keeps_meaning_at_all_original_entries() {
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        for code in [11004, 11140] {
            for late in [false, true] {
                let f = Fixture::new(Behavior::StructuredStreamDenial(code, late)).await;
                let body = if path.ends_with("responses") {
                    json!({"model":"synthetic-wb-model","stream":true,"input":"synthetic prompt"})
                } else {
                    json!({"model":"synthetic-wb-model","stream":true,"max_tokens":256,"messages":[{"role":"user","content":"synthetic prompt"}]})
                };
                let response = f.request(path, body).await;
                if !late {
                    assert!(!response.status().is_success());
                }
                let mut stream = response.into_body().into_data_stream();
                let mut bytes = Vec::new();
                let mut failure = String::new();
                tokio::time::timeout(Duration::from_secs(5), async {
                    if late {
                        while !String::from_utf8_lossy(&bytes).contains(TEXT) {
                            bytes.extend_from_slice(
                                &stream.next().await.expect("visible output absent").unwrap(),
                            );
                        }
                        f.release.notify_one();
                    }
                    while let Some(chunk) = stream.next().await {
                        match chunk {
                            Ok(chunk) => bytes.extend_from_slice(&chunk),
                            Err(error) => {
                                failure = error.to_string();
                                break;
                            }
                        }
                    }
                })
                .await
                .unwrap();
                drop(stream);
                let text = String::from_utf8_lossy(&bytes);
                let terminal = format!("{text}{failure}");
                assert!(
                    terminal.contains(&code.to_string()),
                    "{path}: refusal code lost"
                );
                let meaning = if code == 11004 {
                    "unapproved channel"
                } else {
                    "unusual activity"
                };
                assert!(terminal.contains(meaning), "{path}: refusal meaning lost");
                assert!(!text.contains("response.completed"));
                assert!(!text.contains("message_stop"));
                assert!(!text.contains("fixture-access"));
                assert_eq!(text.matches(TEXT).count(), usize::from(late));
                assert_eq!(f.calls.lock().unwrap().len(), 1);
                assert_eq!(f.transport.attempts(), 1);
                f.idle().await;
            }
        }
    }
}

#[test]
fn workbuddy_managed_protocol_is_mutually_exclusive_despite_editable_metadata() {
    use crate::proxy::providers::{
        codex_provider_uses_anthropic, codex_provider_uses_chat_completions, get_claude_api_format,
    };
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
