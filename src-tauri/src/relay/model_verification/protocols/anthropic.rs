use serde_json::{json, Value};

use crate::relay::model_verification::{
    capability_profiles::CapabilityProfile,
    protocols::{send_and_read, send_sse, RunFailure},
    target::ResolvedTarget,
    types::{EvidenceCode, EvidenceFact, EvidenceOutcome},
};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const REPORT_PROBE: &str = "report_probe";

pub(crate) async fn run_balanced(
    client: &reqwest::Client,
    target: &ResolvedTarget,
    profile: &CapabilityProfile,
) -> Result<Vec<EvidenceFact>, RunFailure> {
    let model = upstream_model(&target.target().model);
    let endpoint = format!(
        "{}/v1/messages",
        target.protocol_base().trim_end_matches('/')
    );

    let core = send_message(client, &endpoint, target.api_key(), core_request(&model)).await?;
    let mut facts = parse_core_response(&core, &model);

    let tool = send_message(client, &endpoint, target.api_key(), tool_request(&model)).await?;
    facts.push(parse_tool_response(&tool));

    let mut stream_facts = StreamReducer::new(&model, profile);
    send_stream(
        client,
        &endpoint,
        target.api_key(),
        stream_request(&model),
        |event| stream_facts.observe(event),
    )
    .await?;
    facts.extend(stream_facts.finish());

    if profile.supports_thinking_signature {
        let thinking = send_message(
            client,
            &endpoint,
            target.api_key(),
            thinking_request(&model),
        )
        .await?;
        let (thinking_fact, signed_thinking_block) = reduce_thinking_response(&thinking);
        facts.push(thinking_fact);

        if profile.supports_signature_continuation {
            facts.push(match signed_thinking_block {
                Some(block) => {
                    match send_message(
                        client,
                        &endpoint,
                        target.api_key(),
                        continuation_request(&model, block),
                    )
                    .await
                    {
                        Ok(response) if is_message_envelope(&response) => {
                            passed(EvidenceCode::SignatureContinuation)
                        }
                        Ok(_) | Err(RunFailure::InvalidResponse) => {
                            failed(EvidenceCode::SignatureContinuation)
                        }
                        Err(error) => return Err(error),
                    }
                }
                None => failed(EvidenceCode::SignatureContinuation),
            });
        }
    }

    Ok(facts)
}

async fn send_message(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    payload: Value,
) -> Result<Vec<u8>, RunFailure> {
    send_and_read(
        client
            .post(endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&payload),
    )
    .await
}

async fn send_stream(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    payload: Value,
    on_event: impl FnMut(&str) -> Result<(), RunFailure>,
) -> Result<(), RunFailure> {
    send_sse(
        client
            .post(endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("accept", "text/event-stream")
            .json(&payload),
        on_event,
    )
    .await
}

fn core_request(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "Reply with the word ready."}],
    })
}

fn tool_request(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "Call report_probe with an object containing ready: true."}],
        "tools": [{
            "name": REPORT_PROBE,
            "description": "Return a fixed verification object.",
            "input_schema": {
                "type": "object",
                "properties": {"ready": {"type": "boolean"}},
                "required": ["ready"]
            }
        }],
        "tool_choice": {"type": "tool", "name": REPORT_PROBE},
    })
}

fn stream_request(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 32,
        "stream": true,
        "messages": [{"role": "user", "content": "Reply with the word stream."}],
    })
}

fn thinking_request(model: &str) -> Value {
    let thinking = if model.starts_with("claude-haiku-4-5") {
        json!({"type": "enabled", "budget_tokens": 1024})
    } else {
        json!({"type": "adaptive"})
    };
    json!({
        "model": model,
        "max_tokens": 2048,
        "thinking": thinking,
        "messages": [{"role": "user", "content": "Think briefly, then reply ready."}],
    })
}

