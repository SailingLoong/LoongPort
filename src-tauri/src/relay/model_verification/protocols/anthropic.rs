use serde_json::{json, Value};

use crate::relay::model_verification::{
    capability_profiles::CapabilityProfile,
    protocols::{model_matches_protocol_family, models_match, send_and_read, send_sse, RunFailure},
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
    run_balanced_with_progress(client, target, profile, &mut || {}).await
}

pub(crate) async fn run_balanced_with_progress(
    client: &reqwest::Client,
    target: &ResolvedTarget,
    profile: &CapabilityProfile,
    on_probe_complete: &mut impl FnMut(),
) -> Result<Vec<EvidenceFact>, RunFailure> {
    let model = upstream_model(&target.target().model);
    let endpoint = format!(
        "{}/v1/messages",
        target.protocol_base().trim_end_matches('/')
    );

    let core = send_message(client, &endpoint, target.api_key(), core_request(&model)).await?;
    let mut facts = parse_core_response(&core, &model);
    on_probe_complete();

    let identity = send_message(
        client,
        &endpoint,
        target.api_key(),
        identity_request(&model),
    )
    .await?;
    facts.push(parse_identity_response(&identity, &model));
    on_probe_complete();

    let tool = send_message(client, &endpoint, target.api_key(), tool_request(&model)).await?;
    facts.push(parse_tool_response(&tool, &model));
    on_probe_complete();

    // 无 message_stop = 流被截断（干净 EOF 中途收尾），按错误重试一次而不是
    // 判「流式生命周期未通过」——截断是网络事实，不是协议证据。
    let mut stream = None;
    for _ in 0..2 {
        let state = send_stream(
            client,
            &endpoint,
            target.api_key(),
            stream_request(&model),
            || StreamReducer::new(&model),
            |stream, event| stream.observe(event),
        )
        .await?;
        if state.saw_message_stop {
            stream = Some(state);
            break;
        }
    }
    let stream = stream.ok_or(RunFailure::InvalidResponse)?;
    facts.extend(stream.finish());
    on_probe_complete();

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
        on_probe_complete();

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
            on_probe_complete();
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
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("accept", "text/event-stream")
            .json(&payload),
        new_state,
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

/// 身份自述探针：让模型自报家门。只有出现**竞品品牌**才判失败（换芯强信号）；
/// 自述含糊或拒绝作答不算证据（Skipped）——prompt 可被站点注入绕过，权重天然低。
fn identity_request(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 200,
        "messages": [{"role": "user", "content":
            "Who are you exactly? What is your model name and version? Which company developed you? Answer concisely."}],
    })
}

