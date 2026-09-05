use std::collections::{BTreeMap, HashMap};

use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use serde_json::{json, Value};

use crate::proxy::model_mapper::strip_one_m_suffix_for_upstream;
use crate::relay::model_verification::{
    capability_profiles::CapabilityProfile,
    protocols::{
        capture_leg_diagnostics, model_matches_protocol_family, models_match, record_event_types,
        send_and_read, send_sse, RunFailure,
    },
    target::ResolvedTarget,
    types::{EvidenceCode, EvidenceFact, EvidenceOutcome, ProbeDiagnostic},
};

const REPORT_PROBE: &str = "report_probe";
const CORE_STREAM_OUTPUT_TOKENS: u16 = 512;
const TOOL_STRUCTURED_OUTPUT_TOKENS: u16 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ProbeKind {
    Core,
    Tool,
    Structured,
    Stream,
}

impl ProbeKind {
    fn name(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Tool => "tool",
            Self::Structured => "structured",
            Self::Stream => "stream",
        }
    }
}

pub(crate) async fn run_balanced(
    client: &reqwest::Client,
    target: &ResolvedTarget,
    profile: &CapabilityProfile,
) -> Result<(Vec<EvidenceFact>, Vec<ProbeDiagnostic>), RunFailure> {
    run_balanced_with_progress(client, target, profile, &mut || {}).await
}

pub(crate) async fn run_balanced_with_progress(
    client: &reqwest::Client,
    target: &ResolvedTarget,
    profile: &CapabilityProfile,
    on_probe_complete: &mut impl FnMut(),
) -> Result<(Vec<EvidenceFact>, Vec<ProbeDiagnostic>), RunFailure> {
    let model = upstream_model(&target.target().model);
    let endpoint = format!("{}/responses", target.protocol_base().trim_end_matches('/'));
    let api_key = target.api_key();
    // 各腿带回 (请求体, 响应体)：诊断边车只在事实 Failed 时采集；流式腿的
    // 「响应体」是事件类型序列（有界），非流式腿是原始响应。
    let mut probes = FuturesUnordered::new();
    probes.push(
        async {
            let request = core_request(&model, profile);
            let result = async {
                let body = send_response(client, &endpoint, api_key, request.clone()).await?;
                reject_incomplete_response(&body)?;
                Ok((parse_core_response(&body, &model), body))
            }
            .await;
            let (result, raw) = unzip_probe_result(result);
            (ProbeKind::Core, result, request, raw)
        }
        .boxed(),
    );
    probes.push(
        async {
            let request = tool_request(&model, profile);
            let result = async {
                let body = send_response(client, &endpoint, api_key, request.clone()).await?;
                reject_incomplete_response(&body)?;
                Ok((vec![parse_tool_response(&body, &model)], body))
            }
            .await;
            let (result, raw) = unzip_probe_result(result);
            (ProbeKind::Tool, result, request, raw)
        }
        .boxed(),
    );
    if profile.supports_structured_output {
        probes.push(
            async {
                let request = structured_output_request(&model, profile);
                let result = async {
                    let body = send_response(client, &endpoint, api_key, request.clone()).await?;
                    reject_incomplete_response(&body)?;
                    Ok((vec![parse_structured_response(&body)], body))
                }
                .await;
                let (result, raw) = unzip_probe_result(result);
                (ProbeKind::Structured, result, request, raw)
            }
            .boxed(),
        );
    }
    probes.push(
        async {
            let request = stream_request(&model, profile);
            // 诊断用：流事件类型序列（有界），随结果一起带回。
            let mut event_log: Vec<String> = Vec::new();
            let result = async {
                // 无 response.completed = 流被截断（干净 EOF 中途收尾），按错误
                // 重试一次，而不是判「流式生命周期未通过」。
                let mut terminal = None;
                for _ in 0..2 {
                    event_log.clear();
                    let state = send_stream(
                        client,
                        &endpoint,
                        api_key,
                        request.clone(),
                        || StreamReducer::new(&model),
                        |stream, event| {
                            record_event_types(&mut event_log, event);
                            stream.observe(event)
                        },
                    )
                    .await?;
                    if state.saw_completed {
                        terminal = Some(state);
                        break;
                    }
                }
                let state = terminal.ok_or(RunFailure::InvalidResponse)?;
                Ok(state.finish())
            }
            .await;
            let raw = result
                .as_ref()
                .ok()
                .map(|_| event_log.join(" → ").into_bytes())
                .unwrap_or_default();
            (ProbeKind::Stream, result, request, raw)
        }
        .boxed(),
    );

    let mut results = BTreeMap::new();
    let mut diagnostics: Vec<ProbeDiagnostic> = Vec::new();
    while let Some((probe, result, request, raw_body)) = probes.next().await {
        on_probe_complete();
        if let Err(failure) = &result {
            log::warn!(
                "[model-verification] responses probe failed: probe={} failure={failure:?}",
                probe.name()
            );
        }
        if let Ok(facts) = &result {
            capture_leg_diagnostics(&mut diagnostics, probe.name(), facts, &request, &raw_body);
        }
        results.insert(probe, result);
    }

    let mut facts = Vec::new();
    for result in results.into_values() {
        match result {
            Ok(probe_facts) => facts.extend(probe_facts),
            Err(failure) => return Err(failure),
        }
    }
    Ok((facts, diagnostics))
}