fn continuation_request(model: &str, thinking_block: Value) -> Value {
    json!({
        "model": model,
        "max_tokens": 64,
        "messages": [
            {"role": "user", "content": "Think briefly, then reply ready."},
            {"role": "assistant", "content": [thinking_block]},
            {"role": "user", "content": "Continue and reply ready."},
        ],
    })
}

fn parse_core_response(body: &[u8], expected_model: &str) -> Vec<EvidenceFact> {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return vec![failed(EvidenceCode::BasicEnvelope)];
    };
    if has_openai_fingerprint(&response) {
        return vec![failed(EvidenceCode::ForeignProtocol)];
    }
    let mut facts = vec![if is_message_envelope_value(&response) {
        passed(EvidenceCode::BasicEnvelope)
    } else {
        failed(EvidenceCode::BasicEnvelope)
    }];
    if let Some(model) = response.get("model").and_then(Value::as_str) {
        facts.push(if model == expected_model {
            passed(EvidenceCode::ModelMatch)
        } else {
            failed(EvidenceCode::ModelMatch)
        });
    }
    facts.push(if usage_is_consistent(&response) {
        passed(EvidenceCode::UsageConsistency)
    } else {
        failed(EvidenceCode::UsageConsistency)
    });
    facts
}

fn parse_tool_response(body: &[u8]) -> EvidenceFact {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return failed(EvidenceCode::ToolCallShape);
    };
    let is_valid_tool = response
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("name").and_then(Value::as_str) == Some(REPORT_PROBE)
                    && block.get("input").is_some_and(Value::is_object)
            })
        });
    if is_valid_tool {
        passed(EvidenceCode::ToolCallShape)
    } else {
        failed(EvidenceCode::ToolCallShape)
    }
}

fn reduce_thinking_response(body: &[u8]) -> (EvidenceFact, Option<Value>) {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return (failed(EvidenceCode::ThinkingSignature), None);
    };
    let Some(block) = response
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find(|block| {
                block.get("type").and_then(Value::as_str) == Some("thinking")
                    && block
                        .get("signature")
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.is_empty())
            })
        })
    else {
        return (failed(EvidenceCode::ThinkingSignature), None);
    };
    (passed(EvidenceCode::ThinkingSignature), Some(block.clone()))
}

fn is_message_envelope(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .is_some_and(|value| is_message_envelope_value(&value))
}

fn is_message_envelope_value(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("message")
        && value.get("model").and_then(Value::as_str).is_some()
        && value.get("content").and_then(Value::as_array).is_some()
        && value.get("usage").is_some_and(usage_is_consistent)
}

fn usage_is_consistent(value: &Value) -> bool {
    let usage = value.get("usage").unwrap_or(value);
    usage.get("input_tokens").and_then(Value::as_u64).is_some()
        && usage.get("output_tokens").and_then(Value::as_u64).is_some()
}

fn has_openai_fingerprint(value: &Value) -> bool {
    value.get("object").and_then(Value::as_str) == Some("chat.completion")
        || value.get("choices").and_then(Value::as_array).is_some()
        || value.get("output").and_then(Value::as_array).is_some()
}

fn upstream_model(model: &str) -> String {
    model
        .trim()
        .strip_suffix("[1m]")
        .unwrap_or(model.trim())
        .to_string()
}

fn passed(code: EvidenceCode) -> EvidenceFact {
    EvidenceFact {
        code,
        outcome: EvidenceOutcome::Passed,
    }
}

fn failed(code: EvidenceCode) -> EvidenceFact {
    EvidenceFact {
        code,
        outcome: EvidenceOutcome::Failed,
    }
}

struct StreamReducer<'a> {
    expected_model: &'a str,
    profile: &'a CapabilityProfile,
    saw_message_start: bool,
    saw_content_start: bool,
    saw_message_delta: bool,
    saw_message_stop: bool,
    lifecycle_order_valid: bool,
    model_matches: Option<bool>,
    usage_consistent: bool,
    saw_usage: bool,
    saw_thinking: bool,
    saw_signature: bool,
    foreign_protocol: bool,
}

