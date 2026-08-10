use std::collections::HashMap;

use serde_json::{json, Value};

use crate::proxy::model_mapper::strip_one_m_suffix_for_upstream;
use crate::relay::model_verification::{
    capability_profiles::CapabilityProfile,
    protocols::{send_and_read, send_sse, RunFailure},
    target::ResolvedTarget,
    types::{EvidenceCode, EvidenceFact, EvidenceOutcome},
};

const REPORT_PROBE: &str = "report_probe";
const CORE_STREAM_OUTPUT_TOKENS: u16 = 512;
const TOOL_STRUCTURED_OUTPUT_TOKENS: u16 = 1024;

pub(crate) async fn run_balanced(
    client: &reqwest::Client,
    target: &ResolvedTarget,
    profile: &CapabilityProfile,
) -> Result<Vec<EvidenceFact>, RunFailure> {
    run_balanced_with_progress(client, target, profile, &mut || {}).await
}

pub(crate) async fn run_balanced_with_progress(
    client: &reqwest::Client,
    target: &ResolvedTarget,
    profile: &CapabilityProfile,
    on_probe_complete: &mut impl FnMut(),
) -> Result<Vec<EvidenceFact>, RunFailure> {
    let model = upstream_model(&target.target().model);
    let endpoint = format!("{}/responses", target.protocol_base().trim_end_matches('/'));

    let core = send_response(
        client,
        &endpoint,
        target.api_key(),
        core_request(&model, profile),
    )
    .await?;
    reject_incomplete_response(&core)?;
    let mut facts = parse_core_response(&core, &model);
    on_probe_complete();

    let tool = send_response(
        client,
        &endpoint,
        target.api_key(),
        tool_request(&model, profile),
    )
    .await?;
    reject_incomplete_response(&tool)?;
    facts.push(parse_tool_response(&tool));
    on_probe_complete();

    if profile.supports_structured_output {
        let structured = send_response(
            client,
            &endpoint,
            target.api_key(),
            structured_output_request(&model, profile),
        )
        .await?;
        reject_incomplete_response(&structured)?;
        facts.push(parse_structured_response(&structured));
        on_probe_complete();
    }

    let mut stream = StreamReducer::new(&model);
    send_stream(
        client,
        &endpoint,
        target.api_key(),
        stream_request(&model, profile),
        |event| stream.observe(event),
    )
    .await?;
    facts.extend(stream.finish());
    on_probe_complete();
    Ok(facts)
}

async fn send_response(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    payload: Value,
) -> Result<Vec<u8>, RunFailure> {
    send_and_read(
        client
            .post(endpoint)
            .bearer_auth(api_key)
            .header("accept", "application/json")
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
            .bearer_auth(api_key)
            .header("accept", "text/event-stream")
            .json(&payload),
        on_event,
    )
    .await
}

fn core_request(model: &str, profile: &CapabilityProfile) -> Value {
    with_reasoning_effort(
        json!({
            "model": model,
            "input": "Reply with ready.",
            "max_output_tokens": CORE_STREAM_OUTPUT_TOKENS,
            "store": false,
        }),
        profile,
    )
}

fn tool_request(model: &str, profile: &CapabilityProfile) -> Value {
    with_reasoning_effort(
        json!({
            "model": model,
            "input": "Call report_probe with ready set to true.",
            "max_output_tokens": TOOL_STRUCTURED_OUTPUT_TOKENS,
            "store": false,
            "tools": [{
                "type": "function",
                "name": REPORT_PROBE,
                "description": "Return the fixed verification object.",
                "parameters": {
                    "type": "object",
                    "properties": {"ready": {"type": "boolean"}},
                    "required": ["ready"],
                    "additionalProperties": false,
                },
                "strict": true,
            }],
            "tool_choice": {"type": "function", "name": REPORT_PROBE},
        }),
        profile,
    )
}

fn structured_output_request(model: &str, profile: &CapabilityProfile) -> Value {
    with_reasoning_effort(
        json!({
            "model": model,
            "input": "Return the fixed verification object.",
            "max_output_tokens": TOOL_STRUCTURED_OUTPUT_TOKENS,
            "store": false,
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "loongport_probe",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": {"ready": {"type": "boolean"}},
                        "required": ["ready"],
                        "additionalProperties": false,
                    },
                },
            },
        }),
        profile,
    )
}

fn stream_request(model: &str, profile: &CapabilityProfile) -> Value {
    with_reasoning_effort(
        json!({
            "model": model,
            "input": "Reply with stream.",
            "max_output_tokens": CORE_STREAM_OUTPUT_TOKENS,
            "store": false,
            "stream": true,
        }),
        profile,
    )
}

fn with_reasoning_effort(mut request: Value, profile: &CapabilityProfile) -> Value {
    if profile.supports_low_reasoning_effort {
        request
            .as_object_mut()
            .expect("probe request must be an object")
            .insert("reasoning".into(), json!({"effort":"low"}));
    }
    request
}

