//! Actual messages ProxyServer -> opted-in ACP transport -> HTTP/SSE mock.
use super::*;
use crate::proxy::providers::workbuddy_acp::tests::{request, server};

#[tokio::test]
#[serial]
async fn workbuddy_acp_text_free_only_rejects_before_any_outbound_request() {
    let mut failures = Vec::new();
    for intent in [
        "configured",
        "openrouter/free",
        "glm-5.3-flash-free",
        "sticky",
    ] {
        for stream in [false, true] {
            let fx = E2eFixture::with_minimax_keys(true).await;
            let (mut row, state, task) = server("end_turn").await;
            row.id = if intent == "sticky" {
                fx.expensive_id.clone()
            } else {
                fx.cheap_id.clone()
            };
            let mut body = request(stream);
            match intent {
                "configured" => row.settings_config["freeOnly"] = json!(true),
                "sticky" => {
                    let mut first = fx
                        .db
                        .get_provider_by_id(&fx.cheap_id, "claude")
                        .unwrap()
                        .unwrap();
                    // Fail configuration before request preparation; its free intent
                    // must survive when the next candidate is an ACP account.
                    first.settings_config = json!({"freeOnly": true});
                    fx.db.save_provider("claude", &first).unwrap();
                }
                model => {
                    body["model"] = json!(model);
                    row.settings_config["workbuddy_acp"]["declared_model"] = json!(model);
                }
            }
            fx.db.save_provider("claude", &row).unwrap();
            let response = reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
                .header("x-api-key", "synthetic-client-key")
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            let connections = state.connections.load(Ordering::SeqCst);
            let methods = state.methods.lock().await.clone();
            println!("intent={intent} stream={stream} status={status} ACP_connections={connections} RPC={methods:?} cheap={} expensive={}", fx.cheap.hits(), fx.expensive.hits());
            if status != StatusCode::BAD_REQUEST
                || !text.contains("Free-only route unavailable")
                || connections != 0
                || !methods.is_empty()
                || fx.cheap.hits() != 0
                || fx.expensive.hits() != 0
            {
                failures.push(format!("{intent}/{stream}: status={status}, connections={connections}, RPC={methods:?}"));
            }
            fx.server.stop().await.unwrap();
            task.abort();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

// Public ACP transport is paused regardless of the synthetic remote behavior.
// Low-level refusal/load/disconnect assertions remain in private ACP protocol tests.
async fn paused_acp_consumer(stop: &'static str, stream: bool, automatic: bool) {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let (mut row, state, task) = server(stop).await;
    row.id = fx.cheap_id.clone();
    fx.db.save_provider("claude", &row).unwrap();
    let mut config = fx.db.get_proxy_config_for_app("claude").await.unwrap();
    config.auto_failover_enabled = automatic;
    fx.db.update_proxy_config_for_app(config).await.unwrap();
    let before = serde_json::to_value(fx.db.get_all_providers("claude").unwrap()).unwrap();
    if stream {
        fx.expensive.set_stream(true).await;
    }
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
        .header("x-api-key", "synthetic-client-key")
        .json(&request(stream))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    if automatic {
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(text.contains("served-by-expensive"));
        assert_eq!(fx.expensive.hits(), 1);
    } else {
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("workbuddy.inference_paused"));
        assert_eq!(fx.expensive.hits(), 0);
    }
    assert_eq!(state.connections.load(Ordering::SeqCst), 0);
    assert!(state.methods.lock().await.is_empty());
    assert_eq!(fx.cheap.hits(), 0);
    assert_eq!(
        serde_json::to_value(fx.db.get_all_providers("claude").unwrap()).unwrap(),
        before
    );
    fx.server.stop().await.unwrap();
    task.abort();
}
#[tokio::test]
#[serial]
async fn workbuddy_acp_manual_nonstream_is_paused_without_fallback() {
    paused_acp_consumer("end_turn", false, false).await;
}
#[tokio::test]
#[serial]
async fn workbuddy_acp_manual_stream_is_paused_without_fallback() {
    paused_acp_consumer("end_turn", true, false).await;
}
#[tokio::test]
#[serial]
async fn workbuddy_acp_manual_never_opens_refusal_session() {
    paused_acp_consumer("refusal", true, false).await;
}
#[tokio::test]
#[serial]
async fn workbuddy_acp_manual_never_opens_disconnect_session() {
    paused_acp_consumer("disconnect", true, false).await;
}
#[tokio::test]
#[serial]
async fn workbuddy_acp_automatic_skips_only_to_existing_authorized_member() {
    paused_acp_consumer("end_turn", true, true).await;
}
