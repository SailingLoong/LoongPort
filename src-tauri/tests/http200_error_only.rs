//! Real handler -> forwarder -> synthetic loopback Chat upstream regressions.
//! No real accounts, credentials, external requests, or alternate retry owner.
use axum::response::IntoResponse;
use cc_switch_lib::Provider;
use futures::StreamExt;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
#[path = "support.rs"]
mod support;

async fn upstream(
    body: Value,
) -> (
    String,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
    Option<tokio::sync::oneshot::Sender<()>>,
) {
    let (release, wait) = tokio::sync::oneshot::channel();
    let release = body.is_string().then_some(release);
    let gate = Arc::new(tokio::sync::Mutex::new(Some(wait)));
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move || {
            let (counter, body, gate) = (counter.clone(), body.clone(), gate.clone());
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                if let Some(sse) = body.as_str() {
                    let split = sse.find("\n\n").unwrap() + 2;
                    let first = sse[..split].to_owned();
                    let rest = sse[split..].to_owned();
                    let wait = gate.lock().await.take().expect("one stream request");
                    let chunks =
                        futures::stream::once(
                            async move { Ok::<_, std::convert::Infallible>(first) },
                        )
                        .chain(futures::stream::once(async move {
                            wait.await
                                .expect("client observed first chunk before error");
                            Ok::<_, std::convert::Infallible>(rest)
                        }));
                    axum::response::Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from_stream(chunks))
                        .unwrap()
                } else {
                    axum::Json(body).into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, hits, task, release)
}

async fn check(body: Value, automatic: bool, expected_status: u16, next_hits: usize, claude: bool) {
    check_with_retries(body, automatic, expected_status, next_hits, claude, 3).await;
}

async fn check_with_retries(
    body: Value,
    automatic: bool,
    expected_status: u16,
    next_hits: usize,
    claude: bool,
    max_retries: u32,
) {
    let streaming = body.is_string();
    support::reset_test_fs();
    let state = support::create_test_state().unwrap();
    let second_body = json!({"id":"fixture-second","object":"chat.completion","model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":"second-provider"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}});
    let (first, first_hits, first_task, release_error) = upstream(body.clone()).await;
    let (second, second_hits, second_task, _) = upstream(second_body).await;
    let app = if claude { "claude" } else { "codex" };
    for (id, url) in [("first", first), ("second", second)] {
        let config = if claude {
            json!({"api_format":"openai_chat","env":{"ANTHROPIC_BASE_URL":url,"ANTHROPIC_AUTH_TOKEN":"synthetic-fixture-key"}})
        } else {
            json!({"config":format!("base_url = '{url}'"),"auth":{"OPENAI_API_KEY":"synthetic-fixture-key"}})
        };
        state
            .db
            .save_provider(app, &Provider::with_id(id.into(), id.into(), config, None))
            .unwrap();
    }
    state.db.set_current_provider(app, "first").unwrap();
    // Seed the original persisted applied chain; the real router consumes it.
    state
        .db
        .set_setting(
            &format!("application_priority_{app}"),
            r#"["first","second"]"#,
        )
        .unwrap();
    let mut app_config = state.db.get_proxy_config_for_app(app).await.unwrap();
    app_config.enabled = true;
    app_config.auto_failover_enabled = automatic;
    app_config.max_retries = max_retries;
    state
        .db
        .update_proxy_config_for_app(app_config)
        .await
        .unwrap();
    let mut config = state.db.get_proxy_config().await.unwrap();
    config.listen_address = "127.0.0.1".into();
    config.listen_port = 0;
    config.enable_logging = false;
    config.non_streaming_timeout = 3;
    config.max_retries = u8::try_from(max_retries).unwrap();
    state.db.update_proxy_config(config).await.unwrap();
    let info = state.proxy_service.start().await.unwrap();
    assert_eq!(
        state
            .db
            .get_proxy_config_for_app(app)
            .await
            .unwrap()
            .max_retries,
        max_retries,
        "fixture must preserve requested retry budget"
    );
    let endpoint = if claude {
        "/v1/messages"
    } else {
        "/v1/chat/completions"
    };
    let result = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(10)).build().unwrap()
        .post(format!("http://127.0.0.1:{}{endpoint}", info.port))
        .json(&json!({"model":"fixture-model","max_tokens":20,"stream":streaming,"messages":[{"role":"user","content":"fixture"}]}))
        .send().await;
    let mut observed_first = None;
    let result = match result {
        Ok(mut response) => {
            let status = response.status().as_u16();
            if let Some(release) = release_error {
                match response.chunk().await {
                    Ok(Some(first)) => {
                        let first = String::from_utf8(first.to_vec()).unwrap();
                        observed_first = Some(first.clone());
                        release
                            .send(())
                            .expect("upstream still awaits client observation");
                        response.text().await.map(|tail| (status, first + &tail))
                    }
                    Ok(None) => Ok((status, String::new())),
                    Err(error) => Err(error),
                }
            } else {
                response.text().await.map(|text| (status, text))
            }
        }
        Err(error) => Err(error),
    };
    let stats = state.proxy_service.get_status().await.unwrap();
    state.proxy_service.stop().await.unwrap();
    first_task.abort();
    second_task.abort();
    let _ = first_task.await;
    let _ = second_task.await;
    let (status, text) = result.expect("real proxy response");
    assert_eq!(
        first_hits.load(Ordering::SeqCst),
        1,
        "first provider is never replayed"
    );
    assert_eq!(
        second_hits.load(Ordering::SeqCst),
        next_hits,
        "only authorized fallback may send"
    );
    assert_eq!(status, expected_status, "{text}");
    assert_eq!(
        stats.success_requests,
        u64::from(expected_status == 200),
        "error-only cannot count as success"
    );
    if next_hits == 1 {
        assert!(text.contains("second-provider"));
    }
    if expected_status == 200 && next_hits == 0 && !claude && !streaming {
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            body,
            "valid/unknown success bytes retain semantics"
        );
    }
    if streaming {
        let first = observed_first.expect("client received first output before releasing error");
        assert!(first.contains("already-produced"));
        assert!(!first.contains("rate_limit_error"));
        assert_eq!(
            text,
            body.as_str().unwrap(),
            "stream is not replayed or reclassified"
        );
    }
}

