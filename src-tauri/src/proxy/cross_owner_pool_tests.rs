//! Cross-owner contracts through the original Responses router and private owners.
//! All upstreams, credentials, and tool results are synthetic loopback fixtures.
//! These are not live-account, real-cost, or crash-recovery acceptance tests.
use super::*;
use crate::zcode_accounts::inference::FixtureTransport as ZCodeTransport;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
enum ZCodeReply {
    Text,
    ToolThenText,
    Denied,
}

struct CrossOwnerFixture {
    minimax: MiniMaxRouterFixture,
    zcode: ZCodeTransport,
    zcode_calls: Arc<std::sync::Mutex<Vec<(HeaderMap, Value)>>>,
    zcode_token: String,
    zcode_server: tokio::task::JoinHandle<()>,
}
impl Drop for CrossOwnerFixture {
    fn drop(&mut self) {
        self.zcode_server.abort();
    }
}
impl CrossOwnerFixture {
    async fn new(minimax_reply: UpstreamBehavior, zcode_reply: ZCodeReply) -> Self {
        let minimax = MiniMaxRouterFixture::new(minimax_reply).await;
        let owner = ZCodeTransport::seed_accounts(minimax.db.secret_session())
            .await
            .expect("seal a separate synthetic ZCode owner in the same isolated vault");
        let zcode_token = owner.synthetic_token(0).to_owned();
        let zcode_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = zcode_calls.clone();
        let turns = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new().route(
            "/api/v1/zcode-plan/anthropic/v1/messages",
            axum::routing::post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let calls = calls.clone();
                let turns = turns.clone();
                async move {
                    calls.lock().unwrap().push((headers, body));
                    let turn = turns.fetch_add(1, Ordering::SeqCst);
                    if matches!(zcode_reply, ZCodeReply::Denied) {
                        return (StatusCode::FORBIDDEN, axum::Json(json!({"error": {
                            "type": "permission_error", "message": "synthetic access denied"
                        }}))).into_response();
                    }
                    let tool = matches!(zcode_reply, ZCodeReply::ToolThenText) && turn == 0;
                    let content = if tool {
                        json!([{"type":"tool_use", "id":"call_cross_owner", "name":"lookup", "input":{"city":"Tokyo"}}])
                    } else {
                        json!([{"type":"text", "text":"synthetic second-owner answer"}])
                    };
                    axum::Json(json!({
                        "id":format!("msg_cross_owner_{turn}"), "type":"message", "role":"assistant",
                        "model":"glm-synthetic", "content":content,
                        "stop_reason":if tool {"tool_use"} else {"end_turn"},
                        "usage":{"input_tokens":3 + turn,"output_tokens":5 + turn}
                    })).into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let zcode_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let zcode = ZCodeTransport::loopback(&owner, &origin).unwrap();
        // Keep the original two-provider queue and priority. Replace only the
        // second entry's owner binding; no shared router or owner is mocked.
        let mut second = Provider::with_id(
            "synthetic-b".into(),
            "Synthetic ZCode".into(),
            json!({"auth":{},"config":"model = \"glm-synthetic\"\n"}),
            None,
        );
        second.sort_index = Some(1);
        second.category = Some("custom".into());
        second.meta = Some(ProviderMeta {
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("zcode_start_plan".into()),
                account_id: Some(owner.account_id(0).to_owned()),
            }),
            api_format: Some("anthropic".into()),
            ..Default::default()
        });
        minimax.db.save_provider("codex", &second).unwrap();
        let mut config = minimax.db.get_proxy_config_for_app("codex").await.unwrap();
        config.circuit_failure_threshold = 100;
        minimax
            .db
            .update_proxy_config_for_app(config)
            .await
            .unwrap();
        Self {
            minimax,
            zcode,
            zcode_calls,
            zcode_token,
            zcode_server,
        }
    }

    async fn request(&self, body: Value) -> axum::response::Response {
        use tower::ServiceExt;
        let router = self
            .minimax
            .proxy
            .build_router()
            .layer(axum::Extension(self.minimax.transport.clone()))
            .layer(axum::Extension(self.zcode.clone()));
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
        .expect("bounded original router response admission")
        .unwrap()
    }

    async fn usage_rows(&self, expected: usize) -> Vec<(String, String, i64, i64)> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let rows = {
                    let conn = self.minimax.db.conn.lock().unwrap();
                    let mut stmt = conn.prepare(
                        "SELECT request_id, provider_id, input_tokens, output_tokens FROM proxy_request_logs \
                         WHERE input_tokens > 0 OR output_tokens > 0 ORDER BY rowid"
                    ).unwrap();
                    let rows = stmt
                        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                        .unwrap()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap();
                    rows
                };
                if rows.len() >= expected {
                    return rows;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("observed usage must reach the existing database")
    }
}