fn tool_request(model: &str) -> Value {
    json!({
        "model": model,
        // 带思考输出的跨家族模型（如 deepseek 系走 claude 协议）会先吐
        // thinking 块：64 会把 tool 参数截成空对象，512 才够完成调用。
        "max_tokens": 512,
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
    // 统一用 enabled+budget（自适应 thinking 的 effort 走顶层 output_config、
    // 且流式下会静默丢 thinking 块——enabled 形状兼容面最宽）。GCD 题保证
    // 模型确实进入思考，"Think briefly" 这类轻提示在自适应档可能完全不思考。
    json!({
        "model": model,
        "max_tokens": 16000,
        "thinking": {"type": "enabled", "budget_tokens": 2000},
        "messages": [{"role": "user", "content":
            "Find the greatest common divisor of 2378 and 1547 using the Euclidean algorithm."}],
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
        facts.push(if models_match(expected_model, model) {
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

/// 身份自述评估（**按请求模型的家族**判定）：自报其他家族品牌 → failed
/// （换芯强信号）；自报本家族品牌 → passed；含糊/拒答 → skipped。家族相对
/// 是关键——GPT 系模型走 claude 协议时自报 GPT 是诚实行为，不是竞品。
fn parse_identity_response(body: &[u8], expected_model: &str) -> EvidenceFact {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return skipped(EvidenceCode::ForeignSelfIdentification);
    };
    let text = response
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<String>()
        })
        .unwrap_or_default()
        .to_ascii_lowercase();
    let Some((self_brands, competitor_brands)) = identity_brand_sets(expected_model) else {
        return skipped(EvidenceCode::ForeignSelfIdentification);
    };
    if competitor_brands.iter().any(|brand| text.contains(brand)) {
        return failed(EvidenceCode::ForeignSelfIdentification);
    }
    if self_brands.iter().any(|brand| text.contains(brand)) {
        return passed(EvidenceCode::ForeignSelfIdentification);
    }
    skipped(EvidenceCode::ForeignSelfIdentification)
}

/// 请求模型 → (本家族品牌词, 其他家族品牌词)。未知家族返回 None（不判）。
/// GPT 家族词用 gpt-5/gpt-4/gpt-3 带版本形式，避免裸 "gpt" 误撞词根。
fn identity_brand_sets(model: &str) -> Option<(Vec<&'static str>, Vec<&'static str>)> {
    const CLAUDE: &[&str] = &["claude", "anthropic"];
    const GPT: &[&str] = &["chatgpt", "openai", "gpt-5", "gpt-4", "gpt-3"];
    const SMALL_FAMILIES: &[&str] = &[
        "gemini", "bard", "deepseek", "qwen", "通义", "grok", "xai", "kimi", "moonshot",
    ];
    let model = model.to_ascii_lowercase();
    let self_brands: Vec<&'static str> = if model.contains("claude") || model.contains("anthropic")
    {
        CLAUDE.to_vec()
    } else if model.contains("gpt") || model.contains("codex") {
        GPT.to_vec()
    } else {
        vec![*SMALL_FAMILIES.iter().find(|brand| model.contains(*brand))?]
    };
    let competitor_brands: Vec<&'static str> = CLAUDE
        .iter()
        .chain(GPT)
        .chain(SMALL_FAMILIES)
        .copied()
        .filter(|brand| !self_brands.contains(brand))
        .collect();
    Some((self_brands, competitor_brands))
}

fn parse_tool_response(body: &[u8], expected_model: &str) -> EvidenceFact {
    let Ok(response) = serde_json::from_slice::<Value>(body) else {
        return failed(EvidenceCode::ToolCallShape);
    };
    // 跨家族网关（如 deepseek 走 claude 协议）在做跨协议转换：既不保证
    // 强制函数调用（文本作答），也不保证调用形状完整（思考吃掉预算把参数
    // 截空）。跨家族一律不因工具形状 Failed——有合法调用记 Passed，否则
    // Skipped；只有同家族才严格判定。
    let cross_family = !model_matches_protocol_family(expected_model, false);
    let is_valid_tool = response
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block.get("name").and_then(Value::as_str) == Some(REPORT_PROBE)
                    && block.get("input").is_some_and(Value::is_object)
            })
        })
        && response.get("stop_reason").and_then(Value::as_str) == Some("tool_use");
    if is_valid_tool {
        passed(EvidenceCode::ToolCallShape)
    } else if cross_family {
        skipped(EvidenceCode::ToolCallShape)
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
                        // 真实签名是服务端加密块，实测远长于 50 字符；空串或
                        // 短占位不足以证明签名机制在位。
                        .is_some_and(|signature| signature.len() >= SIGNATURE_MIN_LEN)
            })
        })
    else {
        return (failed(EvidenceCode::ThinkingSignature), None);
    };
    (passed(EvidenceCode::ThinkingSignature), Some(block.clone()))
}

/// 真实 Anthropic 签名的经验下界（实测 300+；50 是宽松底线）。
const SIGNATURE_MIN_LEN: usize = 50;

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

fn skipped(code: EvidenceCode) -> EvidenceFact {
    EvidenceFact {
        code,
        outcome: EvidenceOutcome::Skipped,
    }
}

struct StreamReducer<'a> {
    expected_model: &'a str,
    saw_message_start: bool,
    saw_content_start: bool,
    saw_message_delta: bool,
    pub(crate) saw_message_stop: bool,
    open_content_block: Option<u64>,
    open_block_has_delta: bool,
    lifecycle_order_valid: bool,
    model_matches: Option<bool>,
    /// message_start 的 usage 基准是否合规（input+output 双字段）。
    usage_baseline_ok: bool,
    /// 官方语义：message_delta.usage 只保证累计 output_tokens。
    delta_output_tokens: Option<u64>,
    foreign_protocol: bool,
}

