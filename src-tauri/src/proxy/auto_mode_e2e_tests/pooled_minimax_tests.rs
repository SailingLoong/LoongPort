//! Two synthetic API keys through the existing messages server/router/streaming stack.
//! This validates the MiniMax Anthropic preset shape against loopback fixtures,
//! not live MiniMax, account claiming, WorkBuddy or Qoder inference compatibility.
use super::*;
use futures::StreamExt;
use std::time::Duration;

async fn assert_last_owner(fx: &E2eFixture, expected: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let owner = {
                let conn = fx.db.conn.lock().unwrap();
                conn.query_row(
                    "SELECT provider_id FROM proxy_request_logs ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
            };
            if owner.as_deref() == Some(expected) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("request owner must reach existing usage store");
}

#[tokio::test]
#[serial]
async fn minimax_pool_priority_failure_uses_each_selected_key_and_owner() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let response = send_model_message(fx.port, "synthetic-pool-priority", "MiniMax-M2.7").await;
    assert_eq!(response_text(response).await, "served-by-cheap");
    assert_last_owner(&fx, &fx.cheap_id).await;
    assert_eq!(
        fx.cheap.auth_header().await.as_deref(),
        Some("Bearer tok-cheap")
    );
    assert_eq!(&*fx.cheap.state.uri.read().await, "/anthropic/v1/messages");
    assert_eq!(
        fx.cheap.state.model.read().await.as_deref(),
        Some("MiniMax-M2.7")
    );
    assert_eq!(
        fx.expensive.hits(),
        0,
        "priority must avoid the second key while first succeeds"
    );

    fx.cheap.set_status(402).await;
    let response = send_model_message(fx.port, "synthetic-pool-fallback", "MiniMax-M2.7").await;
    assert_eq!(response_text(response).await, "served-by-expensive");
    assert_last_owner(&fx, &fx.expensive_id).await;
    assert_eq!(fx.cheap.hits(), 2, "one first-key attempt per request");
    assert_eq!(fx.expensive.hits(), 1, "one forward fallback attempt");
    assert_eq!(
        fx.expensive.auth_header().await.as_deref(),
        Some("Bearer tok-expensive")
    );
    assert_eq!(
        &*fx.expensive.state.uri.read().await,
        "/anthropic/v1/messages"
    );
    assert_eq!(
        fx.expensive.state.model.read().await.as_deref(),
        Some("MiniMax-M2.7")
    );
    for upstream in [&fx.cheap, &fx.expensive] {
        let api_key = upstream.state.api_key_header.read().await.clone();
        assert!(
            api_key.is_none()
                || api_key.as_deref()
                    == Some(if std::ptr::eq(upstream, &fx.cheap) {
                        "tok-cheap"
                    } else {
                        "tok-expensive"
                    }),
            "client or another account key must not leak upstream"
        );
    }
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn minimax_pool_stream_interruption_after_output_never_replays_on_another_key() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    fx.cheap.set_stream(true).await;
    *fx.cheap.state.interrupt_stream.write().await = true;
    fx.expensive.set_stream(true).await;
    let response =
        send_streaming_model_message(fx.port, "synthetic-pool-interruption", "MiniMax-M2.7").await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => received.extend_from_slice(&bytes),
                Err(_) => break,
            }
        }
    })
    .await
    .expect("interrupted response must finish within a bounded interval");
    let output = String::from_utf8(received).unwrap();
    assert!(
        output.contains("served-by-cheap"),
        "must observe first key output before interruption: {output}"
    );
    assert!(
        !output.contains("message_stop"),
        "fixture must actually interrupt before completion"
    );
    assert!(!output.contains("served-by-expensive"));
    assert_eq!(
        fx.cheap.hits(),
        1,
        "partial output must not replay on the same key"
    );
    assert_eq!(
        fx.expensive.hits(),
        0,
        "partial output must not fail over to another key"
    );
    assert_eq!(
        fx.cheap.auth_header().await.as_deref(),
        Some("Bearer tok-cheap")
    );
    assert_eq!(
        fx.db.get_current_provider("claude").unwrap(),
        None,
        "stream failure must not activate a fallback"
    );
    fx.server.stop().await.unwrap();
}