fn reject_incomplete_response(body: &[u8]) -> Result<(), RunFailure> {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return Ok(());
    };
    match response.get("status").and_then(Value::as_str) {
        Some("incomplete" | "failed") => Err(RunFailure::InvalidResponse),
        _ => Ok(()),
    }
}

fn upstream_model(model: &str) -> String {
    strip_one_m_suffix_for_upstream(model).to_string()
}

pub(crate) fn parse_core_response(body: &[u8], expected_model: &str) -> Vec<EvidenceFact> {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return vec![failed(EvidenceCode::BasicEnvelope)];
    };
    if has_foreign_protocol_fingerprint(&response) {
        return vec![failed(EvidenceCode::ForeignProtocol)];
    }

    let mut facts = vec![if is_response_envelope(&response) {
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
    facts.push(if usage_is_consistent(response.get("usage")) {
        passed(EvidenceCode::UsageConsistency)
    } else {
        failed(EvidenceCode::UsageConsistency)
    });
    facts
}

pub(crate) fn parse_tool_response(body: &[u8]) -> EvidenceFact {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return failed(EvidenceCode::ToolCallShape);
    };
    if has_foreign_protocol_fingerprint(&response) {
        return failed(EvidenceCode::ForeignProtocol);
    }
    if !is_response_envelope(&response) {
        return failed(EvidenceCode::ToolCallShape);
    }
    let valid_tool_call = response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call")
                    && item.get("name").and_then(Value::as_str) == Some(REPORT_PROBE)
                    && item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .is_some_and(fixed_probe_payload_matches_schema)
            })
        });
    if valid_tool_call {
        passed(EvidenceCode::ToolCallShape)
    } else {
        failed(EvidenceCode::ToolCallShape)
    }
}

pub(crate) fn parse_structured_response(body: &[u8]) -> EvidenceFact {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return failed(EvidenceCode::StructuredOutput);
    };
    if has_foreign_protocol_fingerprint(&response) {
        return failed(EvidenceCode::ForeignProtocol);
    }
    if !is_response_envelope(&response) {
        return failed(EvidenceCode::StructuredOutput);
    }
    let has_output_text = response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("message")
                    && item
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|parts| {
                            parts.iter().any(|part| {
                                part.get("type").and_then(Value::as_str) == Some("output_text")
                                    && part
                                        .get("text")
                                        .and_then(Value::as_str)
                                        .is_some_and(fixed_probe_payload_matches_schema)
                            })
                        })
            })
        });
    if has_output_text {
        passed(EvidenceCode::StructuredOutput)
    } else {
        failed(EvidenceCode::StructuredOutput)
    }
}

fn is_response_envelope(value: &Value) -> bool {
    value.get("object").and_then(Value::as_str) == Some("response")
        && value.get("status").and_then(Value::as_str) == Some("completed")
        && value.get("model").and_then(Value::as_str).is_some()
        && value
            .get("output")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                !items.is_empty()
                    && items.iter().all(|item| {
                        matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("message" | "reasoning" | "function_call")
                        )
                    })
            })
}

fn has_foreign_protocol_fingerprint(value: &Value) -> bool {
    value.get("object").and_then(Value::as_str) == Some("chat.completion")
        || value.get("choices").is_some()
        || (value.get("type").and_then(Value::as_str) == Some("message")
            && value.get("content").and_then(Value::as_array).is_some())
}

fn fixed_probe_payload_matches_schema(payload: &str) -> bool {
    serde_json::from_str::<Value>(payload)
        .ok()
        .is_some_and(|value| {
            value.as_object().is_some_and(|object| {
                object.len() == 1 && object.get("ready") == Some(&Value::Bool(true))
            })
        })
}

fn usage_is_consistent(usage: Option<&Value>) -> bool {
    let Some(usage) = usage else {
        return false;
    };
    let (Some(input), Some(output), Some(total)) = (
        usage.get("input_tokens").and_then(Value::as_u64),
        usage.get("output_tokens").and_then(Value::as_u64),
        usage.get("total_tokens").and_then(Value::as_u64),
    ) else {
        return false;
    };
    input.saturating_add(output) == total
}

pub(crate) fn parse_stream(
    stream: &str,
    expected_model: &str,
    _profile: &CapabilityProfile,
) -> Result<Vec<EvidenceFact>, RunFailure> {
    let mut reducer = StreamReducer::new(expected_model);
    for event in stream.split("\n\n") {
        if !event.trim().is_empty() {
            reducer.observe(event)?;
        }
    }
    Ok(reducer.finish())
}

struct StreamReducer<'a> {
    expected_model: &'a str,
    saw_created: bool,
    saw_completed: bool,
    items: HashMap<u64, OutputItemLifecycle>,
    completed_messages: u8,
    lifecycle_order_valid: bool,
    model_matches: Option<bool>,
    usage_consistent: bool,
    saw_usage: bool,
    foreign_protocol: bool,
}

