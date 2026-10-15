//! Actual messages ProxyServer -> opted-in ACP transport -> HTTP/SSE mock.
use super::*;
use crate::proxy::providers::workbuddy_acp::tests::{request, server};
use futures::StreamExt;
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
