//! Two synthetic API keys through the existing messages server/router/streaming stack.
//! This validates the MiniMax Anthropic preset shape against loopback fixtures,
//! not live MiniMax, account claiming, WorkBuddy or Qoder inference compatibility.
use super::*;
use futures::StreamExt;
use std::time::Duration;

async fn cooldown_snapshot(
    fx: &E2eFixture,
    provider_id: &str,
) -> Option<super::super::circuit_breaker::BreakerSnapshot> {
    let provider = fx
        .db
        .get_provider_by_id(provider_id, "claude")
        .unwrap()
        .unwrap();
    fx.server
        .provider_breaker_states("claude", &[provider])
        .await
        .remove(provider_id)
}

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

#[tokio::test]
#[serial]
async fn multiround_quota_exhaustion_skips_last_account_until_first_recovers() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(503).await;
    fx.expensive.set_status(429).await;
    *fx.expensive.state.response_override.write().await = Some(
        json!({"type":"error","error":{"type":"insufficient_quota","message":"Synthetic account quota exhausted"}}),
    );

    for round in 0..2 {
        let response = send_model_message(fx.port, "synthetic-exhaustion", "MiniMax-M2.7").await;
        assert!(!response.status().is_success(), "round {round}");
        if round == 0 {
            let cooldown = cooldown_snapshot(&fx, &fx.expensive_id)
                .await
                .expect("last account must enter cooldown");
            assert!(!cooldown.half_open);
            assert!(cooldown
                .reopen_in_secs
                .is_some_and(|seconds| seconds >= 1700));
        }
    }
    assert_eq!(
        fx.cheap.hits(),
        2,
        "short transient cooldown allows a new first-tier probe"
    );
    assert_eq!(
        fx.expensive.hits(),
        1,
        "exhausted last account must not be called again during fatal cooldown"
    );
    fx.cheap.set_status(200).await;
    let response = send_model_message(fx.port, "synthetic-exhaustion", "MiniMax-M2.7").await;
    assert_eq!(response_text(response).await, "served-by-cheap");
    assert_eq!(fx.cheap.hits(), 3);
    assert_eq!(fx.expensive.hits(), 1);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn request_error_does_not_cool_account_on_next_request() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let mut config = fx.db.get_proxy_config_for_app("claude").await.unwrap();
    config.circuit_timeout_seconds = 60;
    fx.db.update_proxy_config_for_app(config).await.unwrap();
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(400).await;
    *fx.cheap.state.response_override.write().await = Some(
        json!({"type":"error","error":{"type":"invalid_request_error","message":"Synthetic invalid request"}}),
    );
    let response = send_model_message(fx.port, "synthetic-bad-request", "MiniMax-M2.7").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fx.cheap.hits(), 1);
    assert_eq!(fx.expensive.hits(), 0);
    fx.cheap.set_status(200).await;
    *fx.cheap.state.response_override.write().await = None;
    assert_eq!(
        response_text(send_model_message(fx.port, "synthetic-next-request", "MiniMax-M2.7").await)
            .await,
        "served-by-cheap"
    );
    assert_eq!(fx.cheap.hits(), 2);
    assert_eq!(fx.expensive.hits(), 0);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn retry_budget_zero_stops_after_one_real_upstream_attempt() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let mut config = fx.db.get_proxy_config_for_app("claude").await.unwrap();
    config.max_retries = 0;
    fx.db.update_proxy_config_for_app(config).await.unwrap();
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(503).await;
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        send_model_message(fx.port, "synthetic-retry-budget", "MiniMax-M2.7"),
    )
    .await
    .expect("request must finish within the bounded fixture deadline");
    assert!(!response.status().is_success());
    assert_eq!(fx.cheap.hits(), 1);
    assert_eq!(
        fx.expensive.hits(),
        0,
        "zero retries must not send to the next account"
    );
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn mixed_quota_and_safety_denial_never_rotates_accounts() {
    for error in [
        json!({"type":"insufficient_quota","code":"safety_error"}),
        json!({"type":"safety_error","code":"insufficient_quota"}),
    ] {
        let fx = E2eFixture::with_minimax_keys(true).await;
        let mut config = fx.db.get_proxy_config_for_app("claude").await.unwrap();
        config.circuit_timeout_seconds = 60;
        fx.db.update_proxy_config_for_app(config).await.unwrap();
        fx.set_current(&fx.cheap_id);
        fx.cheap.set_status(429).await;
        *fx.cheap.state.response_override.write().await =
            Some(json!({"type":"error","error":error}));
        let response = send_model_message(fx.port, "synthetic-mixed-denial", "MiniMax-M2.7").await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            cooldown_snapshot(&fx, &fx.cheap_id).await.is_none(),
            "explicit denial must not open cooldown"
        );
        assert_eq!(fx.cheap.hits(), 1);
        assert_eq!(fx.expensive.hits(), 0);
        fx.cheap.set_status(200).await;
        *fx.cheap.state.response_override.write().await = None;
        assert_eq!(
            response_text(
                send_model_message(fx.port, "synthetic-after-denial", "MiniMax-M2.7").await
            )
            .await,
            "served-by-cheap"
        );
        assert_eq!(
            fx.cheap.hits(),
            2,
            "explicit denial must not cool the account"
        );
        assert_eq!(fx.expensive.hits(), 0);
        fx.server.stop().await.unwrap();
    }
}