// Acceptance red tests: permission/safety refusals must not rotate accounts.
// Authentication expiry and rate capacity remain distinct control cases.
async fn denial_boundary_case(status: u16, body: Value, fallback_allowed: bool) {
    denial_boundary_with_media_case(status, body, fallback_allowed, false).await;
}

async fn denial_boundary_with_media_case(
    status: u16,
    body: Value,
    fallback_allowed: bool,
    image: bool,
) {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let original_error = body.get("error").cloned();
    fx.cheap.set_status(status).await;
    *fx.cheap.state.response_override.write().await = Some(body);
    let response = if image {
        reqwest::Client::builder().no_proxy().build().unwrap()
            .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
            .header("x-api-key", "client-key-should-be-overridden")
            .header("anthropic-version", "2023-06-01")
            .json(&json!({"model":"synthetic-vision-model","max_tokens":16,"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"c3ludGhldGlj"}}]}]}))
            .send().await.unwrap()
    } else {
        send_model_message(fx.port, "synthetic-denial-boundary", "MiniMax-M2.7").await
    };
    let response_status = response.status().as_u16();
    let response_body = response.text().await.unwrap();
    let first_hits = fx.cheap.hits();
    let second_hits = fx.expensive.hits();
    fx.server.stop().await.unwrap();
    assert_eq!(first_hits, 1, "one initial account attempt");
    assert_eq!(
        second_hits,
        usize::from(fallback_allowed),
        "status {status}: permission/safety denial must not rotate accounts; body={response_body}"
    );
    if fallback_allowed {
        assert_eq!(response_status, 200);
        assert!(response_body.contains("served-by-expensive"));
    } else {
        assert_eq!(response_status, status);
        assert!(!response_body.contains("served-by-expensive"));
        if let Some(error) = original_error {
            let received: Value = serde_json::from_str(&response_body).unwrap();
            for field in ["type", "code"] {
                if let Some(expected) = error.get(field) {
                    assert_eq!(
                        received["error"][field], *expected,
                        "preserve original rejection field {field}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
#[serial]
async fn denial_boundary_403_permission_does_not_rotate_accounts() {
    denial_boundary_case(403, json!({"type":"error","error":{"type":"permission_error","message":"Synthetic access permission denied"}}), false).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_403_safety_does_not_rotate_accounts() {
    denial_boundary_case(403, json!({"type":"error","error":{"type":"safety_error","message":"Synthetic safety refusal"}}), false).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_400_safety_does_not_rotate_accounts() {
    denial_boundary_case(400, json!({"type":"error","error":{"type":"safety_error","message":"Synthetic safety refusal"}}), false).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_200_refusal_does_not_rotate_accounts() {
    denial_boundary_case(200, json!({"id":"msg_synthetic_refusal","type":"message","role":"assistant","model":"MiniMax-M2.7","content":[{"type":"text","text":"Synthetic refusal: cannot assist with that request"}],"stop_reason":"refusal","usage":{"input_tokens":1,"output_tokens":1}}), false).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_401_invalid_key_can_use_fallback() {
    denial_boundary_case(401, json!({"type":"error","error":{"type":"authentication_error","message":"Synthetic invalid API key"}}), true).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_429_capacity_can_use_fallback() {
    denial_boundary_case(429, json!({"type":"error","error":{"type":"rate_limit_error","message":"Synthetic capacity limit"}}), true).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_403_quota_can_use_fallback() {
    denial_boundary_case(403, json!({"type":"error","error":{"type":"insufficient_quota","message":"Synthetic quota exhausted"}}), true).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_403_technical_can_use_fallback() {
    denial_boundary_case(403, json!({"type":"error","error":{"type":"upstream_unavailable","message":"Synthetic upstream unavailable"}}), true).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_429_permission_code_overrides_capacity_type() {
    denial_boundary_case(429, json!({"type":"error","error":{"type":"rate_limit_error","code":"permission_denied","message":"Synthetic access refusal"}}), false).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_429_safety_does_not_rotate_accounts() {
    denial_boundary_case(429, json!({"type":"error","error":{"type":"safety_error","message":"Synthetic safety refusal"}}), false).await;
}

#[tokio::test]
#[serial]
async fn denial_boundary_safety_precedes_image_rectification() {
    denial_boundary_with_media_case(400, json!({"type":"error","error":{"type":"safety_error","message":"Synthetic safety refusal: model only supports text input; image unsupported"}}), false, true).await;
}