/// 拆 (事实, 原始响应)：失败时原始响应丢弃（该腿整轮已失败，诊断对
/// RunFailure 不适用——错误分类本身就会展示给用户）。
fn unzip_probe_result(
    result: Result<(Vec<EvidenceFact>, Vec<u8>), RunFailure>,
) -> (Result<Vec<EvidenceFact>, RunFailure>, Vec<u8>) {
    match result {
        Ok((facts, body)) => (Ok(facts), body),
        Err(failure) => (Err(failure), Vec::new()),
    }
}

async fn send_response(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    payload: Value,
) -> Result<Vec<u8>, RunFailure> {
    let body = send_and_read(
        client
            .post(endpoint)
            .bearer_auth(api_key)
            .header("accept", "application/json")
            .json(&payload),
    )
    .await?;
    // 部分网关对非流式请求也回 SSE（订阅号池实测高发）。SSE 不是换芯信号，
    // 归一取出 terminal response.completed 里的 response 对象照常评估；
    // 无终端事件视为截断错误。
    if body_looks_like_sse(&body) {
        let terminal = sse_terminal_response(&body).ok_or(RunFailure::InvalidResponse)?;
        serde_json::to_vec(&terminal).map_err(|_| RunFailure::InvalidResponse)
    } else {
        Ok(body)
    }
}

fn body_looks_like_sse(body: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&body[..body.len().min(256)]);
    // SSE 注释行（": keepalive"）也是流的开头形态，逐行看前几行。
    head.lines().take(4).any(|line| {
        let line = line.trim_start();
        line.starts_with("event:") || line.starts_with("data:")
    })
}

