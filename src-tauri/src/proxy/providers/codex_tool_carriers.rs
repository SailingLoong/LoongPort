//! Codex 私有工具载体的网关能力自适应。
//!
//! Codex 0.15x 默认向每个 Responses 上游发送托管工具载体：`web_search`
//! （任何模型都带）、`tool_search`（已知模型族带）、`namespace`（未知模型族
//! 带），并把会话历史里的私有条目类型（`tool_search_call` /
//! `tool_search_output` / `web_search_call`）原样回放。严格的三方网关会以
//! 400 整单拒绝（实测报错形状：`unknown type: tool_search_call`、`tool type
//! 'web_search' is not supported by this gateway phase`），且因为这些条目
//! 持久化在线程历史里，之后每一轮都同样失败——线程被永久毒化。
//!
//! 写入侧的黑名单（`codex_config.rs`）只覆盖四家第一方网关，而中转站的
//! 渠道组合轮换远快于应用发版。网关的真实能力只有在请求时刻、由代理才能
//! 观察到，所以适配归代理所有：检测到载体被拒 → 从请求里剥离被点名的载体
//! （`tools[]` 条目 + `input[]` 里的同族历史项）→ 同 provider 重试一次 →
//! 按 provider 记住，后续请求预先剥离、不再支付那次必然失败的往返。

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use tokio::sync::RwLock;

use crate::proxy::error::ProxyError;
use crate::proxy::media_sanitizer::extract_error_text;

/// Codex/OpenAI 私有工具 `type` 载体全集。只剥这些——并且只在上游自己点名
/// 拒绝时才剥，普通 function / custom 工具永远原样保留。
const KNOWN_TOOL_CARRIERS: &[&str] = &["web_search", "tool_search", "namespace"];

/// 一个工具载体在会话历史 `input[]` 里的同族条目类型。`tool_search_call` 与
/// `tool_search_output` 必须成对剥离：只删 call 会留下孤儿 output，部分网关
/// 同样拒绝；只删 output 会丢掉动态加载工具的唯一定义。
fn carrier_history_item_types(carrier: &str) -> &'static [&'static str] {
    match carrier {
        "tool_search" => &["tool_search_call", "tool_search_output"],
        "web_search" => &["web_search_call"],
        "namespace" => &["namespace_call", "namespace_output"],
        _ => &[],
    }
}

/// 判定为「载体被网关拒绝」的 HTTP 状态码。400 是 Responses 校验拒绝的
/// 标准形状（#7560、厂商网关实测），422 是部分严格 serde 网关用的变体。
const REJECTION_STATUS: &[u16] = &[400, 422];

/// 报错文本里出现这些短语，且点到了我们发出的某个载体，才认定为载体拒绝。
const REJECTION_HINTS: &[&str] = &[
    "unknown type",
    "unknown tool type",
    "not supported",
    "unsupported",
    "does not support",
    "doesn't support",
    "responses_feature_not_supported",
];

/// `message` 以词边界包含 `carrier`（前后都不是字母数字或下划线），避免
/// `web_search_call` 误匹配 `web_search`、`not_web_search` 这类子串。
fn contains_word(message: &str, carrier: &str) -> bool {
    let bytes = message.as_bytes();
    let needle = carrier.as_bytes();
    let mut start = 0;
    while start + needle.len() <= bytes.len() {
        if &bytes[start..start + needle.len()] == needle {
            let before_ok = start == 0
                || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
            let end = start + needle.len();
            let after_ok =
                end == bytes.len() || !(bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
            if before_ok && after_ok {
                return true;
            }
        }
        start += 1;
    }
    false
}

/// 请求体里实际携带了哪些载体：`tools[]` 的 `type` 或 `input[]` 顶层条目的
/// `type` 命中任一载体（含历史项变体）。错误里点名的载体只有真的在请求里，
/// 才允许剥离——这是防止把碰巧提到相关字样的无关 400（配额、鉴权）误判成
/// 载体拒绝的闸门。
fn carriers_present_in_body(body: &Value) -> HashSet<String> {
    let mut present = HashSet::new();
    let mut consider = |type_name: &str| {
        for carrier in KNOWN_TOOL_CARRIERS {
            let variants: Vec<&str> = std::iter::once(*carrier)
                .chain(carrier_history_item_types(carrier).iter().copied())
                .collect();
            if variants.contains(&type_name) {
                present.insert((*carrier).to_string());
            }
        }
    };
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for tool in tools {
            if let Some(type_name) = tool.get("type").and_then(Value::as_str) {
                consider(type_name);
            }
        }
    }
    if let Some(input) = body.get("input").and_then(Value::as_array) {
        for item in input {
            if let Some(type_name) = item.get("type").and_then(Value::as_str) {
                consider(type_name);
            }
        }
    }
    present
}