// Retain the existing manual, single-provider and transient bypass contracts.
// An automatic multi-candidate fatal current is skipped after the first failure.
async fn observe_current_cooldown_bypass(automatic: bool, single: bool, category: &str) {
    let fx = E2eFixture::with_minimax_keys(true).await;
    let mut config = fx.db.get_proxy_config_for_app("claude").await.unwrap();
    config.auto_failover_enabled = automatic;
    config.circuit_failure_threshold = 1;
    config.circuit_timeout_seconds = 60;
    fx.db.update_proxy_config_for_app(config).await.unwrap();
    fx.set_current(&fx.cheap_id);
    if single {
        fx.db.delete_provider("claude", &fx.expensive_id).unwrap();
    }
    fx.cheap.set_status(429).await;
    *fx.cheap.state.response_override.write().await = Some(json!({
        "type": "error", "error": {"type": category, "message": "synthetic failure"}
    }));
    for round in 0..2 {
        let response =
            send_model_message(fx.port, "synthetic-current-cooldown", "MiniMax-M2.7").await;
        if automatic && !single {
            assert_eq!(response_text(response).await, "served-by-expensive");
        } else {
            assert!(!response.status().is_success());
        }
        if round == 0 {
            let cooldown = cooldown_snapshot(&fx, &fx.cheap_id)
                .await
                .expect("first error must open cooldown");
            assert!(!cooldown.half_open);
            let seconds = cooldown
                .reopen_in_secs
                .expect("open cooldown must have a duration");
            if category == "insufficient_quota" {
                assert!(seconds >= 1700, "fatal quota cooldown: {seconds}");
            } else {
                assert!(
                    (50..=60).contains(&seconds),
                    "ordinary rate cooldown: {seconds}"
                );
            }
        }
    }
    let expected_current_hits = if automatic && !single && category == "insufficient_quota" {
        1
    } else {
        2
    };
    assert_eq!(
        fx.cheap.hits(),
        expected_current_hits,
        "current cooldown behavior must match mode and failure class"
    );
    assert_eq!(
        fx.expensive.hits(),
        if automatic && !single { 2 } else { 0 }
    );
    // This fixture has no AppHandle; unlike a live application it does not persist fallback.
    assert_eq!(
        fx.db.get_current_provider("claude").unwrap().as_deref(),
        Some(fx.cheap_id.as_str())
    );
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn auto_current_fatal_cooldown_skips_after_first_attempt() {
    observe_current_cooldown_bypass(true, false, "insufficient_quota").await;
}

#[tokio::test]
#[serial]
async fn characterize_auto_current_rate_limit_bypass() {
    observe_current_cooldown_bypass(true, false, "rate_limit_error").await;
}

#[tokio::test]
#[serial]
async fn characterize_auto_single_quota_bypass() {
    observe_current_cooldown_bypass(true, true, "insufficient_quota").await;
}

#[tokio::test]
#[serial]
async fn characterize_manual_current_quota_bypass() {
    observe_current_cooldown_bypass(false, false, "insufficient_quota").await;
}

#[tokio::test]
#[serial]
async fn red_auto_current_fatal_cooldown_skips_to_existing_next_candidate() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(429).await;
    *fx.cheap.state.response_override.write().await = Some(json!({
        "type":"error", "error":{"type":"insufficient_quota","message":"synthetic quota"}
    }));
    assert_eq!(
        response_text(send_model_message(fx.port, "synthetic-current-fatal", "MiniMax-M2.7").await)
            .await,
        "served-by-expensive"
    );
    assert!(
        cooldown_snapshot(&fx, &fx.cheap_id)
            .await
            .unwrap()
            .reopen_in_secs
            .unwrap()
            >= 1700
    );
    assert_eq!(
        response_text(send_model_message(fx.port, "synthetic-current-fatal", "MiniMax-M2.7").await)
            .await,
        "served-by-expensive"
    );
    assert_eq!(
        fx.cheap.hits(),
        1,
        "automatic multi-candidate dispatch must not retry current fatal-open provider"
    );
    assert_eq!(fx.expensive.hits(), 2);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn auto_current_transient_503_still_probes_once_per_request() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(503).await;
    for _ in 0..2 {
        let response = send_model_message(fx.port, "synthetic-current-503", "MiniMax-M2.7").await;
        assert_eq!(response_text(response).await, "served-by-expensive");
    }
    assert_eq!(
        fx.cheap.hits(),
        2,
        "transient current behavior remains unchanged"
    );
    assert_eq!(fx.expensive.hits(), 2);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn auto_current_402_fatal_cooldown_skips_after_first_attempt() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(402).await;
    for _ in 0..2 {
        let response = send_model_message(fx.port, "synthetic-current-402", "MiniMax-M2.7").await;
        assert_eq!(response_text(response).await, "served-by-expensive");
    }
    assert_eq!(
        fx.cheap.hits(),
        1,
        "existing 402 fatal classification must also respect cooldown"
    );
    assert_eq!(fx.expensive.hits(), 2);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn red_auto_current_fatal_cooldown_blocks_concurrent_retries() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(429).await;
    *fx.cheap.state.response_override.write().await = Some(json!({
        "type":"error", "error":{"type":"insufficient_quota","message":"synthetic quota"}
    }));
    assert_eq!(
        response_text(
            send_model_message(fx.port, "synthetic-concurrent-first", "MiniMax-M2.7").await
        )
        .await,
        "served-by-expensive"
    );
    assert!(
        cooldown_snapshot(&fx, &fx.cheap_id)
            .await
            .unwrap()
            .reopen_in_secs
            .unwrap()
            >= 1700
    );
    let (a, b) = tokio::join!(
        send_model_message(fx.port, "synthetic-concurrent-a", "MiniMax-M2.7"),
        send_model_message(fx.port, "synthetic-concurrent-b", "MiniMax-M2.7"),
    );
    assert_eq!(response_text(a).await, "served-by-expensive");
    assert_eq!(response_text(b).await, "served-by-expensive");
    assert_eq!(
        fx.cheap.hits(),
        1,
        "concurrent automatic dispatches must both skip fatal-open current"
    );
    assert_eq!(fx.expensive.hits(), 3);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
#[serial]
async fn red_auto_shared_account_fatal_cooldown_skips_current_and_sibling() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    for id in [&fx.cheap_id, &fx.expensive_id] {
        let mut provider = fx.db.get_provider_by_id(id, "claude").unwrap().unwrap();
        provider.website_url = Some("https://shared.example".into());
        provider
            .meta
            .get_or_insert_with(Default::default)
            .loongport_account_id = Some(12345);
        fx.db.save_provider("claude", &provider).unwrap();
    }
    fx.set_current(&fx.cheap_id);
    fx.cheap.set_status(429).await;
    *fx.cheap.state.response_override.write().await = Some(json!({
        "type":"error", "error":{"type":"insufficient_quota","message":"synthetic quota"}
    }));
    assert!(
        !send_model_message(fx.port, "synthetic-shared-fatal", "MiniMax-M2.7")
            .await
            .status()
            .is_success()
    );
    assert_eq!(fx.cheap.hits(), 1);
    assert_eq!(
        fx.expensive.hits(),
        0,
        "same account sibling must be skipped"
    );
    assert!(
        cooldown_snapshot(&fx, &fx.expensive_id)
            .await
            .unwrap()
            .reopen_in_secs
            .unwrap()
            >= 1700
    );
    assert!(
        !send_model_message(fx.port, "synthetic-shared-fatal", "MiniMax-M2.7")
            .await
            .status()
            .is_success()
    );
    assert_eq!(
        fx.cheap.hits(),
        1,
        "current provider shares fatal-open account and must not be retried"
    );
    assert_eq!(fx.expensive.hits(), 0);
    fx.server.stop().await.unwrap();
}