/// 从 SSE 体里取 `response.completed` 事件的 response 对象。
///
/// 部分网关在 `store:false` 下的 terminal 快照**不带 output 项**（实测
/// 空 `output: []`）：此时用事件流重建——`output_item.added` 的 item 骨架 +
/// `output_text.delta` / `function_call_arguments.delta` 的增量累积。
fn sse_terminal_response(body: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(body).ok()?;
    let mut terminal: Option<Value> = None;
    let mut items: Vec<(u64, Value)> = Vec::new();
    let mut deltas: HashMap<u64, String> = HashMap::new();
    let mut item_id_to_index: HashMap<String, u64> = HashMap::new();
    for chunk in text.split("\n\n") {
        let data = chunk
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("response.completed") => terminal = value.get("response").cloned(),
            Some("response.output_item.added") => {
                if let (Some(index), Some(item)) = (
                    value.get("output_index").and_then(Value::as_u64),
                    value.get("item").cloned(),
                ) {
                    if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                        item_id_to_index.insert(item_id.to_string(), index);
                    }
                    // 流内重启会重放同 index 的 item，后到的为准。
                    items.retain(|(existing, _)| *existing != index);
                    items.push((index, item));
                }
            }
            Some("response.output_text.delta" | "response.function_call_arguments.delta") => {
                let index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .or_else(|| {
                        value.get("item_id").and_then(Value::as_str).and_then(|id| {
                            item_id_to_index
                                .get(id)
                                .copied()
                                .or_else(|| id.parse::<u64>().ok())
                        })
                    });
                if let (Some(index), Some(delta)) =
                    (index, value.get("delta").and_then(Value::as_str))
                {
                    deltas.entry(index).or_default().push_str(delta);
                }
            }
            _ => {}
        }
    }
    let mut terminal = terminal?;
    let output_empty = terminal
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    if output_empty && !items.is_empty() {
        let rebuilt: Vec<Value> = items
            .into_iter()
            .map(|(index, mut item)| {
                if let Some(accumulated) = deltas.get(&index) {
                    match item.get("type").and_then(Value::as_str) {
                        Some("message") => {
                            item["content"] = json!([{
                                "type": "output_text",
                                "text": accumulated,
                            }]);
                        }
                        Some("function_call") => {
                            item["arguments"] = Value::String(accumulated.clone());
                        }
                        _ => {}
                    }
                }
                item
            })
            .collect();
        terminal["output"] = Value::Array(rebuilt);
    }
    Some(terminal)
}