/// 从上游错误里解析出被拒的载体集合。空集 = 不是载体拒绝，调用方按原逻辑
/// 走（故障转移 / 直接返回错误）。
pub(crate) fn rejected_carriers_from_error(
    error: &ProxyError,
    outbound_body: &Value,
) -> Vec<String> {
    let ProxyError::UpstreamError { status, body } = error else {
        return Vec::new();
    };
    if !REJECTION_STATUS.contains(status) {
        return Vec::new();
    }
    let Some(body) = body.as_deref() else {
        return Vec::new();
    };

    // 厂商网关的 responses_feature_not_supported 直接断言「这个网关阶段不
    // 支持该托管工具」，无需再依赖报错措辞点名了哪个工具：请求里携带的
    // 载体全部视为被拒。
    let type_says_feature_unsupported = body.contains("responses_feature_not_supported");
    let message = extract_error_text(body).to_ascii_lowercase();
    let hints_reject = REJECTION_HINTS
        .iter()
        .any(|hint| message.contains(hint) || type_says_feature_unsupported);
    if !hints_reject {
        return Vec::new();
    }

    let present = carriers_present_in_body(outbound_body);
    // 报错点名载体时按变体集合做词边界匹配：`unknown type: tool_search_call`
    // 点名的是历史项变体，归属 tool_search 载体；而 `web_search_call_item`
    // 这类更长的词不匹配任何变体，不会被误判。
    let names_a_carrier = |carrier: &str| {
        let mut variants =
            std::iter::once(carrier).chain(carrier_history_item_types(carrier).iter().copied());
        variants.any(|variant| contains_word(&message, variant))
    };
    let mut rejected: Vec<String> = if type_says_feature_unsupported {
        present.iter().cloned().collect()
    } else {
        KNOWN_TOOL_CARRIERS
            .iter()
            .filter(|carrier| present.contains(**carrier))
            .filter(|carrier| names_a_carrier(carrier))
            .map(|carrier| carrier.to_string())
            .collect()
    };
    rejected.sort();
    rejected
}

/// 就地剥离载体：`tools[]` 里 `type` 命中的条目 + `input[]` 顶层同族历史项。
/// 返回是否有改动。幂等：对已剥离的请求再跑一遍不变。
pub(crate) fn strip_carriers(body: &mut Value, carriers: &[String]) -> bool {
    if carriers.is_empty() || !body.is_object() {
        return false;
    }
    let mut expanded: HashSet<&str> = HashSet::new();
    for carrier in carriers {
        expanded.insert(carrier.as_str());
        for item_type in carrier_history_item_types(carrier) {
            expanded.insert(item_type);
        }
    }
    let matches_type = |value: &Value| -> bool {
        value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|type_name| expanded.contains(type_name))
    };

    let mut changed = false;
    if let Some(tools) = body.get_mut("tools").and_then(Value::as_array_mut) {
        let before = tools.len();
        tools.retain(|tool| !matches_type(tool));
        changed |= tools.len() != before;
    }
    if let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) {
        let before = input.len();
        input.retain(|item| !matches_type(item));
        changed |= input.len() != before;
    }
    changed
}

/// 按 (app, provider) 记住的「该网关拒绝过的载体」。进程内存活：冷启动后
/// 第一个请求会再吃一次 400→剥离→重试的自愈往返（对用户只是多一个 RTT），
/// 换来零持久化状态、零过期管理——网关后来修好了能力，重启后自然恢复。
#[derive(Default)]
pub struct CodexToolCarrierStore {
    inner: RwLock<HashMap<(String, String), HashSet<String>>>,
}

impl CodexToolCarrierStore {
    pub async fn record(&self, app_type: &str, provider_id: &str, carriers: &[String]) {
        if carriers.is_empty() {
            return;
        }
        let key = (app_type.to_string(), provider_id.to_string());
        let mut inner = self.inner.write().await;
        inner
            .entry(key)
            .or_default()
            .extend(carriers.iter().cloned());
    }

