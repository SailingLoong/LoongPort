//! Admit raw Responses history before the existing compatibility converter can
//! repair malformed calls. Final Chat admission remains in the HTTP worker.
use crate::proxy::error::ProxyError;
use serde_json::Value;
use std::collections::HashSet;

fn unsupported() -> ProxyError {
    ProxyError::InvalidRequest("unsupported_qoder_input".into())
}
fn text(value: &Value) -> bool {
    value.as_str().is_some_and(|v| !v.trim().is_empty())
}
fn only(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| keys.contains(&key.as_str())))
}
fn parts(value: &Value, images: bool) -> bool {
    if value.is_string() {
        return true;
    }
    value.as_array().is_some_and(|parts| {
        parts.iter().all(|part| match part["type"].as_str() {
            Some("input_text" | "text") => {
                only(part, &["type", "text"]) && part["text"].is_string()
            }
            Some("output_text") => {
                only(part, &["type", "text", "annotations"])
                    && part["text"].is_string()
                    && part
                        .get("annotations")
                        .is_none_or(|annotations| annotations.as_array().is_some_and(Vec::is_empty))
            }
            Some("input_image") if images => {
                only(part, &["type", "image_url"]) && text(&part["image_url"])
            }
            _ => false,
        })
    })
}
fn item(value: &Value) -> bool {
    match value["type"].as_str() {
        Some("function_call") => {
            only(
                value,
                &["type", "id", "call_id", "name", "arguments", "status"],
            ) && text(&value["call_id"])
                && text(&value["name"])
                && value["arguments"]
                    .as_str()
                    .and_then(|v| serde_json::from_str::<Value>(v).ok())
                    .is_some_and(|v| v.is_object())
        }
        Some("function_call_output") => {
            only(value, &["type", "id", "call_id", "output", "status"])
                && text(&value["call_id"])
                && value.get("output").is_some_and(|v| parts(v, true))
        }
        Some("message") | None => {
            only(value, &["type", "id", "role", "content", "status"])
                && value.get("type").is_none_or(|value| value == "message")
                && matches!(
                    value["role"].as_str(),
                    Some("system" | "developer" | "user" | "assistant")
                )
                && parts(&value["content"], value["role"] == "user")
        }
        _ => false,
    }
}

// Reject orphaned/duplicate results before launching a worker or refreshing an
// account. The worker still independently admits the complete Chat request.
fn linked_history(chat: &Value) -> bool {
    let Some(messages) = chat["messages"].as_array() else {
        return false;
    };
    let mut seen = HashSet::new();
    let mut pending = HashSet::new();
    for message in messages {
        if message["role"] == "tool" {
            let Some(id) = message["tool_call_id"].as_str() else {
                return false;
            };
            if !pending.remove(id) || message.get("tool_calls").is_some() {
                return false;
            }
        } else {
            if !pending.is_empty() || message.get("tool_call_id").is_some() {
                return false;
            }
            if let Some(calls) = message.get("tool_calls") {
                let Some(calls) = calls.as_array() else {
                    return false;
                };
                if message["role"] != "assistant" || calls.is_empty() {
                    return false;
                }
                for call in calls {
                    let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) else {
                        return false;
                    };
                    if !seen.insert(id) {
                        return false;
                    }
                    pending.insert(id);
                }
            }
        }
    }
    pending.is_empty()
}

fn chat_tools(chat: &Value) -> bool {
    let mut names = HashSet::new();
    if let Some(tools) = chat.get("tools") {
        let Some(tools) = tools.as_array() else {
            return false;
        };
        for tool in tools {
            let f = &tool["function"];
            let Some(name) = f["name"].as_str() else {
                return false;
            };
            if !only(tool, &["type", "function"])
                || tool["type"] != "function"
                || !only(f, &["name", "description", "parameters", "strict"])
                || name.trim().is_empty()
                || name.trim() != name
                || !names.insert(name)
                || f.get("description").is_some_and(|v| !v.is_string())
                || f.get("parameters")
                    .is_some_and(|v| !v.is_object() && !v.is_boolean())
                || f.get("strict").is_some_and(|v| v != false)
            {
                return false;
            }
        }
    }
    if chat
        .get("parallel_tool_calls")
        .is_some_and(|v| !v.is_boolean())
    {
        return false;
    }
    if let Some(choice) = chat.get("tool_choice") {
        match choice.as_str() {
            Some("auto" | "none") => {}
            Some("required") if !names.is_empty() => {}
            None if only(choice, &["type", "function"])
                && choice["type"] == "function"
                && only(&choice["function"], &["name"])
                && choice["function"]["name"]
                    .as_str()
                    .is_some_and(|name| names.contains(name)) => {}
            _ => return false,
        }
    }
    true
}