impl<'a> StreamReducer<'a> {
    fn new(expected_model: &'a str, profile: &'a CapabilityProfile) -> Self {
        Self {
            expected_model,
            profile,
            saw_message_start: false,
            saw_content_start: false,
            saw_message_delta: false,
            saw_message_stop: false,
            lifecycle_order_valid: true,
            model_matches: None,
            usage_consistent: true,
            saw_usage: false,
            saw_thinking: false,
            saw_signature: false,
            foreign_protocol: false,
        }
    }

    fn observe(&mut self, event: &str) -> Result<(), RunFailure> {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| RunFailure::InvalidResponse)?;
        if has_openai_fingerprint(&value)
            || value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("response."))
        {
            self.foreign_protocol = true;
            return Ok(());
        }
        match value.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                if self.saw_message_start
                    || self.saw_content_start
                    || self.saw_message_delta
                    || self.saw_message_stop
                {
                    self.lifecycle_order_valid = false;
                }
                self.saw_message_start = true;
                let message = value.get("message").unwrap_or(&value);
                self.model_matches = message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(|model| model == self.expected_model);
                self.observe_usage(message.get("usage"));
            }
            Some("content_block_start") => {
                if !self.saw_message_start || self.saw_message_delta || self.saw_message_stop {
                    self.lifecycle_order_valid = false;
                }
                self.saw_content_start = true;
                if value
                    .get("content_block")
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    == Some("thinking")
                {
                    self.saw_thinking = true;
                }
            }
            Some("content_block_delta")
                if value
                    .get("delta")
                    .and_then(|delta| delta.get("type"))
                    .and_then(Value::as_str)
                    == Some("signature_delta")
                    && value
                        .get("delta")
                        .and_then(|delta| delta.get("signature"))
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.is_empty())
                    && self.saw_thinking =>
            {
                self.saw_signature = true;
            }
            Some("message_delta") => {
                if !self.saw_content_start || self.saw_message_stop {
                    self.lifecycle_order_valid = false;
                }
                self.saw_message_delta = true;
                self.observe_usage(value.get("usage"));
            }
            Some("message_stop") => {
                if !self.saw_message_delta || self.saw_message_stop {
                    self.lifecycle_order_valid = false;
                }
                self.saw_message_stop = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn observe_usage(&mut self, usage: Option<&Value>) {
        if let Some(usage) = usage {
            self.saw_usage = true;
            self.usage_consistent &= usage_is_consistent(usage);
        }
    }

    fn finish(self) -> Vec<EvidenceFact> {
        let mut facts = vec![if self.lifecycle_order_valid
            && self.saw_message_start
            && self.saw_content_start
            && self.saw_message_delta
            && self.saw_message_stop
        {
            passed(EvidenceCode::StreamLifecycle)
        } else {
            failed(EvidenceCode::StreamLifecycle)
        }];
        if let Some(matches) = self.model_matches {
            facts.push(if matches {
                passed(EvidenceCode::ModelMatch)
            } else {
                failed(EvidenceCode::ModelMatch)
            });
        }
        facts.push(if self.saw_usage && self.usage_consistent {
            passed(EvidenceCode::UsageConsistency)
        } else {
            failed(EvidenceCode::UsageConsistency)
        });
        if self.profile.supports_thinking_signature {
            facts.push(if self.saw_thinking && self.saw_signature {
                passed(EvidenceCode::ThinkingSignature)
            } else {
                failed(EvidenceCode::ThinkingSignature)
            });
        }
        if self.foreign_protocol {
            facts.push(failed(EvidenceCode::ForeignProtocol));
        }
        facts
    }
}