impl<'a> StreamReducer<'a> {
    fn new(expected_model: &'a str) -> Self {
        Self {
            expected_model,
            saw_message_start: false,
            saw_content_start: false,
            saw_message_delta: false,
            saw_message_stop: false,
            open_content_block: None,
            open_block_has_delta: false,
            lifecycle_order_valid: true,
            model_matches: None,
            usage_baseline_ok: false,
            delta_output_tokens: None,
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
        // 生命周期窗口之外的**内容事件**（message_start 之前 / message_stop
        // 之后漏出的 block/delta）直接忽略：它们不参与响应内容，记违规只会
        // 制造误报。边界事件本身（message_start、message_stop）不在此列。
        if (!self.saw_message_start || self.saw_message_stop)
            && value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| {
                    matches!(
                        kind,
                        "content_block_start"
                            | "content_block_delta"
                            | "content_block_stop"
                            | "message_delta"
                    )
                })
        {
            return Ok(());
        }
        // 未开始就出现的 message_stop 同样是窗口外泄漏，忽略。
        if !self.saw_message_start
            && value.get("type").and_then(Value::as_str) == Some("message_stop")
        {
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
                    .map(|model| models_match(self.expected_model, model));
                if let Some(usage) = message.get("usage") {
                    // 基准必须双字段齐全。
                    self.usage_baseline_ok = usage_get(usage, "input_tokens").is_some()
                        && usage_get(usage, "output_tokens").is_some();
                }
            }
            Some("content_block_start") => {
                let index = event_index(&value);
                if !self.saw_message_start
                    || self.saw_message_delta
                    || self.saw_message_stop
                    || self.open_content_block.is_some()
                    || index.is_none()
                {
                    self.lifecycle_order_valid = false;
                }
                self.saw_content_start = true;
                if let Some(index) = index {
                    self.open_content_block = Some(index);
                    self.open_block_has_delta = false;
                }
            }
            Some("content_block_delta") => {
                if self.open_content_block.is_none()
                    || self.open_content_block != event_index(&value)
                {
                    self.lifecycle_order_valid = false;
                } else {
                    self.open_block_has_delta = true;
                }
            }
            Some("content_block_stop") => {
                if self.open_content_block.is_none()
                    || self.open_content_block != event_index(&value)
                    || !self.open_block_has_delta
                {
                    self.lifecycle_order_valid = false;
                } else {
                    self.open_content_block = None;
                    self.open_block_has_delta = false;
                }
            }
            Some("message_delta") => {
                if !self.saw_content_start
                    || self.saw_message_stop
                    || self.open_content_block.is_some()
                {
                    self.lifecycle_order_valid = false;
                }
                self.saw_message_delta = true;
                // 官方语义：该事件只带累计 output_tokens（有 input 也收）。
                if let Some(output) = value
                    .get("usage")
                    .and_then(|usage| usage_get(usage, "output_tokens"))
                {
                    self.delta_output_tokens = Some(output);
                }
            }
            Some("message_stop") => {
                if !self.saw_message_delta
                    || self.saw_message_stop
                    || self.open_content_block.is_some()
                {
                    self.lifecycle_order_valid = false;
                }
                self.saw_message_stop = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Vec<EvidenceFact> {
        let mut facts = vec![if self.lifecycle_order_valid
            && self.saw_message_start
            && self.saw_content_start
            && self.saw_message_delta
            && self.saw_message_stop
            && self.open_content_block.is_none()
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
        facts.push(if self.usage_is_consistent() {
            passed(EvidenceCode::UsageConsistency)
        } else {
            failed(EvidenceCode::UsageConsistency)
        });
        if self.foreign_protocol {
            facts.push(failed(EvidenceCode::ForeignProtocol));
        }
        facts
    }

    /// 官方语义的用量一致性：message_start 基准双字段 + message_delta 累计
    /// output_tokens 到场。**不做流式与非流式的跨腿比对**——订阅号池后端
    /// 两腿口径天然不同（非流式含注入的系统提示、流式只算裸请求，实测
    /// 4390 vs 10），跨腿等式在该生态不成立；这里只抓字段缺失与形状违规。
    fn usage_is_consistent(&self) -> bool {
        self.usage_baseline_ok && self.delta_output_tokens.is_some()
    }
}

fn usage_get(usage: &Value, field: &str) -> Option<u64> {
    usage.get(field).and_then(Value::as_u64)
}

fn event_index(value: &Value) -> Option<u64> {
    value.get("index").and_then(Value::as_u64)
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

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{
        parse_identity_response, parse_tool_response, reduce_thinking_response, stream_request,
        thinking_request, StreamReducer, REPORT_PROBE,
    };

    use crate::{
        app_config::AppType,
        database::Database,
        provider::{Provider, ProviderMeta},
        relay::model_verification::{
            capability_profiles::CapabilityProfile,
            protocols::{
                anthropic::{
                    parse_core_response, parse_stream, run_balanced, run_balanced_with_progress,
                },
                RunFailure, MAX_RESPONSE_BYTES, MAX_SSE_EVENT_BYTES,
            },
            target::ResolvedTarget,
            types::{EvidenceCode, EvidenceFact, EvidenceOutcome, TargetKey},
        },
    };
    use axum::{
        body::Body,
        extract::{OriginalUri, State},
        http::{HeaderMap, HeaderValue, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
        Json, Router,
    };
    use bytes::Bytes;
    use serde_json::{json, Value};

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        path: String,
        api_key: Option<HeaderValue>,
        version: Option<HeaderValue>,
        content_type: Option<HeaderValue>,
        accept: Option<HeaderValue>,
        body: Value,
    }

    type Requests = Arc<Mutex<Vec<RecordedRequest>>>;

    /// 满足签名长度下界的哨兵签名（≥50 字符，前缀仍是敏感标记便于泄漏断言）。
    const SENTINEL_SIGNATURE: &str = "SENTINEL_SIGNATURE_PADDED_TO_FIFTY_CHARS_0123456789";

    #[test]
    fn thinking_response_reduces_an_opaque_signature_to_a_fact() {
        let signature = format!("SENTINEL_SIGNATURE_MUST_NOT_PERSIST_{}", "x".repeat(64));
        let response = format!(
            "{{\"type\":\"message\",\"model\":\"claude-haiku-4-5\",\"content\":[{{\"type\":\"thinking\",\"thinking\":\"private\",\"signature\":\"{signature}\"}}],\"usage\":{{\"input_tokens\":3,\"output_tokens\":2}}}}"
        );

        let (fact, _) = reduce_thinking_response(response.as_bytes());

        assert_eq!(fact, passed(EvidenceCode::ThinkingSignature));
        assert!(!serde_json::to_string(&fact).unwrap().contains(&signature));
    }

    #[test]
    fn short_signature_does_not_count_as_signed_thinking() {
        let response = br#"{
            "type":"message","model":"claude-haiku-4-5",
            "content":[{"type":"thinking","thinking":"private","signature":"too-short"}],
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#;

        let (fact, block) = reduce_thinking_response(response);

        assert_eq!(fact, failed(EvidenceCode::ThinkingSignature));
        assert!(block.is_none());
    }

    #[test]
    fn message_envelope_reduces_model_usage_and_foreign_protocol_without_text() {
        let expected = "claude-haiku-4-5";
        let response = br#"{
            "type":"message","id":"msg_0123456789","model":"claude-haiku-4-5",
            "content":[{"type":"text","text":"SENTINEL_RESPONSE_TEXT"}],
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#;

        let facts = parse_core_response(response, expected);

        assert!(facts.contains(&passed(EvidenceCode::BasicEnvelope)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
        assert!(!serde_json::to_string(&facts)
            .unwrap()
            .contains("SENTINEL_RESPONSE_TEXT"));

        // 别名请求 ↔ 日期快照回显：前缀等价，不是换芯（订阅渠道常见行为）。
        let snapshot = br#"{
            "type":"message","id":"msg_0123456789","model":"claude-haiku-4-5-20251001",
            "content":[{"type":"text","text":"ready"}],
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#;
        let facts = parse_core_response(snapshot, expected);
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));

        // 回显指向另一个模型家族才是 ModelMatch 失败。
        let swapped = br#"{
            "type":"message","id":"msg_0123456789","model":"gpt-5.6-sol",
            "content":[{"type":"text","text":"ready"}],
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#;
        let facts = parse_core_response(swapped, expected);
        assert!(facts.contains(&failed(EvidenceCode::ModelMatch)));

        let foreign =
            parse_core_response(br#"{"object":"chat.completion","choices":[]}"#, expected);
        assert_eq!(foreign, vec![failed(EvidenceCode::ForeignProtocol)]);
    }

    #[test]
    fn cross_family_text_answer_skips_instead_of_failing_tool_check() {
        let text_answer = br#"{
            "type":"message","id":"msg_test000000000000000","model":"deepseek-v4-pro",
            "content":[{"type":"text","text":"I cannot call tools."}],
            "usage":{"input_tokens":1,"output_tokens":9},"stop_reason":"end_turn"
        }"#;
        assert_eq!(
            parse_tool_response(text_answer, "deepseek-v4-pro"),
            skipped(EvidenceCode::ToolCallShape)
        );
        // 同家族（claude 走 claude 协议）文本作答仍然是失败：能力缺失信号。
        assert_eq!(
            parse_tool_response(text_answer, "claude-opus-5"),
            failed(EvidenceCode::ToolCallShape)
        );
    }

    #[test]
    fn identity_self_report_only_fails_on_competitor_brands() {
        let envelope = |text: &str| {
            format!(
                "{{\"type\":\"message\",\"id\":\"msg_1\",\"model\":\"m\",\"content\":[{{\"type\":\"text\",\"text\":{text}}}],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}"
            )
        };

        assert_eq!(
            parse_identity_response(
                envelope(r#""I am Claude, made by Anthropic.""#).as_bytes(),
                "claude-opus-5",
            ),
            passed(EvidenceCode::ForeignSelfIdentification)
        );
        assert_eq!(
            parse_identity_response(
                envelope(r#""I'm an AI assistant and cannot answer.""#).as_bytes(),
                "claude-opus-5",
            ),
            skipped(EvidenceCode::ForeignSelfIdentification)
        );
        assert_eq!(
            parse_identity_response(
                envelope(r#""I am ChatGPT, made by OpenAI.""#).as_bytes(),
                "claude-opus-5",
            ),
            failed(EvidenceCode::ForeignSelfIdentification)
        );
        // 家族相对：GPT 系模型（哪怕走 claude 协议的档位）自报 GPT 是诚实行为。
        assert_eq!(
            parse_identity_response(
                envelope(r#""I am ChatGPT, made by OpenAI.""#).as_bytes(),
                "gpt-5.6-sol",
            ),
            passed(EvidenceCode::ForeignSelfIdentification)
        );
        // 反向：请求 GPT 却自报 Claude，同样是换芯信号。
        assert_eq!(
            parse_identity_response(
                envelope(r#""I am Claude, made by Anthropic.""#).as_bytes(),
                "gpt-5.6-sol",
            ),
            failed(EvidenceCode::ForeignSelfIdentification)
        );
        // 未知家族不判。
        assert_eq!(
            parse_identity_response(
                envelope(r#""I am ChatGPT.""#).as_bytes(),
                "some-unknown-model",
            ),
            skipped(EvidenceCode::ForeignSelfIdentification)
        );
    }

    #[test]
    fn tool_call_accepts_gateway_ids_but_requires_tool_stop_reason() {
        // 聚合网关会再签发自己的 tool_use id（gen_ 等）——id 形状不构成换芯
        // 信号，不检查；stop_reason=tool_use 是协议语义，必须对。
        let base = r#"{
            "type":"message","id":"msg_1","model":"m",
            "content":[{"type":"tool_use","name":"report_probe","input":{"ready":true},"id":"gen-123"}],
            "usage":{"input_tokens":1,"output_tokens":1},"stop_reason":"STOP"
        }"#;

        let valid = base.replace("\"STOP\"", "\"tool_use\"");
        assert_eq!(
            parse_tool_response(valid.as_bytes(), "claude-haiku-4-5"),
            passed(EvidenceCode::ToolCallShape)
        );

        let wrong_stop = base.replace("\"STOP\"", "\"end_turn\"");
        assert_eq!(
            parse_tool_response(wrong_stop.as_bytes(), "claude-haiku-4-5"),
            failed(EvidenceCode::ToolCallShape)
        );
    }

    /// F1 回归：官方流式协议 message_delta.usage 只带累计 output_tokens，
    /// 旧规则要求双字段导致真上游恒判「用量一致性未通过」。
    #[test]
    fn official_stream_usage_semantics_pass_without_input_tokens_on_delta() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");
        let stream = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ready\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":12}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );

        let facts = parse_stream(stream, "future-model-x", &profile).unwrap();

        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
    }

    /// 订阅号池两腿口径天然不同（非流式含注入系统提示、流式只算裸请求，
    /// 实测 4390 vs 10）：跨腿数值分歧不构成用量不一致，不得误判。
    #[test]
    fn stream_usage_divergence_from_core_leg_is_tolerated() {
        let mut reducer = StreamReducer::new("future-model-x");
        for event in [
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ready\"}}\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":90}}\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n",
        ] {
            reducer.observe(event).unwrap();
        }

        let facts = reducer.finish();

        assert!(facts.contains(&passed(EvidenceCode::UsageConsistency)));
        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
    }

    #[test]
    fn additive_stream_events_do_not_break_anthropic_lifecycle() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");
        let stream = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\",\"metadata\":{\"ignored\":true}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ready\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );

        let facts = parse_stream(stream, "future-model-x", &profile).unwrap();

        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
        assert!(facts.contains(&passed(EvidenceCode::ModelMatch)));
    }

    #[test]
    fn ordinary_stream_does_not_evaluate_thinking_for_a_known_model() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "claude-haiku-4-5");
        let stream = ordinary_stream("claude-haiku-4-5");

        let facts = parse_stream(&stream, "claude-haiku-4-5", &profile).unwrap();

        assert!(!facts
            .iter()
            .any(|fact| fact.code == EvidenceCode::ThinkingSignature));
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
    }

    #[test]
    fn stream_requires_a_delta_and_matching_stop_for_each_content_block() {
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");
        for stream in [
            concat!(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
                "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
            ),
            concat!(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"future-model-x\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ready\"}}\n\n",
                "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n",
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
            ),
        ] {
            let facts = parse_stream(stream, "future-model-x", &profile).unwrap();
            assert!(facts.contains(&failed(EvidenceCode::StreamLifecycle)));
        }
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

        let mut completed_probes = 0;
        let facts = run_balanced_with_progress(&client, &target, &profile, &mut || {
            completed_probes += 1;
        })
        .await
        .unwrap();

        assert!(facts.contains(&passed(EvidenceCode::ToolCallShape)));
        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
        assert!(facts.contains(&passed(EvidenceCode::ThinkingSignature)));
        assert!(facts.contains(&passed(EvidenceCode::SignatureContinuation)));
        assert_eq!(completed_probes, 6);
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
        assert_eq!(requests.len(), 6);
        for request in requests.iter() {
            assert_eq!(request.path, "/v1/messages");
            assert_eq!(request.api_key.as_ref().unwrap(), "SENTINEL_API_KEY");
            assert_eq!(request.version.as_ref().unwrap(), "2023-06-01");
            assert_eq!(request.content_type.as_ref().unwrap(), "application/json");
        }
        // 顺序：core → identity → tool → stream → thinking → continuation。
        let identity = &requests[1];
        assert!(identity.body.get("tools").is_none());
        assert!(identity.body.get("thinking").is_none());
        assert!(identity.body.get("stream").is_none());
        assert!(identity.body["max_tokens"].as_u64().unwrap() <= 200);
        let continuation = &requests[5].body["messages"][1]["content"][0];
        assert_eq!(continuation["signature"], SENTINEL_SIGNATURE);
        assert_eq!(continuation["thinking"], "SENTINEL_THINKING");
        assert_eq!(requests[3].accept.as_ref().unwrap(), "text/event-stream");
        assert!(requests[3].body.get("thinking").is_none());
        assert_eq!(
            requests[4].body["thinking"],
            json!({"type": "enabled", "budget_tokens": 2000})
        );
    }

    #[test]
    fn thinking_requests_use_extended_budget_for_every_model() {
        assert!(stream_request("claude-haiku-4-5").get("thinking").is_none());
        for model in ["claude-haiku-4-5", "claude-opus-5", "claude-sonnet-5"] {
            assert_eq!(
                thinking_request(model)["thinking"],
                json!({"type": "enabled", "budget_tokens": 2000})
            );
        }
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
                RunFailure::Upstream { status: 502 },
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
    async fn utf8_split_across_sse_chunks_is_reduced_like_one_complete_event() {
        let endpoint =
            spawn_server(Router::new().route("/v1/messages", post(utf8_split_handler))).await;
        let target = target_for(&endpoint, "SENTINEL_API_KEY");
        let profile = CapabilityProfile::for_target(&AppType::Claude, "future-model-x");

        let facts = run_balanced(&reqwest::Client::new(), &target, &profile)
            .await
            .unwrap();

        assert!(facts.contains(&passed(EvidenceCode::StreamLifecycle)));
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

    fn skipped(code: EvidenceCode) -> EvidenceFact {
        EvidenceFact {
            code,
            outcome: EvidenceOutcome::Skipped,
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
            accept: headers.get("accept").cloned(),
            body: body.clone(),
        });
        if body.get("stream") == Some(&Value::Bool(true)) {
            return ([("content-type", "text/event-stream")], happy_stream()).into_response();
        }
        if body.get("tools").is_some() {
            return Json(json!({
                "type": "message", "id": "msg_test000000000000000", "model": "claude-haiku-4-5",
                "content": [{"type": "tool_use", "id": "toolu_test00000000000", "name": REPORT_PROBE, "input": {"ready": true}}],
                "usage": {"input_tokens": 2, "output_tokens": 1}, "stop_reason": "tool_use"
            })).into_response();
        }
        if body.get("thinking").is_some() {
            return Json(json!({
                "type": "message", "id": "msg_test000000000000000", "model": "claude-haiku-4-5",
                "content": [{"type": "thinking", "thinking": "SENTINEL_THINKING", "signature": SENTINEL_SIGNATURE}],
                "usage": {"input_tokens": 2, "output_tokens": 1}
            })).into_response();
        }
        message_response("claude-haiku-4-5")
    }

    async fn malformed_then_happy(
        State(calls): State<Arc<Mutex<usize>>>,
        Json(body): Json<Value>,
    ) -> Response {
        let first_call = {
            let mut calls = calls.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        if body.get("stream") == Some(&Value::Bool(true)) {
            return ([("content-type", "text/event-stream")], happy_stream()).into_response();
        }
        if body.get("tools").is_some() {
            return Json(json!({"type": "message", "id": "msg_test000000000000000", "model": "future-model-x", "content": [{"type": "tool_use", "id": "toolu_test00000000000", "name": REPORT_PROBE, "input": {}}], "usage": {"input_tokens": 1, "output_tokens": 1}, "stop_reason": "tool_use"})).into_response();
        }
        if first_call {
            Json(json!({"object": "chat.completion", "choices": [], "text": "SENTINEL_FOREIGN"}))
                .into_response()
        } else {
            message_response("future-model-x")
        }
    }

    async fn utf8_split_handler(Json(body): Json<Value>) -> Response {
        if body.get("stream") != Some(&Value::Bool(true)) {
            if body.get("tools").is_some() {
                return Json(json!({"type": "message", "id": "msg_test000000000000000", "model": "claude-haiku-4-5", "content": [{"type": "tool_use", "id": "toolu_test00000000000", "name": REPORT_PROBE, "input": {}}], "usage": {"input_tokens": 1, "output_tokens": 1}, "stop_reason": "tool_use"})).into_response();
            }
            return message_response("claude-haiku-4-5");
        }

        let stream = async_stream::stream! {
            yield Ok::<_, Infallible>(Bytes::from_static(b"event: ping\ndata: {\"type\":\"ping\",\"note\":\"\xE4"));
            yield Ok(Bytes::from_static(b"\xBD\xA0\"}\n\nevent: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-haiku-4-5\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ready\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
        };
        let mut response = Body::from_stream(stream).into_response();
        response.headers_mut().insert(
            "content-type",
            HeaderValue::from_static("text/event-stream"),
        );
        response
    }

    fn message_response(model: &str) -> Response {
        Json(json!({
            "type": "message", "id": "msg_test000000000000000", "model": model,
            "content": [{"type": "text", "text": "report ready"}],
            "usage": {"input_tokens": 1, "output_tokens": 1}
        }))
        .into_response()
    }

    fn happy_stream() -> String {
        concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-haiku-4-5\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SENTINEL_SIGNATURE\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        ).into()
    }

    fn ordinary_stream(model: &str) -> String {
        format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"model\":\"{model}\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":0}}}}}}\n\nevent: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\"}}}}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"ready\"}}}}\n\nevent: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\nevent: message_delta\ndata: {{\"type\":\"message_delta\",\"usage\":{{\"output_tokens\":1}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        )
    }
}
