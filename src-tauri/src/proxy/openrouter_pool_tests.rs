//! Original Provider SQL/vault -> pool router -> ProxyServer -> loopback HTTP.
//! Synthetic API keys only; no provisioning, model discovery, or public endpoint.
//! Register as a child of proxy::server, like the existing managed pool tests.
use super::*;
use crate::provider::Provider;
use axum::{
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

const KEY_A: &str = "synthetic-openrouter-key-a";
const KEY_B: &str = "synthetic-openrouter-key-b";
const ANSWER: &str = "synthetic OpenRouter selected answer";

#[derive(Clone, Copy)]
enum Protocol {
    ClaudeNative,
    CodexNative,
    ClaudeChat,
}
impl Protocol {
    fn app(self) -> &'static str {
        match self {
            Self::CodexNative => "codex",
            _ => "claude",
        }
    }
    fn ingress(self) -> &'static str {
        match self {
            Self::CodexNative => "/v1/responses",
            _ => "/v1/messages",
        }
    }
    fn suffix(self) -> &'static str {
        match self {
            Self::ClaudeNative => "/api/v1/messages",
            Self::CodexNative => "/api/v1/responses",
            Self::ClaudeChat => "/api/v1/chat/completions",
        }
    }
    fn model(self, account: &str) -> String {
        format!(
            "{}/synthetic-{account}",
            if matches!(self, Self::CodexNative) {
                "openai"
            } else {
                "anthropic"
            }
        )
    }
}

#[derive(Clone, Copy)]
enum Behavior {
    Complete,
    FirstStatus(u16),
    Denial(u16),
    LateFailure,
    AllStatus(u16),
}
#[derive(Clone)]
struct Captured {
    account: String,
    uri: String,
    headers: HeaderMap,
    body: Value,
}

