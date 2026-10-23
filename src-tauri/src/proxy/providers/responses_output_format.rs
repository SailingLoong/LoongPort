//! Lossless output-format mapping at the Responses protocol boundary.
//!
//! This describes wire formats, not model capabilities. A Chat endpoint may still
//! reject a format for its selected model; callers must not retry by removing it.

use crate::proxy::error::ProxyError;
use serde_json::{json, Value};

/// Keep the legacy Chat extension opaque when no Responses format was supplied.
/// When both are supplied, neither constraint may silently override the other.
/// Legacy null imposes no constraint, but remains unchanged on the Chat-only path.
pub(super) fn chat_response_format(body: &Value) -> Result<Option<Value>, ProxyError> {
    let legacy = body.get("response_format");
    let Some(text) = body.get("text") else {
        return Ok(legacy.cloned());
    };
    let text = text
        .as_object()
        .ok_or_else(|| invalid("text must be an object"))?;
    let Some(format) = text.get("format") else {
        return Ok(legacy.cloned());
    };
    let format = format
        .as_object()
        .ok_or_else(|| invalid("text.format must be an object"))?;
    let kind = format.get("type").and_then(Value::as_str);
    let mapped = match kind {
        Some("text" | "json_object") => {
            if format.len() != 1 {
                return Err(invalid("unsupported fields in text.format"));
            }
            Value::Object(format.clone())
        }
        Some("json_schema") => {
            if format.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "type" | "name" | "schema" | "description" | "strict"
                )
            }) {
                return Err(invalid("unsupported fields in text.format for json_schema"));
            }
            let name = format.get("name").and_then(Value::as_str);
            if !name.is_some_and(|name| {
                !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            }) {
                return Err(invalid("text.format.name must contain 1-64 ASCII letters, digits, underscores or dashes"));
            }
            if !format.get("schema").is_some_and(Value::is_object) {
                return Err(invalid("text.format.schema must be a JSON Schema object"));
            }
            if format
                .get("description")
                .is_some_and(|value| !value.is_string())
            {
                return Err(invalid("text.format.description must be a string"));
            }
            if format
                .get("strict")
                .is_some_and(|value| !value.is_boolean() && !value.is_null())
            {
                return Err(invalid("text.format.strict must be a boolean or null"));
            }
            // Clone the complete schema; do not prune unsupported schema keywords,
            // default strict, or invent a name. Schema support belongs upstream.
            let mut schema = format.clone();
            schema.remove("type");
            json!({"type": "json_schema", "json_schema": schema})
        }
        _ => {
            return Err(invalid(
                "text.format.type must be text, json_object or json_schema",
            ))
        }
    };
    if legacy.is_some_and(|legacy| !legacy.is_null() && legacy != &mapped) {
        return Err(invalid("text.format conflicts with response_format"));
    }
    Ok(Some(mapped))
}

/// The current Anthropic bridge has no proven lossless mapping for these
/// constraints (including OpenAI name/description/strict). Fail before forwarding
/// instead of dropping them or encoding them as system instructions.
pub(super) fn ensure_anthropic_output_format_supported(body: &Value) -> Result<(), ProxyError> {
    if let Some(format) = chat_response_format(body)? {
        if !format.is_null() && format != json!({"type": "text"}) {
            return Err(invalid("structured output is not supported losslessly by the Responses-to-Anthropic bridge; use a compatible Responses or Chat endpoint"));
        }
    }
    Ok(())
}

fn invalid(message: &str) -> ProxyError {
    ProxyError::TransformError(message.to_string())
}
