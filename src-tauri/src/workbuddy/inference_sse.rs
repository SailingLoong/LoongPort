//! Private bounded Chat SSE screening. Events are retained byte-for-byte, never
//! rewritten. Known credentials stay inside this owner and the sole KMP helper.
use crate::proxy::providers::codex_chat_common::{
    extract_reasoning_field_text, split_leading_think_block, strip_leading_think_open_tag,
};
use crate::secrets::stream_guard::KnownSecretGuard;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
};
use zeroize::Zeroizing;

const MAX_EVENT: usize = 256 * 1024;
const MAX_HELD: usize = 1024 * 1024;
const MAX_RETAINED: usize = 2 * 1024 * 1024;
const MAX_TOOLS: usize = 16;
fn unsafe_body() -> io::Error {
    io::Error::other("workbuddy.unsafe_response_body")
}
fn feed(guard: &mut KnownSecretGuard, text: &[u8]) -> Result<(), io::Error> {
    guard.feed(Bytes::copy_from_slice(text))?;
    Ok(())
}
struct Tool {
    name: KnownSecretGuard,
    id: KnownSecretGuard,
    arguments: KnownSecretGuard,
    reasoning: KnownSecretGuard,
    reasoning_nonempty: bool,
    json: Zeroizing<String>,
    finished: bool,
}
/// Only suffixes are needed to seed the original converter's trim-and-copy
/// reasoning field on a newly opened tool. Full reasoning is not accumulated.
#[derive(Default)]
struct ReasonTail {
    raw: Vec<u8>,
    trimmed: Vec<u8>,
    nonempty: bool,
}
impl ReasonTail {
    fn append(&mut self, text: &str, limit: usize) {
        if !text.trim_end().is_empty() {
            let mut tail = self.raw.clone();
            tail.extend_from_slice(text.trim_end().as_bytes());
            self.trimmed = tail[tail.len().saturating_sub(limit)..].to_vec();
            self.nonempty = true;
        }
        self.raw.extend_from_slice(text.as_bytes());
        self.raw.drain(..self.raw.len().saturating_sub(limit));
    }
}
pub(super) struct ChatGuard {
    secrets: Zeroizing<Vec<Vec<u8>>>,
    raw: Option<KnownSecretGuard>,
    literal: KnownSecretGuard,
    content: KnownSecretGuard,
    reasoning: KnownSecretGuard,
    tools: BTreeMap<u64, Tool>,
    reasoning_tail: ReasonTail,
    inline: Option<String>,
    buffer: Vec<u8>,
    held: Vec<u8>,
    blocked: Arc<AtomicBool>,
    denial: Arc<AtomicU32>,
    done: bool,
}
impl ChatGuard {
    pub(super) fn new(
        secrets: &[&[u8]],
        blocked: Arc<AtomicBool>,
        denial: Arc<AtomicU32>,
    ) -> Result<Self, io::Error> {
        // NUL is the independent-field delimiter below; validated credentials
        // cannot contain controls. Never broaden this to arbitrary redaction.
        if secrets.iter().any(|s| s.contains(&0)) {
            return Err(unsafe_body());
        }
        Ok(Self {
            secrets: Zeroizing::new(secrets.iter().map(|s| s.to_vec()).collect()),
            raw: Some(KnownSecretGuard::new(secrets)?),
            literal: KnownSecretGuard::new(secrets)?,
            content: KnownSecretGuard::new(secrets)?,
            reasoning: KnownSecretGuard::new(secrets)?,
            tools: BTreeMap::new(),
            reasoning_tail: ReasonTail::default(),
            inline: Some(String::new()),
            buffer: Vec::new(),
            held: Vec::new(),
            blocked,
            denial,
            done: false,
        })
    }
    fn fresh(&self) -> Result<KnownSecretGuard, io::Error> {
        KnownSecretGuard::new(&self.secrets.iter().map(Vec::as_slice).collect::<Vec<_>>())
    }
    fn literal(&mut self, text: &str) -> Result<(), io::Error> {
        feed(&mut self.literal, text.as_bytes())?;
        feed(&mut self.literal, &[0])
    }
    fn scan_value(&mut self, value: &Value, depth: usize) -> Result<(), io::Error> {
        if depth > 64 {
            return Err(unsafe_body());
        }
        match value {
            Value::String(text) => self.literal(text)?,
            Value::Array(values) => {
                for value in values {
                    self.scan_value(value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                for (key, value) in values {
                    self.literal(key)?;
                    self.scan_value(value, depth + 1)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    fn reasoning(&mut self, text: &str, append_tools: bool) -> Result<(), io::Error> {
        feed(&mut self.reasoning, text.as_bytes())?;
        let limit = self
            .secrets
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .saturating_sub(1);
        self.reasoning_tail.append(text, limit);
        if append_tools && !text.trim().is_empty() {
            for tool in self.tools.values_mut() {
                let value = if tool.reasoning_nonempty {
                    text
                } else {
                    text.trim_start()
                };
                feed(&mut tool.reasoning, value.as_bytes())?;
                tool.reasoning_nonempty = true;
            }
        }
        Ok(())
    }
    fn inline(&mut self, text: Option<&str>, boundary: bool) -> Result<(), io::Error> {
        let Some(mut buffer) = self.inline.take() else {
            return Ok(());
        };
        if let Some(text) = text {
            buffer.push_str(text);
        }
        if buffer.len() > MAX_EVENT {
            return Err(unsafe_body());
        }
        if let Some((reasoning, _)) = split_leading_think_block(&buffer) {
            self.reasoning(&reasoning, false)?;
        } else if boundary {
            if let Some(reasoning) = strip_leading_think_open_tag(&buffer) {
                self.reasoning(&reasoning, false)?;
            }
        } else {
            let trimmed = buffer.trim_start();
            if trimmed.is_empty()
                || "<think>".starts_with(trimmed)
                || trimmed.starts_with("<think>")
            {
                self.inline = Some(buffer);
            }
        }
        Ok(())
    }
    fn finish_tools(&mut self) -> Result<(), io::Error> {
        // Tool JSON is intentionally held until its logical end. The original
        // converters canonicalize/decode arguments (including custom-tool input).
        let pending: Vec<_> = self
            .tools
            .values_mut()
            .filter(|t| !t.finished)
            .map(|t| {
                t.finished = true;
                std::mem::take(&mut *t.json)
            })
            .collect();
        for json in pending {
            if !json.trim().is_empty() {
                let value: Value = serde_json::from_str(&json).map_err(|_| unsafe_body())?;
                self.scan_value(&value, 0)?;
            }
        }
        Ok(())
    }
    fn check_event(&mut self, event: &[u8]) -> Result<(), io::Error> {
        if event.len() > MAX_EVENT {
            return Err(unsafe_body());
        }
        let text = std::str::from_utf8(event).map_err(|_| unsafe_body())?;
        let data = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data:")
                    .map(|value| value.strip_prefix(' ').unwrap_or(value))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            return Ok(());
        }
        if self.done {
            return Err(unsafe_body());
        }
        if data.trim() == "[DONE]" {
            self.inline(None, true)?;
            self.finish_tools()?;
            self.done = true;
            return Ok(());
        }
        let value: Value = serde_json::from_str(&data).map_err(|_| unsafe_body())?;
        // Only exact structured refusal codes establish this terminal boundary.
        // Keep the code, never upstream message/body bytes. This must run before
        // original Chat/Claude/Responses consumers can reinterpret the event.
        if let Some(code @ (11004 | 11140)) = super::upstream_error_code(&value) {
            self.denial.store(code as u32, Ordering::Release);
            return Err(unsafe_body());
        }
        self.scan_value(&value, 0)?;
        let Some(choices) = value.get("choices").and_then(Value::as_array) else {
            return Ok(());
        };
        if choices.len() > 1 {
            return Err(unsafe_body());
        }
        let Some(choice) = choices.first() else {
            return Ok(());
        };
        if choice
            .get("index")
            .is_some_and(|index| index.as_u64() != Some(0))
        {
            return Err(unsafe_body());
        }
        if let Some(delta) = choice.get("delta") {
            if let Some(reasoning) = extract_reasoning_field_text(delta) {
                self.reasoning(&reasoning, true)?;
            }
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                feed(&mut self.content, content.as_bytes())?;
                self.inline(Some(content), false)?;
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                self.inline(None, true)?;
                for call in calls {
                    let index = call
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(unsafe_body)?;
                    if !self.tools.contains_key(&index) {
                        if self.tools.len() >= MAX_TOOLS {
                            return Err(unsafe_body());
                        }
                        let mut tool = Tool {
                            name: self.fresh()?,
                            id: self.fresh()?,
                            arguments: self.fresh()?,
                            reasoning: self.fresh()?,
                            reasoning_nonempty: self.reasoning_tail.nonempty,
                            json: Zeroizing::new(String::new()),
                            finished: false,
                        };
                        feed(&mut tool.reasoning, &self.reasoning_tail.trimmed)?;
                        self.tools.insert(index, tool);
                    }
                    let tool = self.tools.get_mut(&index).ok_or_else(unsafe_body)?;
                    if tool.finished {
                        return Err(unsafe_body());
                    }
                    // Independent tool indices and fields. Name/id concatenate in
                    // raw Chat clients, while original converters replace them;
                    // checking both full fields and this stream covers both paths.
                    if let Some(id) = call.get("id").and_then(Value::as_str) {
                        feed(&mut tool.id, id.as_bytes())?;
                    }
                    if let Some(function) = call.get("function") {
                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                            feed(&mut tool.name, name.as_bytes())?;
                        }
                        if let Some(args) = function.get("arguments").and_then(Value::as_str) {
                            if tool.finished && !args.is_empty() {
                                return Err(unsafe_body());
                            }
                            feed(&mut tool.arguments, args.as_bytes())?;
                            tool.json.push_str(args);
                            if tool.json.len() > MAX_EVENT {
                                return Err(unsafe_body());
                            }
                        }
                    }
                }
            }
        }
        if choice.get("finish_reason").is_some_and(|v| !v.is_null()) {
            self.inline(None, true)?;
            self.finish_tools()?;
        }
        Ok(())
    }
    fn pending(&self) -> bool {
        self.content.has_pending()
            || self.reasoning.has_pending()
            || self.tools.values().any(|t| {
                t.reasoning.has_pending()
                    || (!t.finished
                        && (t.name.has_pending()
                            || t.id.has_pending()
                            || t.arguments.has_pending()
                            || !t.json.is_empty()))
            })
    }
    fn budget(&self) -> Result<(), io::Error> {
        let retained = self.buffer.len()
            + self.held.len()
            + self.inline.as_ref().map_or(0, String::len)
            + self.tools.values().map(|t| t.json.len()).sum::<usize>();
        if self.held.len() > MAX_HELD || retained > MAX_RETAINED {
            Err(unsafe_body())
        } else {
            Ok(())
        }
    }
    fn event(&mut self) -> Option<Vec<u8>> {
        let mut best = None;
        for delimiter in [b"\r\n\r\n".as_slice(), b"\r\n\n", b"\n\r\n", b"\n\n"] {
            if let Some(index) = self
                .buffer
                .windows(delimiter.len())
                .position(|w| w == delimiter)
            {
                let end = index + delimiter.len();
                if best.is_none_or(|old| end < old) {
                    best = Some(end);
                }
            }
        }
        best.map(|end| self.buffer.drain(..end).collect())
    }
    fn block(&self) -> io::Error {
        let code = self.denial.load(Ordering::Acquire);
        if matches!(code, 11004 | 11140) {
            io::Error::other(super::safe_error_value(200, Some(i64::from(code)), true).to_string())
        } else {
            self.blocked.store(true, Ordering::Release);
            unsafe_body()
        }
    }
    pub(super) fn wrap_stream<S, E>(
        self,
        stream: S,
    ) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Send + 'static,
    {
        futures::stream::unfold(
            (self, Box::pin(stream), false, false),
            |(mut guard, mut stream, eof, stopped)| async move {
                if stopped {
                    return None;
                }
                let mut eof = eof;
                loop {
                    if let Some(event) = guard.event() {
                        if guard.check_event(&event).is_err() {
                            return Some((Err(guard.block()), (guard, stream, eof, true)));
                        }
                        guard.held.extend_from_slice(&event);
                        if guard.budget().is_err() {
                            return Some((Err(guard.block()), (guard, stream, eof, true)));
                        }
                        if guard.done || !guard.pending() {
                            let bytes = Bytes::from(std::mem::take(&mut guard.held));
                            return Some((Ok(bytes), (guard, stream, eof, false)));
                        }
                        continue;
                    }
                    if guard.buffer.len() > MAX_EVENT {
                        return Some((Err(guard.block()), (guard, stream, eof, true)));
                    }
                    if eof {
                        // Unterminated non-whitespace SSE bytes are ambiguous and
                        // cannot safely be replayed through a different parser.
                        if guard.buffer.iter().any(|b| !b.is_ascii_whitespace())
                            || guard.inline(None, true).is_err()
                            || guard.finish_tools().is_err()
                        {
                            return Some((Err(guard.block()), (guard, stream, eof, true)));
                        }
                        guard
                            .held
                            .extend_from_slice(&std::mem::take(&mut guard.buffer));
                        let tail = std::mem::take(&mut guard.held);
                        return (!tail.is_empty())
                            .then(|| (Ok(Bytes::from(tail)), (guard, stream, eof, true)));
                    }
                    match stream.next().await {
                        Some(Ok(bytes)) => {
                            if bytes.len() > MAX_RETAINED {
                                return Some((Err(guard.block()), (guard, stream, eof, true)));
                            }
                            match guard
                                .raw
                                .as_mut()
                                .ok_or_else(unsafe_body)
                                .and_then(|raw| raw.feed(bytes))
                            {
                                Ok(Some(bytes)) => guard.buffer.extend_from_slice(&bytes),
                                Ok(None) => (),
                                Err(_) => {
                                    return Some((Err(guard.block()), (guard, stream, eof, true)))
                                }
                            }
                            if guard.budget().is_err() {
                                return Some((Err(guard.block()), (guard, stream, eof, true)));
                            }
                        }
                        Some(Err(_)) => {
                            return Some((
                                Err(io::Error::other("workbuddy.stream_interrupted")),
                                (guard, stream, eof, true),
                            ))
                        }
                        None => {
                            if let Some(tail) = guard.raw.take().and_then(KnownSecretGuard::finish)
                            {
                                guard.buffer.extend_from_slice(&tail);
                            }
                            eof = true;
                        }
                    }
                }
            },
        )
    }
}