struct Fixture {
    db: Arc<Database>,
    proxy: ProxyServer,
    protocol: Protocol,
    ids: [String; 2],
    origin: String,
    calls: Arc<std::sync::Mutex<Vec<Captured>>>,
    release: Arc<tokio::sync::Notify>,
    upstream: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl Fixture {
    async fn new(protocol: Protocol, behavior: Behavior) -> Self {
        // Match the original application setup (lib.rs) before creating the
        // native Claude hyper-rustls client; oneshot does not run app setup.
        let _ = rustls::crypto::ring::default_provider().install_default();
        assert!(
            std::env::var_os("CC_SWITCH_TEST_HOME").is_some(),
            "test runner must provide an isolated home"
        );
        let db = Arc::new(Database::memory().unwrap());
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let release = Arc::new(tokio::sync::Notify::new());
        let captured = calls.clone();
        let released = release.clone();
        let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let captured = captured.clone();
            let release = released.clone();
            async move {
                let account = uri.path().split('/').nth(2).unwrap_or("").to_owned();
                assert!(matches!(account.as_str(), "a" | "b" | "outside"));
                captured.lock().unwrap().push(Captured { account: account.clone(), uri: uri.to_string(), headers, body: body.clone() });
                if let Behavior::Denial(status) = behavior {
                    return (StatusCode::from_u16(status).unwrap(), axum::Json(json!({"error":{"type":"permission_denied","message":"Synthetic request is not permitted"}}))).into_response();
                }
                let status = match behavior {
                    Behavior::FirstStatus(status) if account == "a" => Some(status),
                    Behavior::AllStatus(status) => Some(status),
                    _ => None,
                };
                if let Some(status) = status {
                    let kind = if status == 401 { "invalid_api_key" } else if status == 429 { "rate_limit_error" } else { "api_error" };
                    return (StatusCode::from_u16(status).unwrap(), [("retry-after", "0")], axum::Json(json!({"error":{"type":kind,"message":"Synthetic technical failure"}}))).into_response();
                }
                if matches!(behavior, Behavior::LateFailure) && account == "a" {
                    let prefix = stream_prefix(protocol, &body);
                    let stream = async_stream::stream! {
                        yield Ok::<Bytes, std::io::Error>(Bytes::from(prefix));
                        release.notified().await;
                        yield Err(std::io::Error::other("Synthetic transport interrupted after visible output"));
                    };
                    return ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response();
                }
                axum::Json(success(protocol, &body)).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let nonce = uuid::Uuid::new_v4();
        let ids = [
            format!("openrouter-synthetic-{nonce}-a"),
            format!("openrouter-synthetic-{nonce}-b"),
        ];
        for (index, account) in ["a", "b"].iter().enumerate() {
            let provider = provider(protocol, &origin, &ids[index], account);
            db.save_provider(protocol.app(), &provider).unwrap();
        }
        crate::proxy::application_routing::set_order(&db, protocol.app(), &ids).unwrap();
        db.set_current_provider(protocol.app(), &ids[0]).unwrap();
        let mut config = db.get_proxy_config_for_app(protocol.app()).await.unwrap();
        config.enabled = true;
        config.auto_failover_enabled = true;
        config.max_retries = 2;
        config.streaming_first_byte_timeout = 3;
        config.streaming_idle_timeout = 3;
        config.non_streaming_timeout = 5;
        db.update_proxy_config_for_app(config).await.unwrap();
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
            protocol,
            ids,
            origin,
            calls,
            release,
            upstream,
        }
    }
    fn select(&self, index: usize) {
        self.db
            .set_current_provider(self.protocol.app(), &self.ids[index])
            .unwrap();
    }
    async fn request(&self, body: Value) -> axum::response::Response {
        use tower::ServiceExt;
        tokio::time::timeout(
            Duration::from_secs(8),
            self.proxy.build_router().oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(self.protocol.ingress())
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer synthetic-untrusted-inbound")
                    .header("x-api-key", "synthetic-untrusted-inbound")
                    .header("anthropic-version", "2023-06-01")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .expect("original router timed out")
        .unwrap()
    }
    fn calls(&self) -> Vec<Captured> {
        self.calls.lock().unwrap().clone()
    }
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.proxy.get_status().await.active_connections != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("original connection guard was not released");
    }
}

fn provider(protocol: Protocol, origin: &str, id: &str, account: &str) -> Provider {
    let key = if account == "a" { KEY_A } else { KEY_B };
    // Preserve the original OpenRouter classification marker while replacing the
    // entire network origin with loopback, as the pre-existing OpenRouter tests do.
    let base = format!("{origin}/openrouter.ai/{account}/api");
    let model = protocol.model(account);
    let settings = match protocol {
        Protocol::CodexNative => json!({
            "auth": {"OPENAI_API_KEY": key},
            "config": format!("model_provider = \"custom\"\nmodel = \"{model}\"\n[model_providers.custom]\nname = \"openrouter\"\nbase_url = \"{base}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = true\n")
        }),
        _ => {
            let mut settings = json!({"env":{
                "ANTHROPIC_BASE_URL":base,"ANTHROPIC_AUTH_TOKEN":key,
                "ANTHROPIC_MODEL":model,"ANTHROPIC_DEFAULT_SONNET_MODEL":model
            }});
            if matches!(protocol, Protocol::ClaudeChat) {
                settings["api_format"] = json!("openai_chat");
            }
            settings
        }
    };
    let mut provider = Provider::with_id(
        id.into(),
        format!("OpenRouter synthetic {account}"),
        settings,
        Some("https://openrouter.ai".into()),
    );
    provider.category = Some("aggregator".into());
    provider
}

fn request_body(protocol: Protocol, stream: bool) -> Value {
    match protocol {
        Protocol::CodexNative => json!({
            "model":"client-default-must-not-override-saved-model", "stream":stream,
            "instructions":"Keep the user's instructions and tool history.",
            "input":[
                {"role":"user","content":[{"type":"input_text","text":"earlier question"}]},
                {"type":"function_call","call_id":"old-call","name":"lookup","arguments":"{\"q\":\"kept\"}"},
                {"type":"function_call_output","call_id":"old-call","output":"old-result"},
                {"role":"user","content":[{"type":"input_text","text":"continue"}]}
            ],
            "tools":[{"type":"function","name":"lookup","description":"Synthetic lookup","parameters":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}]
        }),
        _ => json!({
            "model":"claude-sonnet-client-alias", "max_tokens":64, "stream":stream,
            "system":[{"type":"text","text":"Keep the user's instructions and tool history."}],
            "messages":[
                {"role":"user","content":"earlier question"},
                {"role":"assistant","content":[{"type":"tool_use","id":"old-call","name":"lookup","input":{"q":"kept"}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"old-call","content":"old-result"},{"type":"text","text":"continue"}]}
            ],
            "tools":[{"name":"lookup","description":"Synthetic lookup","input_schema":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"]}}]
        }),
    }
}

fn success(protocol: Protocol, request: &Value) -> Value {
    match protocol {
        Protocol::ClaudeNative => {
            json!({"id":"msg-synthetic","type":"message","role":"assistant","model":request["model"],
            "content":[{"type":"text","text":ANSWER},{"type":"tool_use","id":"new-call","name":"lookup","input":{"q":"new"}}],
            "stop_reason":"tool_use","usage":{"input_tokens":4,"output_tokens":5}})
        }
        Protocol::CodexNative => {
            json!({"id":"resp-synthetic","object":"response","created_at":1,"status":"completed","model":request["model"],
            "output":[{"type":"message","id":"msg-synthetic","role":"assistant","status":"completed","content":[{"type":"output_text","text":ANSWER,"annotations":[]}]},
                {"type":"function_call","id":"fc-synthetic","call_id":"new-call","name":"lookup","arguments":"{\"q\":\"new\"}","status":"completed"}],
            "usage":{"input_tokens":4,"output_tokens":5,"total_tokens":9}})
        }
        Protocol::ClaudeChat => {
            json!({"id":"chat-synthetic","object":"chat.completion","created":1,"model":request["model"],
            "choices":[{"index":0,"message":{"role":"assistant","content":ANSWER,"tool_calls":[{"id":"new-call","type":"function","function":{"name":"lookup","arguments":"{\"q\":\"new\"}"}}]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":4,"completion_tokens":5,"total_tokens":9}})
        }
    }
}

fn stream_prefix(protocol: Protocol, request: &Value) -> String {
    match protocol {
        Protocol::ClaudeNative => format!(
            "event: message_start\ndata: {}\n\nevent: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\n",
            json!({"type":"message_start","message":{"id":"msg-synthetic","type":"message","role":"assistant","model":request["model"],"content":[],"usage":{"input_tokens":4,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":ANSWER}})
        ),
        Protocol::CodexNative => format!(
            "event: response.created\ndata: {}\n\nevent: response.output_text.delta\ndata: {}\n\n",
            json!({"type":"response.created","response":{"id":"resp-synthetic","object":"response","status":"in_progress","model":request["model"],"output":[]}}),
            json!({"type":"response.output_text.delta","response_id":"resp-synthetic","item_id":"msg-synthetic","output_index":0,"content_index":0,"delta":ANSWER})
        ),
        Protocol::ClaudeChat => unreachable!("the old Chat parser already has original-main replay tests"),
    }
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
fn assert_wire(call: &Captured, protocol: Protocol, account: &str) {
    assert_eq!(call.account, account);
    assert_eq!(
        call.uri,
        format!("/openrouter.ai/{account}{}", protocol.suffix())
    );
    assert_eq!(
        call.headers["authorization"],
        format!("Bearer {}", if account == "a" { KEY_A } else { KEY_B })
    );
    assert!(
        !call.headers.contains_key("x-api-key"),
        "inbound client key must not leak upstream"
    );
    assert_eq!(call.body["model"], protocol.model(account));
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_claude_native_selects_second_saved_key_and_preserves_tools_history() {
    let f = Fixture::new(Protocol::ClaudeNative, Behavior::Complete).await;
    f.select(1);
    let input = request_body(f.protocol, false);
    let response = f.request(input.clone()).await;
    assert!(response.status().is_success());
    let output: Value = serde_json::from_slice(&collect(response).await).unwrap();
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert_wire(&calls[0], f.protocol, "b");
    assert_eq!(calls[0].body["messages"], input["messages"]);
    assert_eq!(calls[0].body["tools"], input["tools"]);
    assert_eq!(
        calls[0].body["system"][0]["text"],
        input["system"][0]["text"]
    );
    assert_eq!(output["content"][1]["id"], "new-call");
    assert_eq!(output["content"][1]["input"], json!({"q":"new"}));
    f.idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_codex_native_uses_original_key_owner_and_saved_source_model() {
    let f = Fixture::new(Protocol::CodexNative, Behavior::Complete).await;
    f.select(1);
    let raw: String =
        f.db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT settings_config FROM providers WHERE id = ?1 AND app_type = 'codex'",
                [&f.ids[1]],
                |row| row.get(0),
            )
            .unwrap();
    assert!(
        !raw.contains(KEY_B),
        "original Provider SQL must not persist plaintext keys"
    );
    let hydrated =
        f.db.get_provider_by_id(&f.ids[1], "codex")
            .unwrap()
            .unwrap();
    assert_eq!(hydrated.settings_config["auth"]["OPENAI_API_KEY"], KEY_B);
    let input = request_body(f.protocol, false);
    let response = f.request(input.clone()).await;
    assert!(response.status().is_success());
    let output: Value = serde_json::from_slice(&collect(response).await).unwrap();
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert_wire(&calls[0], f.protocol, "b");
    assert_eq!(calls[0].body["input"], input["input"]);
    assert_eq!(calls[0].body["tools"], input["tools"]);
    assert_eq!(calls[0].body["instructions"], input["instructions"]);
    assert_eq!(output["output"][1]["call_id"], "new-call");
    assert_eq!(output["output"][1]["arguments"], "{\"q\":\"new\"}");
    f.idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_explicit_legacy_chat_mode_converts_tools_once() {
    let f = Fixture::new(Protocol::ClaudeChat, Behavior::Complete).await;
    let response = f.request(request_body(f.protocol, false)).await;
    assert!(response.status().is_success());
    let output: Value = serde_json::from_slice(&collect(response).await).unwrap();
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert_wire(&calls[0], f.protocol, "a");
    assert_eq!(calls[0].body["tools"][0]["function"]["name"], "lookup");
    let messages = calls[0].body["messages"].as_array().unwrap();
    let old_call = messages
        .iter()
        .find(|m| m["tool_calls"][0]["id"] == "old-call")
        .unwrap();
    assert_eq!(
        old_call["tool_calls"][0]["function"]["arguments"],
        "{\"q\":\"kept\"}"
    );
    let result = messages
        .iter()
        .find(|m| m["tool_call_id"] == "old-call")
        .unwrap();
    assert_eq!(result["content"], "old-result");
    assert_eq!(output["content"][1]["id"], "new-call");
    assert_eq!(output["stop_reason"], "tool_use");
    f.idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_401_and_technical_429_use_only_next_configured_key() {
    for protocol in [Protocol::ClaudeNative, Protocol::CodexNative] {
        for status in [401, 429] {
            let f = Fixture::new(protocol, Behavior::FirstStatus(status)).await;
            let response = f.request(request_body(protocol, false)).await;
            assert!(
                response.status().is_success(),
                "technical {status} did not use the next eligible provider"
            );
            let body = collect(response).await;
            assert!(String::from_utf8_lossy(&body).contains(ANSWER));
            let calls = f.calls();
            assert_eq!(calls.len(), 2);
            assert_wire(&calls[0], protocol, "a");
            assert_wire(&calls[1], protocol, "b");
            f.idle().await;
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_explicit_denial_does_not_switch_even_with_429_status() {
    for protocol in [Protocol::ClaudeNative, Protocol::CodexNative] {
        for status in [403, 429] {
            let f = Fixture::new(protocol, Behavior::Denial(status)).await;
            let response = f.request(request_body(protocol, false)).await;
            assert_eq!(response.status().as_u16(), status);
            let body = collect(response).await;
            assert!(!String::from_utf8_lossy(&body).contains(ANSWER));
            let calls = f.calls();
            assert_eq!(calls.len(), 1);
            assert_wire(&calls[0], protocol, "a");
            f.idle().await;
        }
    }
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_disabled_failover_keeps_a_technical_failure_on_selected_key() {
    let f = Fixture::new(Protocol::CodexNative, Behavior::FirstStatus(429)).await;
    let mut config = f.db.get_proxy_config_for_app("codex").await.unwrap();
    config.auto_failover_enabled = false;
    f.db.update_proxy_config_for_app(config).await.unwrap();
    let response = f.request(request_body(f.protocol, false)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = collect(response).await;
    let calls = f.calls();
    assert_eq!(calls.len(), 1);
    assert_wire(&calls[0], f.protocol, "a");
    f.idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_output_started_then_transport_failure_never_replays() {
    for protocol in [Protocol::ClaudeNative, Protocol::CodexNative] {
        let f = Fixture::new(protocol, Behavior::LateFailure).await;
        let response = f.request(request_body(protocol, true)).await;
        assert!(response.status().is_success());
        let mut stream = response.into_body().into_data_stream();
        let mut body = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !String::from_utf8_lossy(&body).contains(ANSWER) {
                body.extend_from_slice(
                    &stream
                        .next()
                        .await
                        .expect("missing visible output")
                        .unwrap(),
                );
            }
        })
        .await
        .unwrap();
        f.release.notify_one();
        let mut failed = false;
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => body.extend_from_slice(&bytes),
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(
            failed || text.contains("event: error") || text.contains("\"type\":\"error\""),
            "interruption must be observable"
        );
        assert!(!text.contains("message_stop"));
        assert!(!text.contains("response.completed"));
        let calls = f.calls();
        assert_eq!(calls.len(), 1);
        assert_wire(&calls[0], protocol, "a");
        if failed {
            assert_eq!(
                f.proxy.get_status().await.active_connections,
                1,
                "the test still owns the errored Body before dropping it"
            );
        }
        // On Body error a real client discards the body. Our early break still
        // owns it, including the original stream's RAII connection guard.
        drop(stream);
        f.idle().await;
    }
}

#[tokio::test]
#[serial_test::serial]
async fn openrouter_saved_but_outside_applied_chain_is_not_a_fallback() {
    let f = Fixture::new(Protocol::CodexNative, Behavior::AllStatus(503)).await;
    let outside = provider(
        f.protocol,
        &f.origin,
        "openrouter-synthetic-outside",
        "outside",
    );
    f.db.save_provider("codex", &outside).unwrap();
    // Ordinary Provider insertion intentionally appends to an initialized chain.
    // Model the user's subsequent apply action before claiming this row is out.
    assert!(crate::proxy::application_routing::chain_ids(&f.db, "codex")
        .unwrap()
        .contains(&outside.id));
    crate::proxy::application_routing::set_order(&f.db, "codex", &f.ids).unwrap();
    assert_eq!(
        crate::proxy::application_routing::chain_ids(&f.db, "codex").unwrap(),
        f.ids.to_vec()
    );
    let response = f.request(request_body(f.protocol, false)).await;
    assert!(!response.status().is_success());
    let _ = collect(response).await;
    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    assert_wire(&calls[0], f.protocol, "a");
    assert_wire(&calls[1], f.protocol, "b");
    assert!(!calls.iter().any(|call| call.account == "outside"));
    f.idle().await;
}
