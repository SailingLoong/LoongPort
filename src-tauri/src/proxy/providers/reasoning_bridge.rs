//! Opaque reasoning transport helpers shared by the Messages ↔ Responses bridge.
//!
//! The Anthropic Messages protocol has no field for an OpenAI Responses
//! `reasoning` item. To keep stateless tool loops lossless, the item is
//! carried in a versioned thinking signature/redacted-thinking payload and
//! restored when the client replays the assistant message.
//!
//! Only fields the Responses input schema accepts on a `reasoning` item are
//! transported. Backends periodically grow output-only fields on reasoning
//! items (the official Codex backend added `status` on 2026-10-03) and reject
//! them as unknown parameters once replayed into `input`, so both the storing
//! side and the restoring side keep the same whitelist. Restoring must stay
//! whitelisted forever: envelopes written before this constraint sit in
//! existing client histories and still carry the raw output fields.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};

pub(crate) const OPENAI_REASONING_ITEM_PREFIX: &str = "ccswitch-openai-reasoning-v1:";

/// The only fields safe to send back as a `reasoning` input item:
/// `encrypted_content` is the stateless-replay payload, `id`/`summary` are
/// accepted context, and everything else the backend emits is output-only.
const REPLAYABLE_REASONING_FIELDS: &[&str] = &["type", "id", "summary", "encrypted_content"];

fn sanitize_reasoning_item(item: &Value) -> Value {
    let mut sanitized = serde_json::Map::new();
    for field in REPLAYABLE_REASONING_FIELDS {
        if let Some(value) = item.get(*field) {
            sanitized.insert((*field).to_string(), value.clone());
        }
    }
    Value::Object(sanitized)
}

pub(crate) fn reasoning_summary_text(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("summary_text" | "reasoning_text")
            )
            .then(|| part.get("text").and_then(Value::as_str))
            .flatten()
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(crate) fn encode_openai_reasoning_item(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let bytes = serde_json::to_vec(&sanitize_reasoning_item(item)).ok()?;
    Some(format!(
        "{OPENAI_REASONING_ITEM_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub(crate) fn decode_openai_reasoning_item(encoded: &str) -> Option<Value> {
    let payload = encoded.strip_prefix(OPENAI_REASONING_ITEM_PREFIX)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let item: Value = serde_json::from_slice(&bytes).ok()?;
    (item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .then(|| sanitize_reasoning_item(&item))
}

pub(crate) fn anthropic_block_from_openai_reasoning_item(item: &Value) -> Option<Value> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }

    let text = reasoning_summary_text(item);
    let has_encrypted_content = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty());

    if has_encrypted_content {
        // An encrypted-only item (no public summary) is carried as an empty
        // thinking block, the same shape Anthropic returns for omitted
        // thinking. Claude Code's VS Code extension cannot render
        // `redacted_thinking`, and replay decodes both carriers identically.
        let envelope = encode_openai_reasoning_item(item)?;
        return Some(json!({
            "type": "thinking",
            "thinking": text,
            "signature": envelope
        }));
    }

    (!text.is_empty()).then(|| {
        json!({
            "type": "thinking",
            "thinking": text
        })
    })
}

