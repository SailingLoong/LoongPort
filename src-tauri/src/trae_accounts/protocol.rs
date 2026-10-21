//! Wire facts: magpie-community/plugins@5a836745, packages/trae/index.mjs.
//! MIT notice: licenses/magpie-trae-MIT.txt. No device synthesis or refusal retries.
use serde_json::{json, Value};
pub(crate) fn validate_request(body: &Value) -> Result<(), &'static str> {
    let object = body.as_object().ok_or("trae.unsupported_request")?;
    if object.keys().any(|k| {
        !matches!(
            k.as_str(),
            "model"
                | "messages"
                | "stream"
                | "stream_options"
                | "tools"
                | "max_tokens"
                | "max_completion_tokens"
                | "temperature"
        )
    }) {
        return Err("trae.unsupported_request");
    }
    if body
        .get("tools")
        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err("trae.text_only");
    }
    body["model"]
        .as_str()
        .filter(|s| super::store::text(s, 256))
        .ok_or("trae.invalid_model")?;
    let messages = body["messages"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 4096)
        .ok_or("trae.invalid_messages")?;
    for m in messages {
        let o = m.as_object().ok_or("trae.text_only")?;
        if o.keys().any(|k| !matches!(k.as_str(), "role" | "content")) {
            return Err("trae.text_only");
        }
        m["role"]
            .as_str()
            .filter(|r| matches!(*r, "system" | "user" | "assistant"))
            .ok_or("trae.text_only")?;
        m["content"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 1024 * 1024)
            .ok_or("trae.text_only")?;
    }
    if let Some(n) = body.get("max_completion_tokens").or(body.get("max_tokens")) {
        n.as_u64()
            .filter(|n| *n > 0 && *n <= i32::MAX as u64)
            .ok_or("trae.invalid_limit")?;
    }
    if body.get("temperature").is_some_and(|t| !t.is_number()) {
        return Err("trae.invalid_temperature");
    }
    if body.get("stream").is_some_and(|t| !t.is_boolean()) {
        return Err("trae.unsupported_request");
    }
    if let Some(options) = body.get("stream_options") {
        if options.as_object().is_none_or(|o| {
            o.keys().any(|k| k != "include_usage")
                || o.get("include_usage").is_some_and(|v| !v.is_boolean())
        }) {
            return Err("trae.unsupported_request");
        }
        // Usage accounting is not provided by the selected text-only endpoint.
        if options.get("include_usage").and_then(Value::as_bool) == Some(true) {
            return Err("trae.unsupported_request");
        }
    }
    Ok(())
}
pub(crate) fn request(body: Value, session: &str) -> Result<Value, &'static str> {
    validate_request(&body)?;
    let model = &body["model"];
    let out = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| json!({"role": m["role"], "content":[{"type":"text","text":m["content"]}]}))
        .collect::<Vec<_>>();
    let mut result = json!({"messages":out,"function":"chat_v3","model":model,"config_name":model,"stream":true,"session_id":session,"request_id":uuid::Uuid::new_v4().to_string()});
    if let Some(n) = body.get("max_completion_tokens").or(body.get("max_tokens")) {
        let n = n
            .as_u64()
            .filter(|n| *n > 0 && *n <= i32::MAX as u64)
            .ok_or("trae.invalid_limit")?;
        result["max_tokens"] = json!(n);
    }
    if let Some(t) = body.get("temperature") {
        if !t.is_number() {
            return Err("trae.invalid_temperature");
        }
        result["temperature"] = t.clone();
    }
    Ok(result)
}
pub(crate) fn denial(body: &Value) -> Option<u16> {
    let codes = [
        body.get("code"),
        body.get("Code"),
        body.pointer("/error/code"),
        body.pointer("/error/Code"),
        body.pointer("/ResponseMetadata/Error/Code"),
    ]
    .into_iter()
    .flatten()
    .filter_map(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
    .collect::<Vec<_>>();
    if codes
        .iter()
        .any(|n| (11000..12000).contains(n) || matches!(n, 9074 | 9095 | 20403 | 20405 | 10101))
    {
        return Some(403);
    }
    codes.into_iter().find_map(|n| match n {
        0 => None,
        1001 | 401 | 20101 => Some(401),
        1005 | 4008 | 4011 | 429 => Some(429),
        _ => Some(502),
    })
}
#[derive(Default)]
pub(super) struct Decoder {
    pending: Vec<u8>,
    event: String,
    data: Vec<String>,
    done: bool,
}
pub(super) enum Part {
    Delta(Value),
    Done,
    Error(u16),
}
impl Decoder {
    pub(super) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Part>, ()> {
        if self.done {
            return Ok(vec![]);
        }
        if self.pending.len() + bytes.len() > 1024 * 1024 {
            return Err(());
        }
        self.pending.extend_from_slice(bytes);
        let mut out = vec![];
        while let Some(i) = self.pending.iter().position(|b| *b == b'\n') {
            let raw = self.pending.drain(..=i).collect::<Vec<_>>();
            let line = std::str::from_utf8(&raw[..raw.len() - 1])
                .map_err(|_| ())?
                .trim_end_matches('\r');
            if line.is_empty() {
                if let Some(p) = self.flush()? {
                    let terminal = matches!(p, Part::Error(_) | Part::Done);
                    out.push(p);
                    if terminal {
                        self.done = true;
                        break;
                    }
                }
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                self.data.push(v.trim_start().to_string());
                if self.data.iter().map(String::len).sum::<usize>() > 1024 * 1024 {
                    return Err(());
                }
            }
        }
        Ok(out)
    }
    fn flush(&mut self) -> Result<Option<Part>, ()> {
        if self.data.is_empty() {
            self.event.clear();
            return Ok(None);
        }
        let raw = std::mem::take(&mut self.data).join("\n");
        let event = std::mem::take(&mut self.event).to_ascii_lowercase();
        if raw == "[DONE]" {
            return Ok(Some(Part::Done));
        }
        let d: Value = serde_json::from_str(&raw).map_err(|_| ())?;
        if event == "error" || denial(&d).is_some() {
            return Ok(Some(Part::Error(denial(&d).unwrap_or(502))));
        }
        if matches!(event.as_str(), "done" | "response_done" | "stream_done") {
            return Ok(Some(Part::Done));
        }
        if !matches!(
            event.as_str(),
            "" | "output"
                | "message"
                | "delta"
                | "content"
                | "text"
                | "answer"
                | "chunk"
                | "output_delta"
                | "message_delta"
        ) {
            return Ok(None);
        }
        if d.get("tool_calls").is_some() {
            return Ok(Some(Part::Error(502)));
        }
        let text = d
            .get("response")
            .or_else(|| d.get("content"))
            .or_else(|| d.pointer("/delta/content"))
            .or_else(|| d.get("delta"))
            .and_then(Value::as_str);
        let reasoning = d
            .get("reasoning_content")
            .or_else(|| d.get("reasoning"))
            .or_else(|| d.pointer("/delta/reasoning_content"))
            .and_then(Value::as_str);
        let mut delta = json!({});
        if let Some(t) = text {
            delta["content"] = json!(t)
        }
        if let Some(t) = reasoning {
            delta["reasoning_content"] = json!(t)
        }
        Ok((!delta.as_object().unwrap().is_empty()).then_some(Part::Delta(delta)))
    }
    pub(super) fn finished(&self) -> bool {
        self.done
    }
}
#[cfg(test)]
mod tests;