#[tokio::test]
async fn half_open_permit_released_neutrally_allows_next_probe() {
    use crate::proxy::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    let breaker = CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 1,
        timeout_seconds: 0,
        success_threshold: 2,
        ..Default::default()
    });
    breaker.record_failure(false).await;
    let first = breaker.allow_request().await;
    assert!(first.allowed && first.used_half_open_permit);
    let concurrent = breaker.allow_request().await;
    assert!(!concurrent.allowed);
    breaker.release_half_open_permit();
    let second = breaker.allow_request().await;
    assert!(second.allowed && second.used_half_open_permit);
    breaker.record_success(true).await;
    let third = breaker.allow_request().await;
    assert!(
        third.allowed && third.used_half_open_permit,
        "success must release the half-open permit"
    );
    breaker.release_half_open_permit();
}

#[tokio::test]
#[serial]
async fn red_auto_current_sibling_respects_shared_account_fatal_cooldown() {
    let fx = E2eFixture::with_minimax_keys(true).await;
    for id in [&fx.cheap_id, &fx.expensive_id] {
        let mut provider = fx.db.get_provider_by_id(id, "claude").unwrap().unwrap();
        provider.website_url = Some("https://shared.example".into());
        provider
            .meta
            .get_or_insert_with(Default::default)
            .loongport_account_id = Some(12345);
        fx.db.save_provider("claude", &provider).unwrap();
    }
    let third = MockUpstream::spawn("served-by-independent").await;
    let third_id = "synthetic-independent";
    let provider = Provider::with_id(
        third_id.into(),
        "Independent".into(),
        json!({"env":{"ANTHROPIC_BASE_URL":format!("{}/anthropic", third.base_url()), "ANTHROPIC_AUTH_TOKEN":"synthetic-third", "ANTHROPIC_MODEL":"MiniMax-M2.7"}}),
        None,
    );
    fx.db.save_provider("claude", &provider).unwrap();
    super::super::application_routing::set_order(
        &fx.db,
        "claude",
        &[
            fx.cheap_id.clone(),
            fx.expensive_id.clone(),
            third_id.into(),
        ],
    )
    .unwrap();
    fx.set_current(&fx.cheap_id);
    for upstream in [&fx.cheap, &fx.expensive] {
        upstream.set_status(429).await;
        *upstream.state.response_override.write().await = Some(
            json!({"type":"error","error":{"type":"insufficient_quota","message":"synthetic quota"}}),
        );
    }
    assert_eq!(
        response_text(send_model_message(fx.port, "synthetic-shared-owner", "MiniMax-M2.7").await)
            .await,
        "served-by-independent"
    );
    assert_eq!(fx.cheap.hits(), 1);
    assert_eq!(fx.expensive.hits(), 0);
    assert!(
        cooldown_snapshot(&fx, &fx.expensive_id)
            .await
            .unwrap()
            .reopen_in_secs
            .unwrap()
            >= 1700
    );
    // Model a persisted current selection without claiming a user action or real client.
    fx.set_current(&fx.expensive_id);
    assert_eq!(
        response_text(
            send_model_message(fx.port, "synthetic-shared-owner-next", "MiniMax-M2.7").await
        )
        .await,
        "served-by-independent"
    );
    assert_eq!(
        fx.expensive.hits(),
        0,
        "tier has no own error but its original account owner is fatal-open"
    );
    assert_eq!(third.hits(), 2);
    fx.server.stop().await.unwrap();
}