async fn send_stream<State>(
    client: &reqwest::Client,
    endpoint: &str,
    api_key: &str,
    payload: Value,
    new_state: impl FnMut() -> State,
    on_event: impl FnMut(&mut State, &str) -> Result<(), RunFailure>,
) -> Result<State, RunFailure> {
    send_sse(
        client
            .post(endpoint)
            .bearer_auth(api_key)
            .header("accept", "text/event-stream")
            .json(&payload),
        new_state,
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
        facts.push(if models_match(expected_model, model) {
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

pub(crate) fn parse_tool_response(body: &[u8], expected_model: &str) -> EvidenceFact {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return failed(EvidenceCode::ToolCallShape);
    };
    if has_foreign_protocol_fingerprint(&response) {
        return failed(EvidenceCode::ForeignProtocol);
    }
    if !is_response_envelope(&response) {
        return failed(EvidenceCode::ToolCallShape);
    }
    // 跨家族网关（如 claude 模型走 codex 协议）在做跨协议转换：不保证强制
    // 函数调用、也不保证调用形状完整。跨家族一律不因工具形状 Failed——
    // 有合法调用记 Passed，否则 Skipped；只有同家族才严格判定。
    let cross_family = !model_matches_protocol_family(expected_model, true);
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
    } else if cross_family {
        skipped(EvidenceCode::ToolCallShape)
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
    let (Some(input), Some(output)) = (
        usage.get("input_tokens").and_then(Value::as_u64),
        usage.get("output_tokens").and_then(Value::as_u64),
    ) else {
        return false;
    };
    // total_tokens 缺失或为 null 是网关常态：input+output 齐全即可，
    // 只有 total 明确给出且对不上才算虚报。
    match usage.get("total_tokens").and_then(Value::as_u64) {
        Some(total) => input.saturating_add(output) == total,
        None => true,
    }
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
    pub(crate) saw_completed: bool,
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

    // collapsible_match 建议把失败条件折进 match guard，但这些条件要调
    // &mut self 的状态推进（guard 不可变借用），折进去编译不过。
    #[allow(clippy::collapsible_match)]
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

        // 生命周期窗口之外的事件（created 前 / completed 后的网关泄漏，
        // 实测有网关把 output_text.delta 漏在 created 之前）直接忽略：
        // 它们不参与响应内容，记违规只会制造误报。
        if !self.in_active_response()
            && value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("response.") && kind != "response.created")
        {
            return Ok(());
        }

        match value.get("type").and_then(Value::as_str) {
            Some("response.created") => {
                if self.saw_created {
                    // 流中间再次出现 created = 网关在流内做了上游故障转移并
                    // 重放事件头（实测订阅号池高发）。重启段才是最终响应，
                    // 重置状态机接着收，而不是记违规。
                    self.items.clear();
                    self.completed_messages = 0;
                    self.saw_completed = false;
                }
                self.saw_created = true;
                self.observe_response(value.get("response"), false);
            }
            Some("response.completed") => {
                // 快照式流（OpenRouter 免费模型常见）：created→completed 之间
                // 零增量事件，答案全在 completed 载荷里——从载荷的 output 数
                // message 项补足计数。
                if self.completed_messages == 0 {
                    self.completed_messages = value
                        .pointer("/response/output")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .filter(|item| {
                                    item.get("type").and_then(Value::as_str) == Some("message")
                                })
                                .count() as u8
                        })
                        .unwrap_or(0);
                }
                // 多条 message 输出项是合法形状（网关分片常见）：要求至少一条。
                if !self.saw_created
                    || self.items.values().any(|item| !item.closed)
                    || self.completed_messages < 1
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
            // 未知事件类型 = 网关注解（codex.rate_limits / responsesapi.* 等，
            // 实测多家都有）：不是协议事件，忽略。异协议另有指纹与 message_
            // 前缀两道闸，这里不需要第三道。
            _ => {}
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
        // 部分网关对多条 message 不递增 output_index（每条都用 0）：旧 item
        // 已关闭时允许同 index 顶替开启新 item，只有「顶替未关闭项」才是违规。
        if !self.in_active_response() || self.items.get(&index).is_some_and(|item| !item.closed) {
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
        // created 事件的 response 载荷缺失或为 null 是网关常见形状（纯公告，
        // 模型与用量信息后面 completed 会带）；completed 的载荷才是必需。
        let Some(response) = response.filter(|value| !value.is_null()) else {
            if terminal {
                self.lifecycle_order_valid = false;
            }
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
            self.model_matches = Some(models_match(self.expected_model, model));
        }
        // usage 只在 terminal（response.completed）评估：created 事件里的
        // usage 常是空对象（实测网关常态），在那上面要求字段齐全必误报。
        if terminal && response.get("usage").is_some() {
            self.saw_usage = true;
            self.usage_consistent &= usage_is_consistent(response.get("usage"));
        }
    }

    fn finish(self) -> Vec<EvidenceFact> {
        let mut facts = vec![if self.lifecycle_order_valid
            && self.saw_created
            && self.items.values().all(|item| item.closed)
            && self.completed_messages >= 1
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

fn skipped(code: EvidenceCode) -> EvidenceFact {
    EvidenceFact {
        code,
        outcome: EvidenceOutcome::Skipped,
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
    use tokio::sync::Barrier;

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
                "object":"response","id":"resp_test000000000000","status":"completed","model":"gpt-5.6-sol",
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

    /// F2 回归：部分网关对非流式请求也回 SSE（订阅号池实测高发）。
    /// 归一取出 terminal response.completed 后照常评估，不再三连误判。
    #[tokio::test]
    async fn sse_answer_to_non_stream_request_is_normalized_to_terminal_response() {
        let sse_body = concat!(
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"future-model-x\"}}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{",
            "\"object\":\"response\",\"id\":\"resp_test000000000000\",\"status\":\"completed\",\"model\":\"future-model-x\",",
            "\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"ready\"}]}],",
            "\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        );
        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(move |Json(body): Json<Value>| async move {
                let plain_core = body["stream"] != true
                    && body.get("tools").is_none()
                    && body.get("text").is_none();
                if plain_core {
                    return ([("content-type", "text/event-stream")], sse_body).into_response();
                }
                normal_response(&body)
            }),
        ))
        .await;
        let (facts, _) = run_balanced(
            &reqwest::Client::new(),
            &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
            &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
        )
        .await
        .unwrap();
        assert!(facts.contains(&passed(EvidenceCode::BasicEnvelope)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
        assert!(facts.contains(&passed(EvidenceCode::ToolCallShape)));
        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
    }

    /// 回放聚合网关免费模型的实测序列（reasoning summary 事件 + 单 message），
    /// 变体覆盖 created 事件的 response 载荷三种形状。
    #[test]
    fn reasoning_summary_streams_and_null_created_payload_reduce_cleanly() {
        let seq = |created_payload: &str| {
            format!(
                concat!(
                    "event: response.created\ndata: {}\n\n",
                    "event: response.output_item.added\ndata: {{\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{{\"type\":\"reasoning\"}}}}\n\n",
                    "event: response.reasoning_summary_part.added\ndata: {{\"type\":\"response.reasoning_summary_part.added\",\"output_index\":0,\"summary_index\":0,\"part\":{{\"type\":\"summary_text\"}}}}\n\n",
                    "event: response.reasoning_summary_text.delta\ndata: {{\"type\":\"response.reasoning_summary_text.delta\",\"output_index\":0,\"summary_index\":0,\"delta\":\"thinking\"}}\n\n",
                    "event: response.reasoning_summary_text.done\ndata: {{\"type\":\"response.reasoning_summary_text.done\",\"output_index\":0,\"summary_index\":0,\"text\":\"thinking\"}}\n\n",
                    "event: response.reasoning_summary_part.done\ndata: {{\"type\":\"response.reasoning_summary_part.done\",\"output_index\":0,\"summary_index\":0,\"part\":{{\"type\":\"summary_text\"}}}}\n\n",
                    "event: response.output_item.done\ndata: {{\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{{\"type\":\"reasoning\"}}}}\n\n",
                    "event: response.output_item.added\ndata: {{\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{{\"type\":\"message\"}}}}\n\n",
                    "event: response.content_part.added\ndata: {{\"type\":\"response.content_part.added\",\"output_index\":1,\"part\":{{\"type\":\"output_text\"}}}}\n\n",
                    "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"output_index\":1,\"delta\":\"stream\"}}\n\n",
                    "event: response.output_text.done\ndata: {{\"type\":\"response.output_text.done\",\"output_index\":1,\"text\":\"stream\"}}\n\n",
                    "event: response.content_part.done\ndata: {{\"type\":\"response.content_part.done\",\"output_index\":1,\"part\":{{\"type\":\"output_text\"}}}}\n\n",
                    "event: response.output_item.done\ndata: {{\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{{\"type\":\"message\"}}}}\n\n",
                    "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"object\":\"response\",\"status\":\"completed\",\"model\":\"future-model-x\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
                ),
                created_payload
            )
        };
        let profile = CapabilityProfile::for_target(&AppType::Codex, "future-model-x");
        let full_payload = r#"{"type":"response.created","response":{"object":"response","model":"future-model-x"}}"#;
        let facts = parse_stream(&seq(full_payload), "future-model-x", &profile).unwrap();
        assert!(
            facts.contains(&passed(EvidenceCode::StreamLifecycle)),
            "full payload: {facts:?}"
        );

        let null_payload = r#"{"type":"response.created","response":null}"#;
        let facts = parse_stream(&seq(null_payload), "future-model-x", &profile).unwrap();
        assert!(
            facts.contains(&passed(EvidenceCode::StreamLifecycle)),
            "null payload 应容忍: {facts:?}"
        );

        let missing_payload = r#"{"type":"response.created"}"#;
        let facts = parse_stream(&seq(missing_payload), "future-model-x", &profile).unwrap();
        assert!(
            facts.contains(&passed(EvidenceCode::StreamLifecycle)),
            "缺失 response 字段应容忍: {facts:?}"
        );
    }

    fn upstream_model_reuses_case_insensitive_one_m_normalization() {
        assert_eq!(upstream_model("gpt-5.6-sol[1m]"), "gpt-5.6-sol");
    }

    #[test]
    fn function_and_structured_output_reducers_only_emit_finite_facts() {
        let tool = parse_tool_response(
            br#"{"object":"response","id":"resp_test000000000000","status":"completed","model":"gpt-5.6-sol","output":[{"type":"function_call","name":"report_probe","arguments":"{\"ready\":true}"}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#,
            "gpt-5.6-sol",
        );
        let structured = parse_structured_response(
            br#"{"object":"response","id":"resp_test000000000000","status":"completed","model":"gpt-5.6-sol","output":[{"type":"message","content":[{"type":"output_text","text":"{\"ready\":true}"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#,
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
                parse_tool_response(tool.as_bytes(), "gpt-5.6-sol"),
                failed(EvidenceCode::ToolCallShape)
            );
            assert_eq!(
                parse_structured_response(structured.as_bytes()),
                failed(EvidenceCode::StructuredOutput)
            );
        }
        assert_eq!(
            parse_tool_response(
                br#"{"output":[{"type":"function_call","name":"report_probe","arguments":"{}"}]}"#,
                "gpt-5.6-sol",
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
                br#"{"object":"response","id":"resp_test000000000000","status":"completed","model":"gpt-5.6-sol","output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#
            ),
            failed(EvidenceCode::StructuredOutput)
        );
    }

    #[test]
    fn foreign_protocol_shapes_from_optional_probes_are_verdict_facts() {
        assert_eq!(
            parse_tool_response(
                br#"{"object":"chat.completion","choices":[]}"#,
                "gpt-5.6-sol"
            ),
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
        let (facts, _diagnostics) =
            run_balanced_with_progress(&client, &target, &profile, &mut || {
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
        let core = requests
            .iter()
            .find(|request| {
                request.body["stream"] != true
                    && request.body.get("tools").is_none()
                    && request.body.get("text").is_none()
            })
            .unwrap();
        let tool = requests
            .iter()
            .find(|request| request.body.get("tools").is_some())
            .unwrap();
        let structured = requests
            .iter()
            .find(|request| request.body.get("text").is_some())
            .unwrap();
        let stream = requests
            .iter()
            .find(|request| request.body["stream"] == true)
            .unwrap();
        assert_eq!(core.body["max_output_tokens"], 512);
        assert_eq!(tool.body["max_output_tokens"], 1024);
        assert_eq!(structured.body["max_output_tokens"], 1024);
        assert_eq!(stream.body["max_output_tokens"], 512);
        for request in requests
            .iter()
            .filter(|request| request.body["stream"] != true)
        {
            assert_eq!(request.accept.as_ref().unwrap(), "application/json");
        }
        assert_eq!(tool.body["tools"][0]["type"], "function");
        assert_eq!(tool.body["tools"][0]["name"], "report_probe");
        assert_eq!(tool.body["tools"][0]["strict"], true);
        assert_eq!(
            tool.body["tool_choice"],
            json!({"type":"function","name":"report_probe"})
        );
        assert_eq!(structured.body["text"]["format"]["type"], "json_schema");
        assert_eq!(structured.body["text"]["format"]["name"], "loongport_probe");
        assert_eq!(structured.body["text"]["format"]["strict"], true);
        assert_eq!(stream.accept.as_ref().unwrap(), "text/event-stream");
        assert_eq!(stream.body["stream"], true);
    }

    #[tokio::test]
    async fn balanced_probe_runs_independent_requests_concurrently() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/responses", post(barrier_happy_handler))
                .with_state(ConcurrentState {
                    requests,
                    barrier: Arc::new(Barrier::new(4)),
                }),
        )
        .await;
        let profile = CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol");

        tokio::time::timeout(
            Duration::from_secs(2),
            run_balanced(
                &reqwest::Client::new(),
                &target_for(&endpoint, "key"),
                &profile,
            ),
        )
        .await
        .expect("four independent probes should run concurrently")
        .unwrap();
    }

    #[tokio::test]
    async fn balanced_probe_reduces_results_in_protocol_order() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let endpoint = spawn_server(
            Router::new()
                .route("/v1/responses", post(delayed_happy_handler))
                .with_state(requests),
        )
        .await;
        let (facts, _) = run_balanced(
            &reqwest::Client::new(),
            &target_for(&endpoint, "key"),
            &CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol"),
        )
        .await
        .unwrap();

        assert_eq!(
            facts,
            [
                passed(EvidenceCode::BasicEnvelope),
                passed(EvidenceCode::ModelMatch),
                passed(EvidenceCode::UsageConsistency),
                passed(EvidenceCode::ToolCallShape),
                passed(EvidenceCode::StructuredOutput),
                passed(EvidenceCode::StreamLifecycle),
                passed(EvidenceCode::ModelMatch),
                passed(EvidenceCode::UsageConsistency),
            ]
        );
    }

    #[tokio::test]
    async fn balanced_probe_returns_first_failure_in_protocol_order() {
        let endpoint = spawn_server(Router::new().route(
            "/v1/responses",
            post(|Json(body): Json<Value>| async move {
                if body["stream"] == true {
                    return (
                        StatusCode::OK,
                        [("content-type", "text/event-stream")],
                        happy_stream(),
                    )
                        .into_response();
                }
                if body.get("tools").is_some() {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                if body.get("text").is_some() {
                    return normal_response(&body);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                StatusCode::NOT_FOUND.into_response()
            }),
        ))
        .await;

        let failure = run_balanced(
            &reqwest::Client::new(),
            &target_for(&endpoint, "key"),
            &CapabilityProfile::for_target(&AppType::Codex, "gpt-5.6-sol"),
        )
        .await
        .unwrap_err();

        assert_eq!(failure, RunFailure::ModelUnavailable);
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
        let (facts, _) = run_balanced(
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
        // 缺 response.completed = 流被截断：按错误上报（重试一次仍无终端），
        // 不再落成「流式生命周期未通过」的判定 —— 截断是网络事实，不是协议证据。
        assert_eq!(
            run_balanced(
                &reqwest::Client::new(),
                &target_for_model(&endpoint, "SENTINEL_API_KEY", "future-model-x"),
                &CapabilityProfile::for_target(&AppType::Codex, "future-model-x"),
            )
            .await
            .unwrap_err(),
            RunFailure::InvalidResponse
        );
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

    #[derive(Clone)]
    struct ConcurrentState {
        requests: Requests,
        barrier: Arc<Barrier>,
    }

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
                "object":"response", "id":"resp_test000000000000", "status":"completed", "model":"gpt-5.6-sol",
                "output":[{"type":"function_call","name":"report_probe","arguments":"{\"ready\":true}"}],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }))
            .into_response();
        }
        if body.get("text").is_some() {
            return Json(json!({
                "object":"response", "id":"resp_test000000000000", "status":"completed", "model":"gpt-5.6-sol",
                "output":[{"type":"message","content":[{"type":"output_text","text":"{\"ready\":true}"}]}],
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }))
            .into_response();
        }
        Json(json!({
            "object":"response", "id":"resp_test000000000000", "status":"completed", "model":"gpt-5.6-sol",
            "output":[{"type":"message","content":[{"type":"output_text","text":"SENTINEL_OUTPUT"}]}],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }))
        .into_response()
    }

    async fn barrier_happy_handler(
        State(state): State<ConcurrentState>,
        uri: OriginalUri,
        headers: HeaderMap,
        body: Json<Value>,
    ) -> Response {
        state.barrier.wait().await;
        happy_handler(State(state.requests), uri, headers, body).await
    }

    async fn delayed_happy_handler(
        state: State<Requests>,
        uri: OriginalUri,
        headers: HeaderMap,
        body: Json<Value>,
    ) -> Response {
        let delay = if body["stream"] == true {
            Duration::ZERO
        } else if body.get("tools").is_some() {
            Duration::from_millis(10)
        } else if body.get("text").is_some() {
            Duration::from_millis(20)
        } else {
            Duration::from_millis(50)
        };
        tokio::time::sleep(delay).await;
        happy_handler(state, uri, headers, body).await
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
                "object":"response", "id":"resp_test000000000000", "status":"completed", "model":"future-model-x",
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
            "object":"response", "id":"resp_test000000000000", "status":"completed", "model":"future-model-x",
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
