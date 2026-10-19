//! Tests of the original ProxyServer -> pool -> private owner -> final Body.
//! Only the upstream HTTP server is synthetic. No replacement proxy/router.
use super::*;
use crate::minimax_code::{catalog::Region, inference::FixtureTransport};
use crate::provider::{AuthBinding, AuthBindingSource, Provider, ProviderMeta};
use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

const TEXT: &str = "synthetic answer";
const START_AND_TEXT: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_synthetic\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"MiniMax-M2.7\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"synthetic answer\"}}\n\n"
);
const STOP: &str = concat!(
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
);

#[derive(Clone, Copy)]
enum UpstreamBehavior {
    Complete,
    FirstRateLimited,
    FailAfterOutput,
}

#[derive(Clone)]
struct CapturedRequest {
    region: String,
    headers: HeaderMap,
    body: Value,
}

struct MiniMaxRouterFixture {
    db: Arc<Database>,
    proxy: ProxyServer,
    transport: FixtureTransport,
    calls: Arc<std::sync::Mutex<Vec<CapturedRequest>>>,
    release_error: Arc<tokio::sync::Notify>,
    origin: String,
    upstream: tokio::task::JoinHandle<()>,
    _isolation: tokio::sync::OwnedMutexGuard<()>,
}
impl Drop for MiniMaxRouterFixture {
    fn drop(&mut self) {
        self.upstream.abort();
    }
}
impl MiniMaxRouterFixture {
    async fn new(behavior: UpstreamBehavior) -> Self {
        // Follow the original crate's test-home isolation. Never consult the
        // user's normal settings home when the real handlers run.
        assert!(std::env::var_os("CC_SWITCH_TEST_HOME").is_some());
        let isolation = crate::minimax_code::isolated_fixture_guard().await;
        let db = Arc::new(Database::memory().unwrap());
        let owner = FixtureTransport::seed_accounts(
            db.secret_session(),
            &[("synthetic-a", Region::Cn), ("synthetic-b", Region::Global)],
        )
        .await
        .unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_for_upstream = calls.clone();
        let release_error = Arc::new(tokio::sync::Notify::new());
        let gate = release_error.clone();
        let app = axum::Router::new().route(
            "/{region}/mavis/api/v1/llm/v1/messages",
            axum::routing::post(move |axum::extract::Path(region): axum::extract::Path<String>, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let calls = calls_for_upstream.clone();
                let gate = gate.clone();
                async move {
                    let first = headers.get("authorization").unwrap().to_str().unwrap() == "Bearer synthetic-a-access";
                    calls.lock().unwrap().push(CapturedRequest {region, headers, body: body.clone()});
                    if first && matches!(behavior, UpstreamBehavior::FirstRateLimited) {
                        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "0")], axum::Json(json!({"error":{"type":"rate_limit_error","message":"synthetic-a-access"}}))).into_response();
                    }
                    if body["stream"] == true {
                        if matches!(behavior, UpstreamBehavior::FailAfterOutput) {
                            // The test releases this gate only after downstream
                            // converted text was observed. Timing cannot turn
                            // this into a retryable first-byte failure.
                            let stream = async_stream::stream! {
                                yield Ok::<Bytes, std::io::Error>(Bytes::from_static(START_AND_TEXT.as_bytes()));
                                gate.notified().await;
                                yield Err(std::io::Error::other("synthetic-a-access interrupted"));
                            };
                            return ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response();
                        }
                        return ([("content-type", "text/event-stream")], format!("{START_AND_TEXT}{STOP}")).into_response();
                    }
                    axum::Json(json!({"id":"msg_synthetic","type":"message","role":"assistant","model":"MiniMax-M2.7","content":[{"type":"text","text":TEXT}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":2}})).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let upstream = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let transport = FixtureTransport::loopback(&owner, &origin).await.unwrap();
        for (position, id) in ["synthetic-a", "synthetic-b"].iter().enumerate() {
            let mut provider = Provider::with_id(
                (*id).into(),
                (*id).into(),
                json!({"model":"MiniMax-M2.7"}),
                None,
            );
            provider.sort_index = Some(position);
            provider.meta = Some(ProviderMeta {
                auth_binding: Some(AuthBinding {
                    source: AuthBindingSource::ManagedAccount,
                    auth_provider: Some("minimax_code".into()),
                    account_id: Some((*id).into()),
                }),
                api_format: Some("anthropic".into()),
                ..Default::default()
            });
            db.save_provider("codex", &provider).unwrap();
            db.add_to_failover_queue("codex", id).unwrap();
        }
        db.set_current_provider("codex", "synthetic-a").unwrap();
        let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
        config.auto_failover_enabled = true;
        config.max_retries = 2;
        config.non_streaming_timeout = 5;
        config.streaming_first_byte_timeout = 3;
        config.streaming_idle_timeout = 3;
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
            transport,
            calls,
            release_error,
            origin,
            upstream,
            _isolation: isolation,
        }
    }
    async fn request(&self, stream: bool) -> axum::response::Response {
        self.request_body(
            json!({"model":"MiniMax-M2.7","input":"synthetic prompt","stream":stream}),
        )
        .await
    }
    async fn request_body(&self, body: Value) -> axum::response::Response {
        self.request_with_transport(body, Some(self.transport.clone()))
            .await
    }
    async fn request_with_transport(
        &self,
        body: Value,
        transport: Option<FixtureTransport>,
    ) -> axum::response::Response {
        use tower::ServiceExt;
        let mut router = self.proxy.build_router();
        if let Some(transport) = transport {
            router = router.layer(axum::Extension(transport));
        }
        tokio::time::timeout(
            Duration::from_secs(8),
            router.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/responses")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer untrusted-client")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            ),
        )
        .await
        .expect("original router must finish response admission")
        .unwrap()
    }
    async fn retire_first(&self) {
        // retire is a local original-owner operation and does not call Transport.
        crate::minimax_code::checkin::Engine {
            session: self.db.secret_session(),
            transport: &crate::minimax_code::http::OfficialHttp,
        }
        .retire("synthetic-a")
        .await
        .unwrap();
    }
    async fn assert_idle(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.proxy.get_status().await.active_connections != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("original active connection guard must be released");
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
fn sse_data(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn assert_wire(call: &CapturedRequest, account: &str, region: &str) {
    assert_eq!(call.region, region);
    assert_eq!(
        call.headers["authorization"],
        format!("Bearer {account}-access")
    );
    assert_eq!(call.headers["user-agent"], "MiniMaxAgent");
    assert_eq!(call.headers["anthropic-version"], "2023-06-01");
    assert_eq!(call.headers["accept-encoding"], "identity");
    assert_eq!(call.headers["x-mavis-agent-id"], "main");
    assert!(call.headers["x-mavis-session-id"]
        .to_str()
        .unwrap()
        .starts_with("loongport-"));
    assert!(call.headers["x-mavis-timezone-offset"]
        .to_str()
        .unwrap()
        .parse::<i32>()
        .is_ok());
    assert!(!call.headers.contains_key("x-api-key"));
}

#[tokio::test]
async fn minimax_responses_pool_uses_second_account_after_429() {
    for stream in [false, true] {
        let fixture = MiniMaxRouterFixture::new(UpstreamBehavior::FirstRateLimited).await;
        let response = fixture.request(stream).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = collect(response).await;
        assert!(std::str::from_utf8(&body).unwrap().contains(TEXT));
        assert!(!std::str::from_utf8(&body)
            .unwrap()
            .contains("synthetic-a-access"));
        let calls = fixture.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        assert_wire(&calls[0], "synthetic-a", "cn");
        assert_wire(&calls[1], "synthetic-b", "global");
        assert_ne!(
            calls[0].headers["x-mavis-session-id"],
            calls[1].headers["x-mavis-session-id"]
        );
        assert_eq!(calls[1].body["messages"][0]["role"], "user");
        assert_eq!(fixture.transport.attempts(), 2);
        fixture.assert_idle().await;
    }
}

#[tokio::test]
async fn minimax_responses_final_body_rejects_retired_buffer_and_stream() {
    for stream in [false, true] {
        let fixture = MiniMaxRouterFixture::new(UpstreamBehavior::Complete).await;
        let response = fixture.request(stream).await;
        assert_eq!(response.status(), StatusCode::OK);
        fixture.retire_first().await;
        assert!(tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
        )
        .await
        .unwrap()
        .is_err());
        assert_eq!(fixture.calls.lock().unwrap().len(), 1);
        fixture.assert_idle().await;
    }
}

#[tokio::test]
async fn minimax_responses_midstream_failure_never_replays_output_or_switches_account() {
    let fixture = MiniMaxRouterFixture::new(UpstreamBehavior::FailAfterOutput).await;
    let response = fixture.request(true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut emitted = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !sse_data(&emitted)
            .iter()
            .any(|event| event["type"] == "response.output_text.delta" && event["delta"] == TEXT)
        {
            emitted.extend_from_slice(
                &stream
                    .next()
                    .await
                    .expect("stream ended before visible output")
                    .unwrap(),
            );
        }
    })
    .await
    .unwrap();
    fixture.release_error.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = stream.next().await {
            emitted.extend_from_slice(&chunk.unwrap());
        }
    })
    .await
    .unwrap();
    let events = sse_data(&emitted);
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "response.output_text.delta" && e["delta"] == TEXT)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "response.failed")
            .count(),
        1
    );
    assert!(!events.iter().any(|e| e["type"] == "response.completed"));
    assert!(!std::str::from_utf8(&emitted)
        .unwrap()
        .contains("synthetic-a-access"));
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    assert_eq!(fixture.transport.attempts(), 1);
    fixture.assert_idle().await;
}

