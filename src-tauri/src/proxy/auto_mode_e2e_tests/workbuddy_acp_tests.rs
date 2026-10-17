//! Actual messages ProxyServer -> opted-in ACP transport -> HTTP/SSE mock.
use super::*;
use crate::proxy::providers::workbuddy_acp::tests::{request, server};
use futures::StreamExt;

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

#[tokio::test]
#[serial]
async fn workbuddy_acp_text_proxy_server_nonstream_delivers_text_without_fallback() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let (mut row, state, task) = server("end_turn").await;
    row.id = fx.cheap_id.clone();
    fx.db.save_provider("claude", &row).unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
        .header("x-api-key", "synthetic-client-key")
        .json(&request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["content"][0]["text"], "hello");
    assert_eq!(fx.expensive.hits(), 0);
    assert_eq!(state.connections.load(Ordering::SeqCst), 1);
    assert_eq!(state.methods.lock().await.len(), 3);
    fx.server.stop().await.unwrap();
    task.abort();
}

#[tokio::test]
#[serial]
async fn workbuddy_acp_text_proxy_server_delivers_text_without_fallback() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let (mut row, state, task) = server("end_turn").await;
    row.id = fx.cheap_id.clone();
    fx.db.save_provider("claude", &row).unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
        .header("x-api-key", "synthetic-client-key")
        .json(&request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let text = response.text().await.unwrap();
    assert!(text.contains("hello"), "{text}");
    assert!(!text.contains("OLD HISTORY"));
    assert_eq!(text.matches("event: message_stop").count(), 1);
    assert_eq!(fx.expensive.hits(), 0);
    assert_eq!(state.methods.lock().await.len(), 3);
    fx.server.stop().await.unwrap();
    task.abort();
}
#[tokio::test]
#[serial]
async fn workbuddy_acp_text_proxy_server_refusal_never_rotates_account() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let (mut row, _, task) = server("refusal").await;
    row.id = fx.cheap_id.clone();
    fx.db.save_provider("claude", &row).unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
        .header("x-api-key", "synthetic-client-key")
        .json(&request(true))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
            Err(_) => break,
        }
    }
    assert!(!text.contains("event: message_stop"));
    assert_eq!(fx.expensive.hits(), 0);
    fx.server.stop().await.unwrap();
    task.abort();
}

#[tokio::test]
#[serial]
async fn workbuddy_acp_text_proxy_server_disconnect_never_rotates_account() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let (mut row, _, task) = server("disconnect").await;
    row.id = fx.cheap_id.clone();
    fx.db.save_provider("claude", &row).unwrap();
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
        .header("x-api-key", "synthetic-client-key")
        .json(&request(true))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => text.push_str(&String::from_utf8_lossy(&bytes)),
            Err(_) => break,
        }
    }
    assert!(
        text.contains("hello"),
        "must see partial output before disconnect: {text}"
    );
    assert!(!text.contains("event: message_stop"));
    assert_eq!(fx.expensive.hits(), 0);
    fx.server.stop().await.unwrap();
    task.abort();
}