pub(crate) fn openai_reasoning_item_from_anthropic_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking") => block
            .get("signature")
            .and_then(Value::as_str)
            .and_then(decode_openai_reasoning_item),
        Some("redacted_thinking") => block
            .get("data")
            .and_then(Value::as_str)
            .and_then(decode_openai_reasoning_item),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_reasoning_item_round_trips_through_thinking_signature() {
        let item = json!({
            "id": "rs_1",
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "Need a tool."}],
            "encrypted_content": "opaque"
        });
        let block = anthropic_block_from_openai_reasoning_item(&item).unwrap();
        assert_eq!(block["type"], "thinking");
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&block),
            Some(item)
        );
    }

    #[test]
    fn encrypted_only_new_and_legacy_carriers_replay_identical_multiturn_tools() {
        use super::super::transform_responses::{anthropic_to_responses, responses_to_anthropic};

        let mut new_history = Vec::new();
        let mut old_history = Vec::new();
        let mut expected = Vec::new();
        for round in 0..2 {
            let original = json!({
                "id": format!("rs_{round}"), "type": "reasoning", "summary": [],
                "encrypted_content": format!("opaque_{round}"), "status": "completed",
                "content": [{"type": "output_text", "text": "not replayable"}]
            });
            let response = responses_to_anthropic(json!({
                "id": format!("resp_{round}"), "model": "gpt-5.6",
                "status": "completed", "output": [original],
                "usage": {"input_tokens": 2, "output_tokens": 1}
            }))
            .unwrap();
            let block = response["content"][0].clone();
            assert_eq!(block["type"], "thinking");
            assert_eq!(block["thinking"], "");
            let legacy = json!({"type": "redacted_thinking", "data": block["signature"]});
            let tool = json!({
                "type": "tool_use", "id": format!("call_{round}"),
                "name": "lookup", "input": {"round": round}
            });
            let result = json!({
                "role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": format!("call_{round}"),
                    "content": "ok"
                }]
            });
            new_history.push(json!({"role": "assistant", "content": [block, tool]}));
            new_history.push(result.clone());
            old_history.push(json!({"role": "assistant", "content": [legacy, tool]}));
            old_history.push(result);
            expected.push(json!({
                "id": format!("rs_{round}"), "type": "reasoning", "summary": [],
                "encrypted_content": format!("opaque_{round}")
            }));
        }
        let replay = |messages| {
            anthropic_to_responses(
                json!({"model": "gpt-5.6", "messages": messages}),
                None,
                true,
                false,
            )
            .unwrap()
        };
        let new_request = replay(new_history);
        let old_request = replay(old_history);
        assert_eq!(new_request, old_request);
        let input = new_request["input"].as_array().unwrap();
        let reasoning: Vec<_> = input
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .cloned()
            .collect();
        assert_eq!(reasoning, expected);
        assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "function_call")
                .count(),
            2
        );
        assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "function_call_output")
                .count(),
            2
        );
    }

    #[test]
    fn encrypted_item_without_summary_uses_empty_thinking() {
        let item = json!({
            "id": "rs_2",
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "opaque"
        });
        let block = anthropic_block_from_openai_reasoning_item(&item).unwrap();
        assert_eq!(block["type"], "thinking");
        assert_eq!(block["thinking"], "");
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&block),
            Some(item.clone())
        );
        // Histories written by older versions still carry redacted_thinking.
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&json!({
                "type": "redacted_thinking",
                "data": block["signature"]
            })),
            Some(item)
        );
    }

    #[test]
    fn replay_drops_backend_only_fields() {
        let item = json!({
            "id": "rs_3",
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "Need a tool."}],
            "encrypted_content": "opaque",
            "status": "completed",
            "content": [{"type": "reasoning_text", "text": "output-only plaintext"}]
        });
        let block = anthropic_block_from_openai_reasoning_item(&item).unwrap();
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&block),
            Some(json!({
                "id": "rs_3",
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "Need a tool."}],
                "encrypted_content": "opaque"
            }))
        );
    }

    #[test]
    fn decode_strips_fields_from_legacy_envelopes() {
        // Envelopes written before the whitelist kept the backend output item
        // verbatim (e.g. `status`, plaintext `content`); replay must drop
        // those fields even though encode no longer stores them.
        let legacy = json!({
            "id": "rs_4",
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "opaque",
            "status": "completed",
            "content": [{"type": "reasoning_text", "text": "legacy"}]
        });
        let encoded = format!(
            "{OPENAI_REASONING_ITEM_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&legacy).unwrap())
        );
        let expected = json!({
            "id": "rs_4",
            "type": "reasoning",
            "summary": [],
            "encrypted_content": "opaque"
        });
        assert_eq!(
            decode_openai_reasoning_item(&encoded),
            Some(expected.clone())
        );
        // Same guarantee through the redacted_thinking carrier.
        assert_eq!(
            openai_reasoning_item_from_anthropic_block(&json!({
                "type": "redacted_thinking",
                "data": encoded
            })),
            Some(expected)
        );
    }
}