#[tokio::test]
#[serial_test::serial]
async fn cross_owner_priority_keeps_healthy_first_owner_and_private_identity() {
    let f = CrossOwnerFixture::new(UpstreamBehavior::Complete, ZCodeReply::Text).await;
    let response = f.request(json!({"model":"MiniMax-M2.7","input":"synthetic question","stream":false,"account_id":"synthetic-b"})).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(String::from_utf8(collect(response).await.to_vec())
        .unwrap()
        .contains(TEXT));
    let calls = f.minimax.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_wire(&calls[0], "synthetic-a", "cn");
    assert!(f.zcode_calls.lock().unwrap().is_empty());
    assert_eq!(f.zcode.attempts(), 0);
    f.minimax.assert_idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn cross_owner_tool_roundtrip_survives_preoutput_fallback_and_attributes_usage() {
    let f =
        CrossOwnerFixture::new(UpstreamBehavior::FirstRateLimited, ZCodeReply::ToolThenText).await;
    let mut body = json!({"model":"MiniMax-M2.7","stream":false,"input":[{"role":"user","content":"synthetic lookup"}],
        "tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}]});
    let response = f.request(body.clone()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let first: Value = serde_json::from_slice(&collect(response).await).unwrap();
    let call = first["output"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("real converter must expose upstream tool call");
    assert_eq!(call["call_id"], "call_cross_owner");
    assert_eq!(
        serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
        json!({"city":"Tokyo"})
    );
    assert_eq!(
        f.usage_rows(1).await,
        vec![(
            "session:codex:synthetic-b:resp_msg_cross_owner_0".into(),
            "synthetic-b".into(),
            3,
            5
        )]
    );
    body["input"].as_array_mut().unwrap().extend([
        call.clone(),
        json!({"type":"function_call_output","call_id":call["call_id"],"output":"synthetic tool result"}),
        json!({"role":"user","content":"synthetic followup"}),
    ]);
    // No AppHandle exists in this harness; explicitly keep the configured
    // first choice, rather than pretending a GUI persistence hook ran.
    f.minimax
        .db
        .set_current_provider("codex", "synthetic-a")
        .unwrap();
    let response = f.request(body).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(String::from_utf8(collect(response).await.to_vec())
        .unwrap()
        .contains("synthetic second-owner answer"));
    let calls = f.zcode_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(f.minimax.calls.lock().unwrap().len(), 2);
    for (headers, _) in &calls {
        assert_eq!(
            headers["authorization"],
            format!("Bearer {}", f.zcode_token)
        );
        assert!(!headers.contains_key("x-mavis-session-id"));
    }
    let first_owner_calls = f.minimax.calls.lock().unwrap().clone();
    for turn in 0..2 {
        assert_eq!(
            first_owner_calls[turn].body["messages"], calls[turn].1["messages"],
            "fallback must not mutate the same turn's history"
        );
    }
    let messages = calls[1].1["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "user"]
    );
    assert_eq!(messages[0]["content"].as_array().unwrap().len(), 1);
    assert_eq!(messages[0]["content"][0]["text"], "synthetic lookup");
    assert_eq!(messages[1]["content"].as_array().unwrap().len(), 1);
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["content"][0]["id"], "call_cross_owner");
    assert_eq!(messages[1]["content"][0]["name"], "lookup");
    assert_eq!(messages[1]["content"][0]["input"], json!({"city":"Tokyo"}));
    assert_eq!(messages[2]["content"].as_array().unwrap().len(), 2);
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "call_cross_owner");
    assert_eq!(
        messages[2]["content"][0]["content"],
        "synthetic tool result"
    );
    assert_eq!(messages[2]["content"][1]["type"], "text");
    assert_eq!(messages[2]["content"][1]["text"], "synthetic followup");
    let blocks: Vec<&Value> = calls[1].1["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap())
        .collect();
    assert_eq!(
        blocks
            .iter()
            .filter(|b| b["type"] == "tool_use" && b["id"] == "call_cross_owner")
            .count(),
        1
    );
    assert_eq!(
        blocks
            .iter()
            .filter(|b| b["type"] == "tool_result"
                && b["tool_use_id"] == "call_cross_owner"
                && b["content"] == "synthetic tool result")
            .count(),
        1
    );
    assert_eq!(
        f.usage_rows(2).await,
        vec![
            (
                "session:codex:synthetic-b:resp_msg_cross_owner_0".into(),
                "synthetic-b".into(),
                3,
                5
            ),
            (
                "session:codex:synthetic-b:resp_msg_cross_owner_1".into(),
                "synthetic-b".into(),
                4,
                6
            ),
        ]
    );
    f.minimax.assert_idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn cross_owner_explicit_denial_never_falls_back_to_other_owner() {
    let f = CrossOwnerFixture::new(UpstreamBehavior::Complete, ZCodeReply::Denied).await;
    f.minimax
        .db
        .set_current_provider("codex", "synthetic-b")
        .unwrap();
    let response = f
        .request(json!({"model":"glm-synthetic","input":"synthetic denied request","stream":false}))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = serde_json::from_slice(&collect(response).await).unwrap();
    assert_eq!(body["error"]["type"], "access_denied");
    assert_eq!(f.zcode_calls.lock().unwrap().len(), 1);
    assert!(f.minimax.calls.lock().unwrap().is_empty());
    f.minimax.assert_idle().await;
}

#[tokio::test]
#[serial_test::serial]
async fn cross_owner_midstream_failure_is_terminal_not_cross_owner_replay() {
    let f = CrossOwnerFixture::new(UpstreamBehavior::FailAfterOutput, ZCodeReply::Text).await;
    let response = f
        .request(json!({"model":"MiniMax-M2.7","input":"synthetic question","stream":true}))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut emitted = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !sse_data(&emitted)
            .iter()
            .any(|e| e["type"] == "response.output_text.delta" && e["delta"] == TEXT)
        {
            emitted.extend_from_slice(
                &stream
                    .next()
                    .await
                    .expect("visible output before failure")
                    .unwrap(),
            );
        }
    })
    .await
    .unwrap();
    f.minimax.release_error.notify_one();
    tokio::time::timeout(Duration::from_secs(3), async {
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
    assert_eq!(f.minimax.calls.lock().unwrap().len(), 1);
    assert!(f.zcode_calls.lock().unwrap().is_empty());
    assert_eq!(f.zcode.attempts(), 0);
    assert!(!String::from_utf8(emitted)
        .unwrap()
        .contains("synthetic-a-access"));
    f.minimax.assert_idle().await;
}