pub(crate) fn parse_stream(
    stream: &str,
    expected_model: &str,
    profile: &CapabilityProfile,
) -> Result<Vec<EvidenceFact>, RunFailure> {
    let mut reducer = StreamReducer::new(expected_model, profile);
    for event in stream.split("\n\n") {
        if !event.trim().is_empty() {
            reducer.observe(event)?;
        }
    }
    Ok(reducer.finish())
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::REPORT_PROBE;

    use crate::{
        app_config::AppType,
        database::Database,
        provider::{Provider, ProviderMeta},
        relay::model_verification::{
            capability_profiles::CapabilityProfile,
            protocols::{
                anthropic::{parse_core_response, parse_stream, run_balanced},
                RunFailure, MAX_RESPONSE_BYTES, MAX_SSE_EVENT_BYTES,
            },
            target::ResolvedTarget,
            types::{EvidenceCode, EvidenceFact, EvidenceOutcome, TargetKey},
        },
    };
    use axum::{
        extract::{OriginalUri, State},
        http::{HeaderMap, HeaderValue, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
        Json, Router,
    };
    use serde_json::{json, Value};

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        path: String,
        api_key: Option<HeaderValue>,
        version: Option<HeaderValue>,
        content_type: Option<HeaderValue>,
        body: Value,
    }

    type Requests = Arc<Mutex<Vec<RecordedRequest>>>;

    #[test]
    fn stream_reduces_an_opaque_thinking_signature_to_a_fact() {
        let signature = "SENTINEL_SIGNATURE_MUST_NOT_PERSIST";
        let stream = format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"type\":\"message\",\"model\":\"claude-haiku-4-5\",\"usage\":{{\"input_tokens\":3,\"output_tokens\":0}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"thinking\",\"thinking\":\"private\"}}}}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"signature_delta\",\"signature\":\"{signature}\"}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":2}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        );
        let profile = CapabilityProfile::for_target(&AppType::Claude, "claude-haiku-4-5");

        let facts = parse_stream(&stream, "claude-haiku-4-5", &profile).unwrap();

        assert!(facts.iter().any(|fact| {
            fact.code == EvidenceCode::ThinkingSignature && fact.outcome == EvidenceOutcome::Passed
        }));
        assert!(!serde_json::to_string(&facts).unwrap().contains(signature));
    }

    #[test]
    fn message_envelope_reduces_model_usage_and_foreign_protocol_without_text() {
        let expected = "claude-haiku-4-5";
        let response = br#"{
            "type":"message","model":"claude-haiku-4-5","content":[{"type":"text","text":"SENTINEL_RESPONSE_TEXT"}],
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#;

        let facts = parse_core_response(response, expected);

        assert!(facts.contains(&passed(EvidenceCode::BasicEnvelope)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
        assert!(!serde_json::to_string(&facts)
            .unwrap()
            .contains("SENTINEL_RESPONSE_TEXT"));

        let foreign =
            parse_core_response(br#"{"object":"chat.completion","choices":[]}"#, expected);
        assert_eq!(foreign, vec![failed(EvidenceCode::ForeignProtocol)]);
    }

    #[test]
    fn additive_stream_events_do_not_break_anthropic_lifecycle() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");
        let stream = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\",\"metadata\":{\"ignored\":true}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );

        let facts = parse_stream(stream, "future-model-x", &profile).unwrap();

        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
    }

    #[test]
    fn out_of_order_stream_events_cannot_satisfy_lifecycle_or_signature_checks() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "claude-haiku-4-5");
        let stream = concat!(
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SENTINEL_SIGNATURE\"}}\n\n",
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-haiku-4-5\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"thinking\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n"
        );

        let facts = parse_stream(stream, "claude-haiku-4-5", &profile).unwrap();

        assert!(facts.contains(&failed(EvidenceCode::StreamLifecycle)));
        assert!(facts.contains(&failed(EvidenceCode::ThinkingSignature)));
    }

    #[tokio::test]
    async fn balanced_probe_uses_messages_contract_and_keeps_private_values_out_of_facts() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/messages", post(happy_handler))
                .with_state(requests.clone()),
        )
        .await;
        let target = target_for(&endpoint, "SENTINEL_API_KEY");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let profile = CapabilityProfile::for_target(&AppType::Claude, "claude-haiku-4-5");

        let facts = run_balanced(&client, &target, &profile).await.unwrap();

        assert!(facts.contains(&passed(EvidenceCode::ToolCallShape)));
        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
        assert!(facts.contains(&passed(EvidenceCode::ThinkingSignature)));
        assert!(facts.contains(&passed(EvidenceCode::SignatureContinuation)));
        let serialized = serde_json::to_string(&facts).unwrap();
        for private_value in [
            "SENTINEL_API_KEY",
            "SENTINEL_SIGNATURE",
            "SENTINEL_THINKING",
            "report ready",
        ] {
            assert!(!serialized.contains(private_value));
        }

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        for request in requests.iter() {
            assert_eq!(request.path, "/v1/messages");
            assert_eq!(request.api_key.as_ref().unwrap(), "SENTINEL_API_KEY");
            assert_eq!(request.version.as_ref().unwrap(), "2023-06-01");
            assert_eq!(request.content_type.as_ref().unwrap(), "application/json");
        }
        let continuation = &requests[4].body["messages"][1]["content"][0];
        assert_eq!(continuation["signature"], "SENTINEL_SIGNATURE");
        assert_eq!(continuation["thinking"], "SENTINEL_THINKING");
    }

    #[tokio::test]
    async fn http_failures_are_finite_and_do_not_include_error_body() {
        for (status, body, expected) in [
            (
                StatusCode::UNAUTHORIZED,
                "SENTINEL_401",
                RunFailure::Authentication,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                "SENTINEL_429",
                RunFailure::RateLimited,
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                "insufficient balance SENTINEL_402",
                RunFailure::InsufficientBalance,
            ),
            (
                StatusCode::BAD_GATEWAY,
                "SENTINEL_502",
                RunFailure::Upstream,
            ),
        ] {
            let app =
                Router::new().route("/v1/messages", post(move || async move { (status, body) }));
            let endpoint = spawn_server(app).await;
            let target = target_for(&endpoint, "SENTINEL_API_KEY");
            let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");

            let error = run_balanced(&reqwest::Client::new(), &target, &profile)
                .await
                .unwrap_err();

            assert_eq!(error, expected);
            assert!(!format!("{error:?}").contains("SENTINEL"));
        }
    }

    #[tokio::test]
    async fn malformed_success_is_protocol_evidence_not_a_leaked_error() {
        let calls = Arc::new(Mutex::new(0usize));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/messages", post(malformed_then_happy))
                .with_state(calls),
        )
        .await;
        let target = target_for(&endpoint, "SENTINEL_API_KEY");
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");

        let facts = run_balanced(&reqwest::Client::new(), &target, &profile)
            .await
            .unwrap();

        assert!(facts.contains(&failed(EvidenceCode::ForeignProtocol)));
        assert!(!serde_json::to_string(&facts).unwrap().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn oversized_body_and_sse_event_stop_with_sanitized_failure() {
        let large_body = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let endpoint = spawn_server(
            Router::new().route("/v1/messages", post(move || async move { large_body })),
        )
        .await;
        let target = target_for(&endpoint, "SENTINEL_API_KEY");
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");
        assert_eq!(
            run_balanced(&reqwest::Client::new(), &target, &profile)
                .await
                .unwrap_err(),
            RunFailure::ResponseTooLarge
        );

        let too_large_event = format!(
            "data: {{\"type\":\"message_start\",\"payload\":\"{}\"}}\n\n",
            "x".repeat(MAX_SSE_EVENT_BYTES)
        );
        let endpoint = spawn_server(Router::new().route(
            "/v1/messages",
            post(move |Json(body): Json<Value>| {
                let too_large_event = too_large_event.clone();
                async move {
                    if body.get("stream") == Some(&Value::Bool(true)) {
                        ([("content-type", "text/event-stream")], too_large_event).into_response()
                    } else {
                        message_response("future-model-x")
                    }
                }
            }),
        ))
        .await;
        let target = target_for(&endpoint, "SENTINEL_API_KEY");
        assert_eq!(
            run_balanced(&reqwest::Client::new(), &target, &profile)
                .await
                .unwrap_err(),
            RunFailure::ResponseTooLarge
        );
    }

    fn passed(code: EvidenceCode) -> EvidenceFact {
        EvidenceFact {
            code,
            outcome: EvidenceOutcome::Passed,
        }
    }

    fn failed(code: EvidenceCode) -> EvidenceFact {
        EvidenceFact {
            code,
            outcome: EvidenceOutcome::Failed,
        }
    }

    async fn spawn_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    fn target_for(endpoint: &str, api_key: &str) -> ResolvedTarget {
        let db = Database::memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO loongport_relay (site_origin, site_name, api_base_url, account_id, account_label, login_identifier, auth_token, sort_index) VALUES (?1, 'Test', ?1, 7, 'test', 'test', 'token', 0)",
                [endpoint],
            ).unwrap();
        }
        db.save_provider(
            "claude",
            &Provider {
                id: "loongport-0123456789abcdef".into(),
                name: "Test tier".into(),
                settings_config: json!({"env": {"ANTHROPIC_AUTH_TOKEN": api_key}}),
                website_url: Some(endpoint.into()),
                category: Some("aggregator".into()),
                created_at: None,
                sort_index: None,
                notes: None,
                meta: Some(ProviderMeta {
                    loongport_account_id: Some(7),
                    ..Default::default()
                }),
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            },
        )
        .unwrap();
        ResolvedTarget::resolve(
            &db,
            TargetKey::new("loongport-0123456789abcdef", "claude", "claude-haiku-4-5"),
        )
        .unwrap()
    }

    async fn happy_handler(
        State(requests): State<Requests>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        requests.lock().unwrap().push(RecordedRequest {
            path: uri.path().into(),
            api_key: headers.get("x-api-key").cloned(),
            version: headers.get("anthropic-version").cloned(),
            content_type: headers.get("content-type").cloned(),
            body: body.clone(),
        });
        if body.get("stream") == Some(&Value::Bool(true)) {
            return ([("content-type", "text/event-stream")], happy_stream()).into_response();
        }
        if body.get("tools").is_some() {
            return Json(json!({
                "type": "message", "model": "claude-haiku-4-5", "content": [{"type": "tool_use", "name": REPORT_PROBE, "input": {"ready": true}}],
                "usage": {"input_tokens": 2, "output_tokens": 1}
            })).into_response();
        }
        if body.get("thinking").is_some() {
            return Json(json!({
                "type": "message", "model": "claude-haiku-4-5", "content": [{"type": "thinking", "thinking": "SENTINEL_THINKING", "signature": "SENTINEL_SIGNATURE"}],
                "usage": {"input_tokens": 2, "output_tokens": 1}
            })).into_response();
        }
        message_response("claude-haiku-4-5")
    }

    async fn malformed_then_happy(State(calls): State<Arc<Mutex<usize>>>) -> Response {
        let mut calls = calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            Json(json!({"object": "chat.completion", "choices": [], "text": "SENTINEL_FOREIGN"}))
                .into_response()
        } else if *calls == 3 {
            ([("content-type", "text/event-stream")], happy_stream()).into_response()
        } else if *calls == 2 {
            Json(json!({"type": "message", "model": "future-model-x", "content": [{"type": "tool_use", "name": REPORT_PROBE, "input": {}}], "usage": {"input_tokens": 1, "output_tokens": 1}})).into_response()
        } else {
            message_response("future-model-x")
        }
    }

    fn message_response(model: &str) -> Response {
        Json(json!({
            "type": "message", "model": model, "content": [{"type": "text", "text": "report ready"}],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        })).into_response()
    }

    fn happy_stream() -> String {
        concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-haiku-4-5\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"thinking\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SENTINEL_SIGNATURE\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        ).into()
    }
}