#[tokio::test]
async fn minimax_responses_preserves_tools_history_and_selected_pool_identity() {
    let fixture = MiniMaxRouterFixture::new(UpstreamBehavior::Complete).await;
    let response = fixture.request_body(json!({
        "model":"MiniMax-M2.7", "stream":false, "max_output_tokens":64,
        "account_id":"synthetic-b", "instructions":"base instruction",
        "tools":[{"type":"function","name":"lookup","description":"synthetic tool","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}],
        "tool_choice":{"type":"function","name":"lookup"},
        "input":[
            {"role":"system","content":"system history"},
            {"role":"developer","content":"developer history"},
            {"role":"user","content":"first question"},
            {"role":"assistant","content":[{"type":"output_text","text":"prior answer"}]},
            {"type":"function_call","call_id":"call_synthetic","name":"lookup","arguments":"{\"city\":\"Tokyo\"}"},
            {"type":"function_call_output","call_id":"call_synthetic","output":"synthetic tool result"},
            {"role":"user","content":"followup question"}
        ]
    })).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = collect(response).await;
    let output: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(output["object"], "response");
    let calls = fixture.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_wire(&calls[0], "synthetic-a", "cn");
    let wire = &calls[0].body;
    assert_eq!(
        wire["system"],
        json!([{"type":"text", "text":"base instruction\n\nsystem history\n\ndeveloper history", "cache_control":{"type":"ephemeral"}}])
    );
    assert_eq!(wire["max_tokens"], 64);
    assert_eq!(wire["tools"][0]["name"], "lookup");
    assert_eq!(
        wire["tools"][0]["input_schema"]["required"],
        json!(["city"])
    );
    assert_eq!(wire["tool_choice"], json!({"type":"tool","name":"lookup"}));
    let messages = wire["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .all(|m| m["role"] == "user" || m["role"] == "assistant"));
    let blocks: Vec<&Value> = messages
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap())
        .collect();
    for text in ["first question", "prior answer", "followup question"] {
        assert!(blocks
            .iter()
            .any(|block| block["type"] == "text" && block["text"] == text));
    }
    assert!(blocks.iter().any(|b| b["type"] == "tool_use"
        && b["id"] == "call_synthetic"
        && b["input"]["city"] == "Tokyo"));
    assert!(blocks.iter().any(|b| b["type"] == "tool_result"
        && b["tool_use_id"] == "call_synthetic"
        && b["content"] == "synthetic tool result"));
    fixture.assert_idle().await;
}

#[tokio::test]
async fn minimax_router_fixture_missing_or_cross_root_is_zero_http() {
    let fixture = MiniMaxRouterFixture::new(UpstreamBehavior::Complete).await;
    let body = json!({"model":"MiniMax-M2.7","input":"synthetic prompt","stream":false});
    let response = fixture.request_with_transport(body.clone(), None).await;
    assert!(!response.status().is_success());
    let _ = collect(response).await;
    assert_eq!(fixture.transport.attempts(), 0);
    assert!(fixture.calls.lock().unwrap().is_empty());
    fixture.assert_idle().await;

    for id in ["synthetic-a", "synthetic-b"] {
        fixture
            .proxy
            .reset_provider_circuit_breaker(id, "codex")
            .await;
    }
    let foreign = Database::memory().unwrap();
    let owner = FixtureTransport::seed_accounts(
        foreign.secret_session(),
        &[("synthetic-a", Region::Cn), ("synthetic-b", Region::Global)],
    )
    .await
    .unwrap();
    let wrong = FixtureTransport::loopback(&owner, &fixture.origin)
        .await
        .unwrap();
    let response = fixture
        .request_with_transport(body.clone(), Some(wrong.clone()))
        .await;
    assert!(!response.status().is_success());
    let _ = collect(response).await;
    assert_eq!(wrong.attempts(), 0);
    assert!(fixture.calls.lock().unwrap().is_empty());
    fixture.assert_idle().await;
    // A valid root still succeeds after both refusals; circuit state must not
    // accidentally make the cross-root case pass without reaching the owner.
    for id in ["synthetic-a", "synthetic-b"] {
        fixture
            .proxy
            .reset_provider_circuit_breaker(id, "codex")
            .await;
    }
    let response = fixture.request_body(body).await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = collect(response).await;
    assert_eq!(fixture.calls.lock().unwrap().len(), 1);
    fixture.assert_idle().await;
}