impl<'a> StreamReducer<'a> {
    fn new(expected_model: &'a str) -> Self {
        Self {
            expected_model,
            saw_created: false,
            saw_completed: false,
            items: HashMap::new(),
            completed_messages: 0,
            lifecycle_order_valid: true,
            model_matches: None,
            usage_consistent: true,
            saw_usage: false,
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
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| RunFailure::InvalidResponse)?;
        if has_foreign_protocol_fingerprint(&value)
            || value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("message_"))
        {
            self.foreign_protocol = true;
            return Ok(());
        }

        match value.get("type").and_then(Value::as_str) {
            Some("response.created") => {
                if self.saw_created || self.saw_completed {
                    self.lifecycle_order_valid = false;
                }
                self.saw_created = true;
                self.observe_response(value.get("response"), false);
            }
            Some("response.completed") => {
                if !self.saw_created
                    || self.items.values().any(|item| !item.closed)
                    || self.completed_messages != 1
                    || self.saw_completed
                {
                    self.lifecycle_order_valid = false;
                }
                self.saw_completed = true;
                self.observe_response(value.get("response"), true);
            }
            Some("response.failed" | "response.incomplete") => {
                return Err(RunFailure::InvalidResponse);
            }
            Some("response.output_item.added") => {
                self.add_output_item(&value);
            }
            Some("response.content_part.added") => {
                if value
                    .get("part")
                    .and_then(|part| part.get("type"))
                    .and_then(Value::as_str)
                    != Some("output_text")
                    || !self.advance_message(
                        &value,
                        TextStreamProgress::AwaitContentPart,
                        TextStreamProgress::AwaitTextDelta,
                    )
                {
                    self.lifecycle_order_valid = false;
                }
            }
            Some("response.output_text.delta") => {
                if !self.advance_message_one_of(
                    &value,
                    &[
                        TextStreamProgress::AwaitTextDelta,
                        TextStreamProgress::TextStreaming,
                    ],
                    TextStreamProgress::TextStreaming,
                ) {
                    self.lifecycle_order_valid = false;
                }
            }
            Some("response.output_text.done") => {
                if !self.message_is_at(&value, TextStreamProgress::TextStreaming) {
                    self.lifecycle_order_valid = false;
                }
            }
            Some("response.content_part.done") => {
                if !self.advance_message(
                    &value,
                    TextStreamProgress::TextStreaming,
                    TextStreamProgress::AwaitItemDone,
                ) {
                    self.lifecycle_order_valid = false;
                }
            }
            Some("response.output_item.done") => {
                self.complete_output_item(&value);
            }
            Some(
                "response.function_call_arguments.delta" | "response.function_call_arguments.done",
            ) => {
                if self
                    .find_open_item_index(&value, OutputItemType::FunctionCall)
                    .is_none()
                {
                    self.lifecycle_order_valid = false;
                }
            }
            Some(kind) if kind.starts_with("response.") => {
                if !self.in_active_response() {
                    self.lifecycle_order_valid = false;
                }
            }
            _ => self.lifecycle_order_valid = false,
        }
        Ok(())
    }

    fn in_active_response(&self) -> bool {
        self.saw_created && !self.saw_completed
    }

    fn add_output_item(&mut self, value: &Value) {
        let item_type = value
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            .and_then(OutputItemType::from_wire);
        let index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .or_else(|| self.items.is_empty().then_some(0));
        let (Some(index), Some(item_type)) = (index, item_type) else {
            self.lifecycle_order_valid = false;
            return;
        };
        if !self.in_active_response() || self.items.contains_key(&index) {
            self.lifecycle_order_valid = false;
            return;
        }
        let progress = match item_type {
            OutputItemType::Message => Some(TextStreamProgress::AwaitContentPart),
            OutputItemType::Reasoning | OutputItemType::FunctionCall => None,
        };
        self.items.insert(
            index,
            OutputItemLifecycle {
                item_type,
                text_progress: progress,
                closed: false,
            },
        );
    }

    fn complete_output_item(&mut self, value: &Value) {
        let requested_type = value
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            .and_then(OutputItemType::from_wire);
        let index = value
            .get("output_index")
            .and_then(Value::as_u64)
            .or_else(|| {
                let mut open = self
                    .items
                    .iter()
                    .filter_map(|(index, item)| (!item.closed).then_some(*index));
                let only = open.next();
                (open.next().is_none()).then_some(only).flatten()
            });
        let Some(item) = index.and_then(|index| self.items.get_mut(&index)) else {
            self.lifecycle_order_valid = false;
            return;
        };
        if item.closed
            || requested_type.is_some_and(|item_type| item_type != item.item_type)
            || (item.item_type == OutputItemType::Message
                && item.text_progress != Some(TextStreamProgress::AwaitItemDone))
        {
            self.lifecycle_order_valid = false;
            return;
        }
        item.closed = true;
        if item.item_type == OutputItemType::Message {
            self.completed_messages = self.completed_messages.saturating_add(1);
        }
    }

    fn find_open_item_index(&self, value: &Value, item_type: OutputItemType) -> Option<u64> {
        if !self.in_active_response() {
            return None;
        }
        if let Some(index) = value.get("output_index").and_then(Value::as_u64) {
            return self
                .items
                .get(&index)
                .is_some_and(|item| item.item_type == item_type && !item.closed)
                .then_some(index);
        }
        let mut matches = self.items.iter().filter_map(|(index, item)| {
            (item.item_type == item_type && !item.closed).then_some(*index)
        });
        let only = matches.next()?;
        matches.next().is_none().then_some(only)
    }

    fn message_is_at(&self, value: &Value, expected: TextStreamProgress) -> bool {
        self.find_open_item_index(value, OutputItemType::Message)
            .and_then(|index| self.items.get(&index))
            .is_some_and(|item| item.text_progress == Some(expected))
    }

    fn advance_message(
        &mut self,
        value: &Value,
        expected: TextStreamProgress,
        next: TextStreamProgress,
    ) -> bool {
        self.advance_message_one_of(value, &[expected], next)
    }

    fn advance_message_one_of(
        &mut self,
        value: &Value,
        expected: &[TextStreamProgress],
        next: TextStreamProgress,
    ) -> bool {
        let Some(index) = self.find_open_item_index(value, OutputItemType::Message) else {
            return false;
        };
        let item = self
            .items
            .get_mut(&index)
            .expect("resolved item must exist");
        if !item
            .text_progress
            .is_some_and(|progress| expected.contains(&progress))
        {
            return false;
        }
        item.text_progress = Some(next);
        true
    }

    fn observe_response(&mut self, response: Option<&Value>, terminal: bool) {
        let Some(response) = response else {
            self.lifecycle_order_valid = false;
            return;
        };
        if has_foreign_protocol_fingerprint(response)
            || response.get("object").and_then(Value::as_str) != Some("response")
        {
            self.foreign_protocol = true;
            self.lifecycle_order_valid = false;
            return;
        }
        if terminal && response.get("status").and_then(Value::as_str) != Some("completed") {
            self.lifecycle_order_valid = false;
        }
        if let Some(model) = response.get("model").and_then(Value::as_str) {
            self.model_matches = Some(model == self.expected_model);
        }
        if response.get("usage").is_some() {
            self.saw_usage = true;
            self.usage_consistent &= usage_is_consistent(response.get("usage"));
        }
    }

    fn finish(self) -> Vec<EvidenceFact> {
        let mut facts = vec![if self.lifecycle_order_valid
            && self.saw_created
            && self.items.values().all(|item| item.closed)
            && self.completed_messages == 1
            && self.saw_completed
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
        if self.foreign_protocol {
            facts.push(failed(EvidenceCode::ForeignProtocol));
        }
        facts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextStreamProgress {
    AwaitContentPart,
    AwaitTextDelta,
    TextStreaming,
    AwaitItemDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputItemType {
    Message,
    Reasoning,
    FunctionCall,
}

impl OutputItemType {
    fn from_wire(value: &str) -> Option<Self> {
        match value {
            "message" => Some(Self::Message),
            "reasoning" => Some(Self::Reasoning),
            "function_call" => Some(Self::FunctionCall),
            _ => None,
        }
    }
}

struct OutputItemLifecycle {
    item_type: OutputItemType,
    text_progress: Option<TextStreamProgress>,
    closed: bool,
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

#[cfg(test)]
mod openai_responses_tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::{
        extract::{OriginalUri, State},
        http::{HeaderMap, HeaderValue, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
        Json, Router,
    };
    use serde_json::{json, Value};

    use crate::relay::model_verification::{
        capability_profiles::CapabilityProfile,
        protocols::{
            openai_responses::{
                parse_core_response, parse_stream, parse_structured_response, parse_tool_response,
                run_balanced, run_balanced_with_progress, upstream_model,
            },
            RunFailure, MAX_RESPONSE_BYTES, MAX_SSE_EVENT_BYTES,
        },
        target::ResolvedTarget,
        types::{EvidenceCode, EvidenceFact, EvidenceOutcome},
    };
    use crate::{
        app_config::AppType,
        database::Database,
        provider::{Provider, ProviderMeta},
        relay::model_verification::types::TargetKey,
    };

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

    #[test]
    fn response_envelope_reduces_message_reasoning_and_usage_without_output_text() {
        let facts = parse_core_response(
            br#"{
                "object":"response","status":"completed","model":"gpt-5.6-sol",
                "output":[
                    {"type":"reasoning","summary":[]},
                    {"type":"message","content":[{"type":"output_text","text":"SENTINEL_OUTPUT"}]}
                ],
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}
            }"#,
            "gpt-5.6-sol",
        );

        assert!(facts.contains(&passed(EvidenceCode::BasicEnvelope)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
        assert!(!serde_json::to_string(&facts)
            .unwrap()
            .contains("SENTINEL_OUTPUT"));

        for status in [None, Some("incomplete"), Some("failed")] {
            let status_field = status
                .map(|status| format!("\"status\":\"{status}\","))
                .unwrap_or_default();
            let response = format!(
                "{{\"object\":\"response\",{status_field}\"model\":\"gpt-5.6-sol\",\"output\":[{{\"type\":\"message\",\"content\":[]}}],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}"
            );
            assert!(parse_core_response(response.as_bytes(), "gpt-5.6-sol")
                .contains(&failed(EvidenceCode::BasicEnvelope)));
        }
    }

    #[test]
    fn chat_completions_or_anthropic_shapes_are_foreign_not_success() {
        for response in [
            br#"{"object":"chat.completion","choices":[]}"#.as_slice(),
            br#"{"choices":{}}"#.as_slice(),
            br#"{"type":"message","content":[]}"#.as_slice(),
        ] {
            assert_eq!(
                parse_core_response(response, "gpt-5.6-sol"),
                vec![failed(EvidenceCode::ForeignProtocol)]
            );
        }

        assert_eq!(
            parse_core_response(b"not-json", "gpt-5.6-sol"),
            vec![failed(EvidenceCode::BasicEnvelope)]
        );
    }

    #[test]
    fn upstream_model_reuses_case_insensitive_one_m_normalization() {
        assert_eq!(upstream_model("gpt-5.6-sol[1m]"), "gpt-5.6-sol");
    }

    #[test]
    fn function_and_structured_output_reducers_only_emit_finite_facts() {
        let tool = parse_tool_response(
            br#"{"object":"response","status":"completed","model":"gpt-5.6-sol","output":[{"type":"function_call","name":"report_probe","arguments":"{\"ready\":true}"}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#,
        );
        let structured = parse_structured_response(
            br#"{"object":"response","status":"completed","model":"gpt-5.6-sol","output":[{"type":"message","content":[{"type":"output_text","text":"{\"ready\":true}"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#,
        );

        assert_eq!(tool, passed(EvidenceCode::ToolCallShape));
        assert_eq!(structured, passed(EvidenceCode::StructuredOutput));
        let serialized = serde_json::to_string(&[tool, structured]).unwrap();
        assert!(!serialized.contains("ready"));
        for invalid_payload in [
            "not-json",
            "{\"ready\":false}",
            "{\"ready\":true,\"extra\":1}",
        ] {
            let tool = format!(
                "{{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"output\":[{{\"type\":\"function_call\",\"name\":\"report_probe\",\"arguments\":{invalid_payload:?}}}],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}"
            );
            let structured = format!(
                "{{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"output\":[{{\"type\":\"message\",\"content\":[{{\"type\":\"output_text\",\"text\":{invalid_payload:?}}}]}}],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}"
            );
            assert_eq!(
                parse_tool_response(tool.as_bytes()),
                failed(EvidenceCode::ToolCallShape)
            );
            assert_eq!(
                parse_structured_response(structured.as_bytes()),
                failed(EvidenceCode::StructuredOutput)
            );
        }
        assert_eq!(
            parse_tool_response(
                br#"{"output":[{"type":"function_call","name":"report_probe","arguments":"{}"}]}"#
            ),
            failed(EvidenceCode::ToolCallShape)
        );
        assert_eq!(
            parse_structured_response(
                br#"{"output":[{"type":"message","content":[{"type":"output_text","text":"{}"}]}]}"#
            ),
            failed(EvidenceCode::StructuredOutput)
        );
        assert_eq!(
            parse_structured_response(
                br#"{"object":"response","status":"completed","model":"gpt-5.6-sol","output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#
            ),
            failed(EvidenceCode::StructuredOutput)
        );
    }

    #[test]
    fn foreign_protocol_shapes_from_optional_probes_are_verdict_facts() {
        assert_eq!(
            parse_tool_response(br#"{"object":"chat.completion","choices":[]}"#),
            failed(EvidenceCode::ForeignProtocol)
        );
        assert_eq!(
            parse_structured_response(br#"{"type":"message","content":[]}"#),
            failed(EvidenceCode::ForeignProtocol)
        );
    }

    #[test]
    fn ordered_responses_sse_with_additive_events_completes() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        let stream = concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n",
            "event: response.content_part.added\ndata: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\"}}\n\n",
            "event: response.future_output.delta\ndata: {\"type\":\"response.future_output.delta\",\"delta\":\"ignored\"}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"ready\"}\n\n",
            "event: response.content_part.done\ndata: {\"type\":\"response.content_part.done\"}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n"
        );

        let facts = parse_stream(stream, "gpt-5.6-sol", &profile).unwrap();

        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
        assert!(!serde_json::to_string(&facts)
            .unwrap()
            .contains("SENTINEL_ARGUMENT"));
    }

    #[test]
    fn reasoning_and_function_items_can_precede_the_message_item() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        let stream = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\"}}\n\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"delta\":\"SENTINEL_REASONING\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"SENTINEL_ARGUMENT\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"output_index\":1,\"arguments\":\"SENTINEL_ARGUMENT\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":2,\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.content_part.added\",\"output_index\":2,\"part\":{\"type\":\"output_text\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":2,\"delta\":\"ready\"}\n\n",
            "data: {\"type\":\"response.output_text.done\",\"output_index\":2,\"text\":\"ready\"}\n\n",
            "data: {\"type\":\"response.content_part.done\",\"output_index\":2,\"part\":{\"type\":\"output_text\"}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":2,\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n"
        );

        let facts = parse_stream(stream, "gpt-5.6-sol", &profile).unwrap();

        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
        let serialized = serde_json::to_string(&facts).unwrap();
        assert!(!serialized.contains("SENTINEL"));
    }

    #[test]
    fn missing_or_early_terminal_cannot_satisfy_stream_lifecycle() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        for stream in [
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"gpt-5.6-sol\"}}\n\n",
                "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n"
            ),
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"gpt-5.6-sol\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n"
            ),
        ] {
            let facts = parse_stream(stream, "gpt-5.6-sol", &profile).unwrap();
            assert!(facts.contains(&failed(EvidenceCode::StreamLifecycle)));
        }
    }

    #[test]
    fn failed_stream_cannot_be_followed_by_a_passing_completion() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        let stream = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ready\"}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"object\":\"response\",\"status\":\"failed\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "data: {\"type\":\"response.content_part.done\"}\n\n",
            "data: {\"type\":\"response.output_item.done\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        );

        assert_eq!(
            parse_stream(stream, "gpt-5.6-sol", &profile),
            Err(RunFailure::InvalidResponse)
        );
    }

    #[test]
    fn incomplete_stream_cannot_be_followed_by_a_passing_completion() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        let stream = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ready\"}\n\n",
            "data: {\"type\":\"response.incomplete\",\"response\":{\"object\":\"response\",\"status\":\"incomplete\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "data: {\"type\":\"response.content_part.done\"}\n\n",
            "data: {\"type\":\"response.output_item.done\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        );

        assert_eq!(
            parse_stream(stream, "gpt-5.6-sol", &profile),
            Err(RunFailure::InvalidResponse)
        );
    }

    #[test]
    fn unknown_or_out_of_order_events_cannot_satisfy_fixed_text_stream_lifecycle() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        for stream in [
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"gpt-5.6-sol\",\"status\":\"in_progress\"}}\n\n",
                "data: {\"type\":\"response.future.added\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
            ),
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"gpt-5.6-sol\",\"status\":\"in_progress\"}}\n\n",
                "data: {\"type\":\"response.content_part.added\"}\n\n",
                "data: {\"type\":\"response.output_item.added\"}\n\n",
                "data: {\"type\":\"response.output_text.delta\"}\n\n",
                "data: {\"type\":\"response.content_part.done\"}\n\n",
                "data: {\"type\":\"response.output_item.done\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
            ),
        ] {
            let facts = parse_stream(stream, "gpt-5.6-sol", &profile).unwrap();
            assert!(facts.contains(&failed(EvidenceCode::StreamLifecycle)));
        }
    }

    #[test]
    fn terminal_response_requires_completed_status() {
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        for status in [None, Some("incomplete"), Some("failed")] {
            let status_field = status
                .map(|status| format!("\"status\":\"{status}\","))
                .unwrap_or_default();
            let stream = format!(
                concat!(
                    "data: {{\"type\":\"response.created\",\"response\":{{\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.6-sol\"}}}}\n\n",
                    "data: {{\"type\":\"response.output_item.added\",\"item\":{{\"type\":\"message\"}}}}\n\n",
                    "data: {{\"type\":\"response.content_part.added\",\"part\":{{\"type\":\"output_text\"}}}}\n\n",
                    "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"ready\"}}\n\n",
                    "data: {{\"type\":\"response.content_part.done\"}}\n\n",
                    "data: {{\"type\":\"response.output_item.done\"}}\n\n",
                    "data: {{\"type\":\"response.completed\",\"response\":{{\"object\":\"response\",{status_field}\"model\":\"gpt-5.6-sol\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
                ),
                status_field = status_field
            );
            let facts = parse_stream(&stream, "gpt-5.6-sol", &profile).unwrap();
            assert!(facts.contains(&failed(EvidenceCode::StreamLifecycle)));
        }
    }

    #[tokio::test]
    async fn balanced_probe_uses_responses_contract_and_discards_private_values() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/responses", post(happy_handler))
                .with_state(requests.clone()),
        )
        .await;
        let target = target_for(&endpoint, "SENTINEL_API_KEY");
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();

        let mut completed_probes = 0;
        let facts = run_balanced_with_progress(&client, &target, &profile, &mut || {
            completed_probes += 1;
        })
        .await
        .unwrap();

        for code in [
            EvidenceCode::BasicEnvelope,
            EvidenceCode::ModelMatch,
            EvidenceCode::UsageConsistency,
            EvidenceCode::ToolCallShape,
            EvidenceCode::StructuredOutput,
            EvidenceCode::StreamLifecycle,
        ] {
            assert!(facts.contains(&passed(code)));
        }
        let serialized = serde_json::to_string(&facts).unwrap();
        assert!(!serialized.contains("SENTINEL"));
        assert_eq!(completed_probes, 4);

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert_eq!(request.path, "/v1/responses");
            assert_eq!(
                request.authorization.as_ref().unwrap(),
                "Bearer SENTINEL_API_KEY"
            );
            assert_eq!(request.content_type.as_ref().unwrap(), "application/json");
            assert_eq!(request.body["store"], false);
            assert_eq!(request.body["reasoning"], json!({"effort":"low"}));
        }
        assert_eq!(requests[0].body["max_output_tokens"], 512);
        assert_eq!(requests[1].body["max_output_tokens"], 1024);
        assert_eq!(requests[2].body["max_output_tokens"], 1024);
        assert_eq!(requests[3].body["max_output_tokens"], 512);
        for request in &requests[..3] {
            assert_eq!(request.accept.as_ref().unwrap(), "application/json");
        }
        assert_eq!(requests[1].body["tools"][0]["type"], "function");
        assert_eq!(requests[1].body["tools"][0]["name"], "report_probe");
        assert_eq!(requests[1].body["tools"][0]["strict"], true);
        assert_eq!(
            requests[1].body["tool_choice"],
            json!({"type":"function","name":"report_probe"})
        );
        assert_eq!(requests[2].body["text"]["format"]["type"], "json_schema");
        assert_eq!(
            requests[2].body["text"]["format"]["name"],
            "loongport_probe"
        );
        assert_eq!(requests[2].body["text"]["format"]["strict"], true);
        assert_eq!(requests[3].accept.as_ref().unwrap(), "text/event-stream");
        assert_eq!(requests[3].body["stream"], true);
    }

    #[tokio::test]
    async fn unknown_models_do_not_receive_unestablished_reasoning_fields() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/responses", post(happy_handler))
                .with_state(requests.clone()),
        )
        .await;
        let profile = CapabilityProfile::for_target(&AppType::Codex, "future-model-x");

        run_balanced(
            &reqwest::Client::new(),
            &target_for_model(&endpoint, "key", "future-model-x"),
            &profile,
        )
        .await
        .unwrap();

        assert!(requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.body.get("reasoning").is_none()));
    }

    #[tokio::test]
    async fn incomplete_success_response_is_a_finite_run_failure() {
        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(|| async {
                Json(json!({
                    "object":"response",
                    "status":"incomplete",
                    "incomplete_details":{"reason":"max_output_tokens"},
                    "model":"gpt-5.6-sol",
                    "output":[]
                }))
            }),
        ))
        .await;

        let result = run_balanced(
            &reqwest::Client::new(),
            &target_for_model(&endpoint, "key", "gpt-5.6-sol"),
            &CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol"),
        )
        .await;

        assert_eq!(result, Err(RunFailure::InvalidResponse));
    }

    #[tokio::test]
    async fn http_failures_are_finite_and_do_not_include_response_text() {
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
                RunFailure::Upstream { status: 502 },
            ),
        ] {
            let endpoint = spawn_server(
                Router::new().route("/v1/responses", post(move || async move { (status, body) })),
            )
            .await;
            let error = run_balanced(
                &reqwest::Client::new(),
                &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
                &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
            )
            .await
            .unwrap_err();

            assert_eq!(error, expected);
            assert!(!format!("{error:?}").contains("SENTINEL"));
        }
    }

    #[tokio::test]
    async fn timeout_and_oversized_responses_stop_without_leaking_values() {
        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(|| async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                normal_response(&json!({}))
            }),
        ))
        .await;
        let short_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1))
            .build()
            .unwrap();
        assert_eq!(
            run_balanced(
                &short_client,
                &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
                &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
            )
            .await
            .unwrap_err(),
            RunFailure::Timeout
        );

        let large_body = format!("SENTINEL_BODY{}", "x".repeat(MAX_RESPONSE_BYTES));
        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(move || {
                let large_body = large_body.clone();
                async move { large_body }
            }),
        ))
        .await;
        assert_eq!(
            run_balanced(
                &reqwest::Client::new(),
                &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
                &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
            )
            .await
            .unwrap_err(),
            RunFailure::ResponseTooLarge
        );
    }

    #[tokio::test]
    async fn malformed_foreign_and_missing_terminal_are_reduced_to_finite_evidence() {
        let calls = Arc::new(Mutex::new(0usize));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/responses", post(foreign_then_complete_handler))
                .with_state(calls),
        )
        .await;
        let facts = run_balanced(
            &reqwest::Client::new(),
            &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
            &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
        )
        .await
        .unwrap();
        assert!(facts.contains(&failed(EvidenceCode::ForeignProtocol)));
        assert!(!serde_json::to_string(&facts).unwrap().contains("SENTINEL"));

        let oversized_event = format!(
            "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{}\"}}\n\n",
            "x".repeat(MAX_SSE_EVENT_BYTES)
        );
        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(move |Json(body): Json<Value>| {
                let oversized_event = oversized_event.clone();
                async move {
                    if body["stream"] == true {
                        return ([("content-type", "text/event-stream")], oversized_event)
                            .into_response();
                    }
                    normal_response(&body)
                }
            }),
        ))
        .await;
        assert_eq!(
            run_balanced(
                &reqwest::Client::new(),
                &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
                &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
            )
            .await
            .unwrap_err(),
            RunFailure::ResponseTooLarge
        );

        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(|Json(body): Json<Value>| async move {
                if body["stream"] == true {
                    return ([("content-type", "text/event-stream")], concat!(
                        "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"future-model-x\"}}\n\n",
                        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n"
                    )).into_response();
                }
                normal_response(&body)
            }),
        ))
        .await;
        let facts = run_balanced(
            &reqwest::Client::new(),
            &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
            &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
        )
        .await
        .unwrap();
        assert!(facts.contains(&failed(EvidenceCode::StreamLifecycle)));
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        path: String,
        authorization: Option<HeaderValue>,
        content_type: Option<HeaderValue>,
        accept: Option<HeaderValue>,
        body: Value,
    }

    type Requests = Arc<Mutex<Vec<RecordedRequest>>>;

    async fn spawn_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    fn target_for(endpoint: &str, api_key: &str) -> ResolvedTarget {
        target_for_model(endpoint, api_key, "gpt-5.6-sol")
    }

    fn target_for_model(endpoint: &str, api_key: &str, model: &str) -> ResolvedTarget {
        let db = Database::memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO loongport_relay (site_origin, site_name, api_base_url, account_id, account_label, login_identifier, auth_token, sort_index) VALUES (?1, 'Test', ?1, 7, 'test', 'test', 'token', 0)",
                [endpoint],
            )
            .unwrap();
        }
        db.save_provider(
            "codex",
            &Provider {
                id: "loongport-0123456789abcdef".into(),
                name: "Test tier".into(),
                settings_config: json!({"auth": {"OPENAI_API_KEY": api_key}}),
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
            TargetKey::new("loongport-0123456789abcdef", "codex", model),
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
            authorization: headers.get("authorization").cloned(),
            content_type: headers.get("content-type").cloned(),
            accept: headers.get("accept").cloned(),
            body: body.clone(),
        });
        if body["stream"] == true {
            return ([("content-type", "text/event-stream")], happy_stream()).into_response();
        }
        if body.get("tools").is_some() {
            return Json(json!({
                "object":"response", "status":"completed", "model":"gpt-5.6-sol",
                "output":[{"type":"function_call","name":"report_probe","arguments":"{\"ready\":true}"}],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }))
            .into_response();
        }
        if body.get("text").is_some() {
            return Json(json!({
                "object":"response", "status":"completed", "model":"gpt-5.6-sol",
                "output":[{"type":"message","content":[{"type":"output_text","text":"{\"ready\":true}"}]}],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }))
            .into_response();
        }
        Json(json!({
            "object":"response", "status":"completed", "model":"gpt-5.6-sol",
            "output":[{"type":"message","content":[{"type":"output_text","text":"SENTINEL_OUTPUT"}]}],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }))
        .into_response()
    }

    async fn foreign_then_complete_handler(
        State(calls): State<Arc<Mutex<usize>>>,
        Json(body): Json<Value>,
    ) -> Response {
        let mut calls = calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            return Json(json!({
                "object": "chat.completion",
                "choices": [],
                "text": "SENTINEL_FOREIGN"
            }))
            .into_response();
        }
        normal_response(&body)
    }

    fn normal_response(body: &Value) -> Response {
        if body.get("tools").is_some() {
            return Json(json!({
                "object":"response", "status":"completed", "model":"future-model-x",
                "output":[{"type":"function_call","name":"report_probe","arguments":"{\"ready\":true}"}],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }))
            .into_response();
        }
        if body["stream"] == true {
            return (
                [("content-type", "text/event-stream")],
                future_model_stream(),
            )
                .into_response();
        }
        Json(json!({
            "object":"response", "status":"completed", "model":"future-model-x",
            "output":[{"type":"message","content":[{"type":"output_text","text":"SENTINEL_OUTPUT"}]}],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }))
        .into_response()
    }

    fn happy_stream() -> String {
        concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"gpt-5.6-sol\"}}\n\n",
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n",
            "event: response.content_part.added\ndata: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\"}}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"SENTINEL_STREAM_TEXT\"}\n\n",
            "event: response.content_part.done\ndata: {\"type\":\"response.content_part.done\"}\n\n",
            "event: response.output_item.done\ndata: {\"type\":\"response.output_item.done\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        )
        .into()
    }

    fn future_model_stream() -> String {
        concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"status\":\"in_progress\",\"model\":\"future-model-x\"}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.content_part.added\",\"part\":{\"type\":\"output_text\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ready\"}\n\n",
            "data: {\"type\":\"response.content_part.done\"}\n\n",
            "data: {\"type\":\"response.output_item.done\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        )
        .into()
    }
}
