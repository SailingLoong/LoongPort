//! Observe native Anthropic completion without rewriting any response bytes.
use crate::proxy::sse::{append_utf8_safe, strip_sse_field, take_sse_block};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

// Only the tiny message_stop envelope needs JSON inspection. Large text/tool
// frames still pass unchanged; discard their observation copy until a boundary.
const MAX_STOP_EVENT: usize = 16 * 1024;
// Retain one original chunk completing message_stop, plus a bounded inert tail.
const MAX_TERMINAL_TAIL: usize = 64 * 1024;

fn response_event(name: &str) -> bool {
    matches!(
        name,
        "error"
            | "message_start"
            | "message_delta"
            | "message_stop"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
    )
}

#[derive(Default)]
struct Completion {
    buffer: String,
    remainder: Vec<u8>,
    stopped: bool,
    skipping: bool,
    invalid: bool,
}
impl Completion {
    fn feed(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(4096) {
            append_utf8_safe(&mut self.buffer, &mut self.remainder, chunk);
            while let Some(block) = take_sse_block(&mut self.buffer) {
                if self.skipping {
                    self.skipping = false;
                    continue;
                }
                let name = block
                    .lines()
                    .filter_map(|line| strip_sse_field(line, "event"))
                    .next_back()
                    .map(str::trim);
                if !self.stopped && name.is_some_and(|name| name != "message_stop") {
                    continue;
                }
                let data = block
                    .lines()
                    .filter_map(|line| strip_sse_field(line, "data"))
                    .collect::<Vec<_>>();
                // SSE fields without data do not dispatch an application event.
                if data.is_empty() {
                    continue;
                }
                let data = data.join("\n");
                let value = serde_json::from_str::<Value>(&data).ok();
                let kind = value.as_ref().and_then(|v| v["type"].as_str());
                // message_stop is final. Comments, ping and unknown extensions
                // remain inert, but another response/error event cannot revive it.
                if self.stopped
                    && (name.is_some_and(response_event) || kind.is_some_and(response_event))
                {
                    self.invalid = true;
                }
                if kind == Some("message_stop") && name.is_none_or(|name| name == "message_stop") {
                    self.stopped = true;
                }
            }
            if self.buffer.len() > MAX_STOP_EVENT {
                self.skipping = true;
                // A terminal tail too large to inspect cannot prove success.
                self.invalid |= self.stopped;
                // Retain only possible CR/LF boundary bytes, never a suffix
                // containing a nested or quoted message_stop lookalike.
                let tail = self.buffer.len()
                    - self
                        .buffer
                        .bytes()
                        .rev()
                        .take_while(|b| matches!(b, b'\r' | b'\n'))
                        .take(3)
                        .count();
                self.buffer.drain(..tail);
            }
        }
    }

    fn complete(&self) -> bool {
        self.stopped
            && !self.invalid
            && !self.skipping
            && self.remainder.is_empty()
            && self
                .buffer
                .lines()
                .all(|line| line != "data" && !line.starts_with("data:"))
    }
}

pub(super) fn observe_completion<S>(
    stream: S,
    streaming: bool,
    incomplete: Arc<AtomicBool>,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + 'static,
{
    async_stream::stream! {
        let mut stream = Box::pin(stream);
        let mut completion = streaming.then(Completion::default);
        let mut terminal = Vec::new();
        let mut tail_bytes = 0usize;
        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(bytes) => bytes,
                Err(error) => { yield Err(error); return; }
            };
            if let Some(completion) = &mut completion {
                completion.feed(&bytes);
                if completion.invalid {
                    incomplete.store(true, Ordering::Release);
                    yield Err(io::Error::other("minimax_code.incomplete_stream"));
                    return;
                }
                if completion.stopped {
                    if !terminal.is_empty() {
                        tail_bytes = tail_bytes.saturating_add(bytes.len());
                        if tail_bytes > MAX_TERMINAL_TAIL {
                            incomplete.store(true, Ordering::Release);
                            yield Err(io::Error::other("minimax_code.incomplete_stream"));
                            return;
                        }
                    }
                    // A converter finalizes immediately on message_stop. Do not
                    // expose its final delimiter before checking the native EOF.
                    if !bytes.is_empty() { terminal.push(bytes); }
                    continue;
                }
            }
            yield Ok(bytes);
        }
        if completion.as_ref().is_some_and(|c| !c.complete()) {
            incomplete.store(true, Ordering::Release);
            yield Err(io::Error::other("minimax_code.incomplete_stream"));
            return;
        }
        for bytes in terminal { yield Ok(bytes); }
    }
}