    pub async fn carriers_for(&self, app_type: &str, provider_id: &str) -> Vec<String> {
        let key = (app_type.to_string(), provider_id.to_string());
        let inner = self.inner.read().await;
        inner
            .get(&key)
            .map(|set| {
                let mut carriers: Vec<String> = set.iter().cloned().collect();
                carriers.sort();
                carriers
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn outbound_body() -> Value {
        json!({
            "model": "gpt-5.5",
            "tools": [
                {"type": "function", "name": "shell"},
                {"type": "tool_search"},
                {"type": "web_search"}
            ],
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "tool_search_call", "call_id": "c1", "arguments": {}},
                {"type": "tool_search_output", "call_id": "c1", "output": {}},
                {"type": "web_search_call", "id": "w1", "status": "completed"}
            ]
        })
    }

    fn upstream_error(status: u16, body: &str) -> ProxyError {
        ProxyError::UpstreamError {
            status,
            body: Some(body.to_string()),
        }
    }

    #[test]
    fn detects_unknown_history_type_rejection() {
        let error = upstream_error(
            400,
            r#"{"error":{"message":"unknown type: tool_search_call","type":"invalid_request_error"}}"#,
        );
        assert_eq!(
            rejected_carriers_from_error(&error, &outbound_body()),
            vec!["tool_search".to_string()]
        );
    }

    #[test]
    fn detects_vendor_gateway_web_search_rejection() {
        // responses_feature_not_supported 断言的是网关级不支持（不是单个工具），
        // 请求里在场的托管载体全部视为被拒
        let error = upstream_error(
            400,
            r#"{"error":{"type":"responses_feature_not_supported","message":"tool type 'web_search' is not supported by this gateway phase"}}"#,
        );
        assert_eq!(
            rejected_carriers_from_error(&error, &outbound_body()),
            vec!["tool_search".to_string(), "web_search".to_string()]
        );
    }

    #[test]
    fn feature_unsupported_without_named_tool_strips_all_present_carriers() {
        let error = upstream_error(
            400,
            r#"{"error":{"type":"responses_feature_not_supported","message":"feature not supported"}}"#,
        );
        assert_eq!(
            rejected_carriers_from_error(&error, &outbound_body()),
            vec!["tool_search".to_string(), "web_search".to_string()]
        );
    }

    #[test]
    fn unrelated_quota_error_is_not_a_carrier_rejection() {
        let error = upstream_error(
            400,
            r#"{"error":{"message":"web_search quota exceeded for this key"}}"#,
        );
        assert!(rejected_carriers_from_error(&error, &outbound_body()).is_empty());
    }

    #[test]
    fn carrier_not_present_in_body_is_ignored() {
        let error = upstream_error(400, r#"{"error":{"message":"unknown type: namespace"}}"#);
        // outbound_body 没有 namespace 载体
        assert!(rejected_carriers_from_error(&error, &outbound_body()).is_empty());
    }

    #[test]
    fn server_errors_and_other_statuses_are_ignored() {
        let error = upstream_error(500, r#"{"error":{"message":"unknown type: web_search"}}"#);
        assert!(rejected_carriers_from_error(&error, &outbound_body()).is_empty());
    }

    #[test]
    fn stripping_removes_tools_and_history_pairs_but_keeps_functions() {
        let mut body = outbound_body();
        assert!(strip_carriers(&mut body, &["tool_search".to_string()]));
        let tool_types: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["type"].as_str())
            .collect();
        assert_eq!(tool_types, vec!["function", "web_search"]);
        let item_types: Vec<&str> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["type"].as_str())
            .collect();
        assert_eq!(item_types, vec!["message", "web_search_call"]);
    }

    #[test]
    fn stripping_is_idempotent() {
        let mut body = outbound_body();
        strip_carriers(&mut body, &["tool_search".to_string()]);
        assert!(!strip_carriers(&mut body, &["tool_search".to_string()]));
    }

    #[test]
    fn stripping_multiple_carriers_covers_web_search_call_history() {
        let mut body = outbound_body();
        assert!(strip_carriers(
            &mut body,
            &["web_search".to_string(), "tool_search".to_string()]
        ));
        let item_types: Vec<&str> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["type"].as_str())
            .collect();
        assert_eq!(item_types, vec!["message"]);
    }

    #[tokio::test]
    async fn store_records_and_returns_sorted_carriers() {
        let store = CodexToolCarrierStore::default();
        store
            .record("codex", "p1", &["web_search".to_string()])
            .await;
        store
            .record("codex", "p1", &["tool_search".to_string()])
            .await;
        assert_eq!(
            store.carriers_for("codex", "p1").await,
            vec!["tool_search".to_string(), "web_search".to_string()]
        );
        assert!(store.carriers_for("codex", "p2").await.is_empty());
        assert!(store.carriers_for("claude", "p1").await.is_empty());
    }

    #[test]
    fn word_boundary_prevents_suffix_false_positive() {
        let error = upstream_error(
            400,
            r#"{"error":{"message":"unknown type: web_search_call_item"}}"#,
        );
        // web_search_call_item 不是我们发出的载体；词边界后 web_search 不应命中
        assert!(rejected_carriers_from_error(&error, &outbound_body()).is_empty());
    }
}
