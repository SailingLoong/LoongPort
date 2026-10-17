//! Current-main actual HTTP ingress; synthetic loopback only.
use super::*;
use futures::StreamExt;
#[tokio::test]
#[serial]
async fn later_free_candidate_does_not_reclassify_an_unrelated_paid_request() {
    let fx = E2eFixture::new().await;
    let mut provider = fx
        .db
        .get_provider_by_id(&fx.expensive_id, "claude")
        .unwrap()
        .unwrap();
    provider.settings_config["freeOnly"] = json!(true);
    fx.db.save_provider("claude", &provider).unwrap();
    let response = send_model_message(fx.port, "synthetic-paid-first", "alias").await;
    let status = response.status();
    let _ = response.bytes().await;
    fx.server.stop().await.unwrap();
    assert!(status.is_success());
    assert_eq!(fx.cheap.hits(), 1);
    assert_eq!(fx.expensive.hits(), 0);
}

#[tokio::test]
#[serial]
async fn free_review_configured_intent_survives_an_early_config_failure() {
    let fx = E2eFixture::new().await;
    let mut provider = fx
        .db
        .get_provider_by_id(&fx.cheap_id, "claude")
        .unwrap()
        .unwrap();
    provider.settings_config["freeOnly"] = json!(true);
    provider.settings_config["env"]
        .as_object_mut()
        .unwrap()
        .remove("ANTHROPIC_BASE_URL");
    fx.db.save_provider("claude", &provider).unwrap();
    let response = send_model_message(fx.port, "synthetic-free-early", "alias").await;
    let status = response.status();
    let _ = response.bytes().await;
    fx.server.stop().await.unwrap();
    assert_eq!(fx.cheap.hits(), 0);
    assert_eq!(
        fx.expensive.hits(),
        0,
        "configured free request reached paid fallback after a config failure"
    );
    assert!(!status.is_success());
}

#[tokio::test]
#[serial]
async fn free_model_intent_never_reaches_a_paid_or_unverified_fallback() {
    let fx = E2eFixture::new().await;
    let response = reqwest::Client::builder().no_proxy().build().unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages",fx.port))
        .header("x-api-key","synthetic-client")
        .header("anthropic-version","2023-06-01")
        .json(&json!({"model":"openrouter/free","max_tokens":16,"messages":[{"role":"user","content":"synthetic"}]}))
        .send().await.unwrap();
    let status = response.status();
    let _ = response.bytes().await;
    fx.server.stop().await.unwrap();
    assert_eq!(
        fx.cheap.hits(),
        0,
        "free request reached an unverified paid provider"
    );
    assert_eq!(
        fx.expensive.hits(),
        0,
        "free request fell back to a paid provider"
    );
    assert!(!status.is_success());
}

async fn collect(response: reqwest::Response) -> String {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => body.extend_from_slice(&bytes),
                Err(_) => break,
            }
        }
    })
    .await
    .unwrap();
    String::from_utf8(body).unwrap()
}

#[tokio::test]
#[serial]
async fn openrouter_main_entry_malformed_chat_chunk_must_not_synthesize_success() {
    for wire in [
        "data: {\"id\":\"bad-synthetic\",\"choices\":[{\"delta\":{\"content\":7}}]}\n\ndata: [DONE]\n\n",
        "data: {\"id\":\"partial\",\"model\":\"vendor/model\",\"choices\":[{\"delta\":{\"content\":\"partial-text\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":7}}]}\n\ndata: [DONE]\n\n",
        "data: {\"id\":\"partial\",\"choices\":[{\"delta\":{\"content\":\"partial-text\"}}]}\n\ndata: {\"error\":{\"code\":\"content_filter\",\"message\":\"synthetic refusal\"}}\n\ndata: [DONE]\n\n",
    ] {
        rejected_chat_wire(wire).await;
    }
}

async fn receive_chat_wire(wire: &'static str) -> String {
    receive_chat_bytes(wire, None).await
}