pub(super) fn chat_request(body: &Value, endpoint: &str) -> Result<Value, ProxyError> {
    let path = endpoint.split('?').next().unwrap_or(endpoint);
    if matches!(path, "/chat/completions" | "/v1/chat/completions") {
        // Keep the original inert treatment of untrusted SDK/executable hints.
        // All actual Chat fields remain for worker admission before network IO.
        let mut chat = body
            .as_object()
            .map(|_| body.clone())
            .ok_or_else(unsupported)?;
        for key in ["options", "nodePath", "sdkRoot"] {
            chat.as_object_mut().unwrap().remove(key);
        }
        if !linked_history(&chat) || !chat_tools(&chat) {
            return Err(unsupported());
        }
        return Ok(chat);
    }
    if !matches!(path, "/responses" | "/v1/responses") {
        return Err(unsupported());
    }
    if !only(
        body,
        &[
            "model",
            "input",
            "instructions",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "stream",
            "max_output_tokens",
            "max_tokens",
            "max_completion_tokens",
            "reasoning",
            "store",
            "options",
            "nodePath",
            "sdkRoot",
        ],
    ) || body.get("store").is_some_and(|v| v != false)
        || body.get("instructions").is_some_and(|v| !v.is_string())
        || body.get("stream").is_some_and(|v| !v.is_boolean())
    {
        return Err(unsupported());
    }
    if !body["input"].is_string()
        && !body["input"]
            .as_array()
            .is_some_and(|items| !items.is_empty() && items.iter().all(item))
    {
        return Err(unsupported());
    }
    if body
        .get("reasoning")
        .is_some_and(|value| !only(value, &["effort"]) || !value["effort"].is_string())
    {
        return Err(unsupported());
    }
    if let Some(tools) = body.get("tools") {
        let tools = tools.as_array().ok_or_else(unsupported)?;
        let mut names = HashSet::new();
        if tools.iter().any(|tool| {
            !only(
                tool,
                &["type", "name", "description", "parameters", "strict"],
            ) || tool["type"] != "function"
                || !text(&tool["name"])
                || tool["name"]
                    .as_str()
                    .is_some_and(|name| name.trim() != name || !names.insert(name))
                || tool.get("strict").is_some_and(|v| v != false)
                || tool.get("description").is_some_and(|v| !v.is_string())
                || tool
                    .get("parameters")
                    .is_some_and(|v| !v.is_object() && !v.is_boolean())
        }) {
            return Err(unsupported());
        }
    }
    // The general converter drops choice/parallel when tools are absent. Check
    // those requests here instead of silently erasing an execution constraint.
    let has_tools = body["tools"].as_array().is_some_and(|v| !v.is_empty());
    if let Some(choice) = body.get("tool_choice") {
        match choice.as_str() {
            Some("auto" | "none") => {}
            Some("required") if has_tools => {}
            None if has_tools
                && only(choice, &["type", "name"])
                && choice["type"] == "function"
                && text(&choice["name"]) => {}
            _ => return Err(unsupported()),
        }
    }
    if body
        .get("parallel_tool_calls")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(unsupported());
    }
    let mut chat =
        crate::proxy::providers::transform_codex_chat::responses_to_chat_completions(body.clone())?;
    // Raw Responses does not admit these Chat-only fields. Thus only the
    // compatibility converter's exact empty placeholder/usage injection can
    // reach this removal; nonempty data is never silently discarded.
    if let Some(messages) = chat["messages"].as_array_mut() {
        for message in messages {
            if message["role"] == "assistant" && message["reasoning_content"] == "" {
                message.as_object_mut().unwrap().remove("reasoning_content");
            }
        }
    }
    if chat.get("stream_options") == Some(&serde_json::json!({"include_usage": true})) {
        chat.as_object_mut().unwrap().remove("stream_options");
    }
    if !linked_history(&chat) || !chat_tools(&chat) {
        return Err(unsupported());
    }
    Ok(chat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn history() -> Value {
        json!({"model":"fixture", "input":[{"type":"function_call","call_id":"call1","name":"inspect","arguments":"{}"},{"type":"function_call_output","call_id":"call1","output":"result"}]})
    }
    #[test]
    fn original_converter_preserves_call_result_and_drops_only_generated_placeholders() {
        let chat = chat_request(&history(), "/v1/responses").unwrap();
        assert_eq!(chat["messages"][0]["tool_calls"][0]["id"], "call1");
        assert_eq!(chat["messages"][1]["tool_call_id"], "call1");
        assert_eq!(chat["messages"][1]["content"], "result");
        assert!(chat["messages"][0].get("reasoning_content").is_none());
    }
    #[test]
    fn malformed_history_cannot_be_repaired_into_plain_text() {
        for key in ["call_id", "name", "arguments"] {
            let mut body = history();
            body["input"][0].as_object_mut().unwrap().remove(key);
            assert!(chat_request(&body, "/responses").is_err());
        }
        let mut body = history();
        body["input"][0]["arguments"] = json!("{cut");
        assert!(chat_request(&body, "/responses").is_err());
        let mut body = history();
        body["input"][1].as_object_mut().unwrap().remove("output");
        assert!(chat_request(&body, "/responses").is_err());
    }
    #[test]
    fn raw_usage_recovery_and_custom_tools_are_not_silently_erased() {
        for key in ["stream_options", "previous_response_id"] {
            let mut body = history();
            body[key] = json!({});
            assert!(chat_request(&body, "/responses").is_err());
        }
        let mut body = history();
        body["tools"] = json!([{"type":"custom","name":"run"}]);
        assert!(chat_request(&body, "/responses").is_err());
        let mut body = history();
        body["tool_choice"] = json!("required");
        assert!(chat_request(&body, "/responses").is_err());
    }
    #[test]
    fn orphan_duplicate_and_unfinished_tools_fail_before_worker() {
        let mut body = history();
        body["input"].as_array_mut().unwrap().remove(0);
        assert!(chat_request(&body, "/responses").is_err());
        let mut body = history();
        body["input"].as_array_mut().unwrap().pop();
        assert!(chat_request(&body, "/responses").is_err());
        let mut body = history();
        let result = body["input"][1].clone();
        body["input"].as_array_mut().unwrap().push(result);
        assert!(chat_request(&body, "/responses").is_err());
        assert!(chat_request(
            &json!({"messages":[{"role":"tool","tool_call_id":"orphan","content":"x"}]}),
            "/chat/completions"
        )
        .is_err());
    }
    #[test]
    fn converter_cannot_deduplicate_or_discard_raw_tool_definitions() {
        let mut body = history();
        body["tools"] = json!([
            {"type":"function","name":"inspect","parameters":{"type":"object"}},
            {"type":"function","name":"inspect","parameters":{"type":"object","required":["x"]}}
        ]);
        assert!(chat_request(&body, "/responses").is_err());
        for name in [" ", " inspect", "inspect "] {
            body["tools"] = json!([{"type":"function","name":name}]);
            assert!(chat_request(&body, "/responses").is_err());
        }
    }
    #[test]
    fn original_response_output_can_be_replayed_without_editing_annotations() {
        let response = crate::proxy::providers::transform_codex_chat::chat_completion_to_response(json!({
            "id":"chat_fixture", "model":"fixture", "choices":[{"index":0,"message":{"role":"assistant","content":"answer"},"finish_reason":"stop"}]
        })).unwrap();
        assert_eq!(
            response["output"][0]["content"][0]["annotations"],
            json!([])
        );
        let mut input = vec![json!({"role":"user","content":"question"})];
        input.extend(response["output"].as_array().unwrap().clone());
        input.push(json!({"role":"user","content":"continue"}));
        assert!(chat_request(&json!({"model":"fixture","input":input}), "/responses").is_ok());
    }
    #[test]
    fn incoming_executable_and_auth_hints_remain_inert() {
        let body = serde_json::json!({"model":"fixture", "messages":[{"role":"user","content":"hello"}],
            "options":{"auth":{"accessToken":"untrusted"}}, "nodePath":"untrusted", "sdkRoot":"untrusted"});
        let chat = chat_request(&body, "/v1/chat/completions").unwrap();
        assert_eq!(chat["messages"], body["messages"]);
        for key in ["options", "nodePath", "sdkRoot"] {
            assert!(chat.get(key).is_none());
        }
    }
}