macro_rules! case {
    ($name:ident, $body:expr, $auto:expr, $status:expr, $next:expr, $claude:expr) => {
        #[allow(clippy::await_holding_lock)]
        #[tokio::test]
        async fn $name() {
            let _guard = support::test_mutex()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            check($body, $auto, $status, $next, $claude).await;
        }
    };
}
case!(
    automatic_rate_limit_follows_existing_order,
    json!({"error":{"type":"rate_limit_error","message":"synthetic TPM limit"}}),
    true,
    200,
    1,
    false
);
case!(
    claude_chat_error_is_classified_before_transform,
    json!({"error":{"type":"rate_limit_error","message":"synthetic TPM limit"}}),
    true,
    200,
    1,
    true
);
case!(
    manual_rate_limit_does_not_switch,
    json!({"error":{"type":"rate_limit_error"}}),
    false,
    429,
    0,
    false
);
case!(
    manual_quota_is_not_short_rate_limit,
    json!({"error":{"code":"insufficient_quota"}}),
    false,
    402,
    0,
    false
);
case!(
    unknown_error_is_not_assumed_quota,
    json!({"error":{"type":"unknown_fixture","message":"balance limit words are not a contract"}}),
    false,
    502,
    0,
    false
);
case!(
    permission_denial_never_switches,
    json!({"error":{"type":"permission_error"}}),
    true,
    403,
    0,
    false
);
case!(
    safety_denial_overrides_rate_limit,
    json!({"error":{"type":"rate_limit_error","code":"content_policy_violation"}}),
    true,
    403,
    0,
    false
);
case!(
    valid_content_with_error_is_not_replayed,
    json!({"error":{"type":"rate_limit_error"},"choices":[{"message":{"role":"assistant","content":"already-produced"}}]}),
    true,
    200,
    0,
    false
);
case!(
    tool_call_is_not_replayed,
    json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"tool_fixture","type":"function","function":{"name":"fixture","arguments":"{}"}}]}}]}),
    true,
    200,
    0,
    false
);
case!(
    empty_delta_is_not_error,
    json!({"choices":[{"delta":{}}]}),
    true,
    200,
    0,
    false
);
case!(
    usage_only_is_not_error,
    json!({"choices":[],"usage":{"completion_tokens":0}}),
    true,
    200,
    0,
    false
);
case!(
    normal_empty_reply_is_not_error,
    json!({"choices":[{"message":{"role":"assistant","content":""},"finish_reason":"stop"}]}),
    true,
    200,
    0,
    false
);
case!(
    empty_choices_with_error_is_not_guessed,
    json!({"error":{"type":"rate_limit_error"},"choices":[]}),
    true,
    200,
    0,
    false
);
case!(
    unknown_success_shape_is_not_error,
    json!({"message":"balance limit","data":[]}),
    true,
    200,
    0,
    false
);

case!(stream_error_after_client_observes_output_never_replays, json!("data: {\"choices\":[{\"delta\":{\"content\":\"already-produced\"}}]}\n\ndata: {\"error\":{\"type\":\"rate_limit_error\"}}\n\ndata: [DONE]\n\n"), true, 200, 0, false);
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn retry_budget_zero_does_not_switch() {
    let _guard = support::test_mutex()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    check_with_retries(
        json!({"error":{"type":"rate_limit_error"}}),
        true,
        429,
        0,
        false,
        0,
    )
    .await;
}