async fn receive_chat_bytes(wire: &'static str, chunk_size: Option<usize>) -> String {
    let fx = E2eFixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().fallback(move || async move {
        // Pace rejection cases so a later rejection cannot swallow prior text.
        // Byte fragments retain HTTP/1 chunked frame boundaries without sleeps;
        // per-byte timers can exhaust collect()'s deadline on Windows.
        let blocks: Vec<Vec<u8>> = match chunk_size {
            Some(size) => wire
                .as_bytes()
                .chunks(size)
                .map(|bytes| bytes.to_vec())
                .collect(),
            None => wire
                .split("\n\n")
                .filter(|block| !block.is_empty())
                .map(|block| format!("{block}\n\n").into_bytes())
                .collect(),
        };
        let stream = futures::stream::iter(blocks).then(move |block| async move {
            if chunk_size.is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Ok::<_, std::io::Error>(block)
        });
        (
            [("content-type", "text/event-stream")],
            axum::body::Body::from_stream(stream),
        )
    });
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut provider = fx
        .db
        .get_provider_by_id(&fx.cheap_id, "claude")
        .unwrap()
        .unwrap();
    provider.settings_config["env"]["ANTHROPIC_BASE_URL"] =
        json!(format!("http://127.0.0.1:{port}/openrouter.ai/api"));
    provider.settings_config["api_format"] = json!("openai_chat");
    fx.db.save_provider("claude", &provider).unwrap();
    // A rejection before the first valid chunk may close HTTP before headers.
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://127.0.0.1:{}/v1/messages", fx.port))
        .header("x-api-key", "synthetic-client")
        .header("anthropic-version", "2023-06-01")
        .json(
            &json!({"model":"vendor/model", "max_tokens":16, "stream":true,
            "messages":[{"role":"user","content":"hi"}]}),
        )
        .send()
        .await;
    let output = match response {
        Ok(response) => collect(response).await,
        Err(error) => {
            assert!(
                !wire.contains("partial-text"),
                "partial stream must reach client before rejection: {error}"
            );
            assert!(error.is_request(), "unexpected client error: {error}");
            String::new()
        }
    };
    fx.server.stop().await.unwrap();
    task.abort();
    assert_eq!(fx.expensive.hits(), 0);
    if wire.contains("partial-text") {
        assert!(output.contains("partial-text"));
    }
    output
}

async fn rejected_chat_wire(wire: &'static str) {
    let output = receive_chat_wire(wire).await;
    assert!(
        !output.contains("message_stop"),
        "invalid stream became success: {output}"
    );
}

#[tokio::test]
#[serial]
async fn openrouter_main_entry_wire_error_after_finish_suppresses_pending_success() {
    for wire in [
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial-text\"},\"finish_reason\":\"stop\"}]}\n\nevent: error\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial-text\"},\"finish_reason\":\"stop\"}]}\n\ndata: {\"error\":{\"message\":\"synthetic\"}}\n\ndata: [DONE]\n\n",
    ] { rejected_chat_wire(wire).await; }
}

#[tokio::test]
#[serial]
async fn openrouter_main_entry_wire_multiline_and_done_ignore_tail() {
    let wire = concat!(
        "data: {\"id\":\"multiline\",\"choices\":[{\n",
        "data: \"delta\":{\"content\":\"partial-text\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"unwanted-tail\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let output = receive_chat_wire(wire).await;
    assert_eq!(output.matches("event: message_stop").count(), 1);
    assert_eq!(output.matches("event: message_delta").count(), 1);
    assert!(!output.contains("unwanted-tail"));
}

#[tokio::test]
#[serial]
async fn openrouter_main_entry_wire_null_error_does_not_reject_valid_finish() {
    let wire = concat!(
        "data: {\"error\":null,\"choices\":[{\"delta\":{\"content\":\"partial-text\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let output = receive_chat_wire(wire).await;
    assert!(!output.contains("event: error"));
    assert_eq!(output.matches("event: message_stop").count(), 1);
}

#[tokio::test]
#[serial]
async fn openrouter_main_entry_fragmented_utf8_usage_and_terminal() {
    let wire = concat!(
        "data: {\"id\":\"utf8-synthetic\",\"model\":\"vendor/model\",\"choices\":[{\"delta\":{\"content\":\"partial-text你好🌊\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}\n\n",
        "data: [DONE]\n\n",
    );
    for size in [1, 7, 31] {
        let output = receive_chat_bytes(wire, Some(size)).await;
        assert!(
            output.contains("你好🌊"),
            "UTF8 corrupted at chunk {size}: {output}"
        );
        assert!(!output.contains('�'));
        assert_eq!(output.matches("event: message_start").count(), 1);
        assert_eq!(output.matches("event: message_delta").count(), 1);
        assert_eq!(output.matches("event: message_stop").count(), 1);
        let usage = output
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .find(|event| event["type"] == "message_delta")
            .unwrap();
        assert_eq!(usage["usage"]["output_tokens"], 7);
    }
}

#[tokio::test]
#[serial]
async fn openrouter_main_entry_preserves_597_no_finish_block_closure() {
    let output = receive_chat_wire(
        "data: {\"choices\":[{\"delta\":{\"content\":\"partial-text\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    assert_eq!(output.matches("event: content_block_start").count(), 1);
    assert_eq!(output.matches("event: content_block_stop").count(), 1);
    assert_eq!(output.matches("event: message_stop").count(), 1);
    assert!(!output.contains("event: error"));
}
