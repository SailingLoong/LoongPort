use serde_json::Value;

use crate::relay::model_verification::{
    passive::{
        EvidenceBatch, MAX_RESPONSE_INSPECTION_BYTES, MAX_SSE_EVENT_BYTES, SELF_ID_TAIL_BYTES,
    },
    types::{EvidenceCode, EvidenceFact, EvidenceOutcome, TargetKey},
};

const FOREIGN_SELF_IDENTIFICATION: &[&[u8]] = &[
    b"i am claude",
    b"i'm claude",
    b"anthropic claude",
    b"i am an anthropic model",
    b"i am gpt",
    b"i'm gpt",
    b"openai gpt",
];

/// A bounded reducer for Anthropic Messages SSE. It retains only parser state and one outcome
/// per known evidence code; response text is never retained after an event is reduced.
pub(crate) struct AnthropicPassiveTap {
    target: TargetKey,
    pending: Vec<u8>,
    outcomes: [Option<EvidenceOutcome>; EvidenceCode::CARDINALITY],
    tail: Vec<u8>,
    terminal: bool,
    oversized: bool,
    /// 流式时序：content_block_start(thinking) 先到、signature 靠后续
    /// signature_delta 事件（或非流式的 inline 字段）补齐——缺失判定只能在
    /// finish 时做，见到 thinking 块就判会误报正常流。
    saw_thinking: bool,
    saw_signature: bool,
}

impl AnthropicPassiveTap {
    pub(crate) fn new(target: TargetKey) -> Self {
        Self {
            target,
            pending: Vec::new(),
            outcomes: [None; EvidenceCode::CARDINALITY],
            tail: Vec::with_capacity(SELF_ID_TAIL_BYTES),
            terminal: false,
            oversized: false,
            saw_thinking: false,
            saw_signature: false,
        }
    }

    pub(crate) fn observe_chunk(&mut self, chunk: &[u8]) {
        if self.oversized {
            return;
        }
        for byte in chunk.iter().copied() {
            self.push_tail(byte);
            self.pending.push(byte);
            if self.pending.len() > MAX_SSE_EVENT_BYTES {
                self.oversized = true;
                self.pending.clear();
                return;
            }
            if self.pending.ends_with(b"\n\n") || self.pending.ends_with(b"\r\n\r\n") {
                let event = std::mem::take(&mut self.pending);
                self.observe_event(&event);
            }
        }
    }

    fn push_tail(&mut self, byte: u8) {
        self.tail.push(byte.to_ascii_lowercase());
        if self.tail.len() > SELF_ID_TAIL_BYTES {
            let excess = self.tail.len() - SELF_ID_TAIL_BYTES;
            self.tail.drain(..excess);
        }
        if FOREIGN_SELF_IDENTIFICATION.iter().any(|phrase| {
            self.tail
                .windows(phrase.len())
                .any(|window| window == *phrase)
        }) {
            self.record(
                EvidenceCode::ForeignSelfIdentification,
                EvidenceOutcome::Failed,
            );
        }
    }

    fn observe_event(&mut self, event: &[u8]) {
        let mut data = String::new();
        for line in event.split(|byte| *byte == b'\n' || *byte == b'\r') {
            if let Some(value) = line.strip_prefix(b"data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(std::str::from_utf8(value).unwrap_or("").trim());
            }
        }
        if data.is_empty() {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            return;
        };
        self.observe_payload(&value);
    }

    fn observe_payload(&mut self, value: &Value) {
        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("response."))
            || value.get("object").and_then(Value::as_str) == Some("chat.completion")
            || value.get("choices").is_some()
        {
            self.record(EvidenceCode::ForeignProtocol, EvidenceOutcome::Failed);
            return;
        }

        match value.get("type").and_then(Value::as_str) {
            Some("message") => {
                self.record(EvidenceCode::BasicEnvelope, EvidenceOutcome::Passed);
                self.observe_model(value.get("model"));
                self.observe_usage(value.get("usage"));
                self.observe_content(value.get("content"));
                self.terminal = value.get("stop_reason").is_some();
            }
            Some("message_start") => {
                self.record(EvidenceCode::BasicEnvelope, EvidenceOutcome::Passed);
                let message = value.get("message").unwrap_or(value);
                self.observe_model(message.get("model"));
                self.observe_usage(message.get("usage"));
            }
            Some("content_block_start") => {
                let block = value.get("content_block");
                self.observe_content_block(block);
            }
            Some("content_block_delta") => {
                let delta = value.get("delta");
                match delta
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                {
                    Some("input_json_delta") => {
                        self.record(EvidenceCode::ToolCallShape, EvidenceOutcome::Passed)
                    }
                    Some("signature_delta") => {
                        self.saw_signature = true;
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                self.observe_usage(value.get("usage"));
            }
            Some("message_stop") => {
                self.terminal = true;
                self.record(EvidenceCode::StreamLifecycle, EvidenceOutcome::Passed);
            }
            _ => {}
        }
    }

    fn observe_content(&mut self, content: Option<&Value>) {
        if let Some(items) = content.and_then(Value::as_array) {
            for item in items {
                self.observe_content_block(Some(item));
            }
        }
    }

    fn observe_content_block(&mut self, block: Option<&Value>) {
        match block
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
        {
            Some("tool_use") => self.record(EvidenceCode::ToolCallShape, EvidenceOutcome::Passed),
            Some("thinking") => {
                self.saw_thinking = true;
                if block.and_then(|item| item.get("signature")).is_some() {
                    self.saw_signature = true;
                }
            }
            _ => {}
        }
    }

    fn observe_model(&mut self, model: Option<&Value>) {
        let Some(actual) = model.and_then(Value::as_str) else {
            return;
        };
        let expected = self
            .target
            .model
            .trim_end_matches("[1M]")
            .trim_end_matches("[1m]");
        let actual = actual.trim_end_matches("[1M]").trim_end_matches("[1m]");
        self.record(
            EvidenceCode::ModelMatch,
            if actual == expected {
                EvidenceOutcome::Passed
            } else {
                EvidenceOutcome::Failed
            },
        );
    }

    fn observe_usage(&mut self, usage: Option<&Value>) {
        if usage.is_some_and(Value::is_object) {
            self.record(EvidenceCode::UsageConsistency, EvidenceOutcome::Passed);
        }
    }

    fn record(&mut self, code: EvidenceCode, outcome: EvidenceOutcome) {
        let slot = &mut self.outcomes[code.index()];
        *slot = match (*slot, outcome) {
            (Some(EvidenceOutcome::Failed), _) | (_, EvidenceOutcome::Failed) => {
                Some(EvidenceOutcome::Failed)
            }
            (Some(EvidenceOutcome::Passed), _) | (_, EvidenceOutcome::Passed) => {
                Some(EvidenceOutcome::Passed)
            }
            _ => Some(EvidenceOutcome::Skipped),
        };
    }

    pub(crate) fn finish(mut self, completed: bool, observed_at: i64) -> EvidenceBatch {
        if !self.pending.is_empty() && !self.oversized {
            let event = std::mem::take(&mut self.pending);
            self.observe_event(&event);
        }
        if self.oversized {
            self.record(EvidenceCode::BasicEnvelope, EvidenceOutcome::Skipped);
        }
        if !self.terminal {
            self.record(EvidenceCode::StreamLifecycle, EvidenceOutcome::Failed);
        }
        if self.saw_thinking {
            self.record(
                EvidenceCode::ThinkingSignature,
                if self.saw_signature {
                    EvidenceOutcome::Passed
                } else {
                    EvidenceOutcome::Failed
                },
            );
        }
        let facts = EvidenceCode::ALL.into_iter().filter_map(|code| {
            self.outcomes[code.index()].map(|outcome| EvidenceFact { code, outcome })
        });
        EvidenceBatch {
            target: self.target,
            completed: completed && self.terminal,
            facts: facts.collect(),
            observed_at,
        }
    }

    pub(crate) fn reduce_non_streaming(
        target: TargetKey,
        body: &[u8],
        observed_at: i64,
    ) -> EvidenceBatch {
        let mut tap = Self::new(target);
        if body.len() > MAX_RESPONSE_INSPECTION_BYTES {
            tap.oversized = true;
            return tap.finish(false, observed_at);
        }
        if let Ok(value) = serde_json::from_slice::<Value>(body) {
            tap.observe_payload(&value);
            tap.terminal = true;
        }
        tap.finish(true, observed_at)
    }

    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        self.pending.len() + self.tail.len() + std::mem::size_of_val(&self.outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::AnthropicPassiveTap;
    use crate::relay::model_verification::{
        passive::{MAX_RESPONSE_INSPECTION_BYTES, MAX_SSE_EVENT_BYTES, SELF_ID_TAIL_BYTES},
        types::{EvidenceCode, EvidenceOutcome, TargetKey},
    };

    const STREAM: &str = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-5\",\"usage\":{}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"thinking\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SIG\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\"}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"usage\":{}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );

    fn run(split: usize) -> serde_json::Value {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "claude-sonnet-5"));
        for chunk in STREAM.as_bytes().chunks(split) {
            tap.observe_chunk(chunk);
        }
        serde_json::to_value(tap.finish(true, 9)).unwrap()
    }

    #[test]
    fn every_chunk_boundary_produces_the_same_facts() {
        let expected = run(usize::MAX);
        for split in 1..=STREAM.len() {
            assert_eq!(run(split), expected, "split {split}");
        }
    }

    #[test]
    fn foreign_protocol_and_self_identification_are_sanitized_facts() {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "claude-sonnet-5"));
        tap.observe_chunk(b"data: {\"type\":\"response.created\"}\n\n");
        tap.observe_chunk(b"data: {\"type\":\"message_start\",\"message\":{\"model\":\"x\"}}\n\n");
        tap.observe_chunk(b"data: {\"type\":\"message_stop\",\"text\":\"I am Claude\"}\n\n");
        let batch = tap.finish(true, 0);
        let facts = batch.facts.iter().collect::<Vec<_>>();
        assert!(facts.iter().any(
            |f| f.code == EvidenceCode::ForeignProtocol && f.outcome == EvidenceOutcome::Failed
        ));
        assert!(facts
            .iter()
            .any(|f| f.code == EvidenceCode::ForeignSelfIdentification
                && f.outcome == EvidenceOutcome::Failed));
        let serialized = serde_json::to_string(&batch).unwrap();
        assert!(!serialized.contains("I am Claude"));
    }

    #[test]
    fn chat_completions_shape_is_foreign_protocol() {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "m"));
        tap.observe_chunk(b"data: {\"object\":\"chat.completion\",\"choices\":[]}\n\n");
        let batch = tap.finish(true, 0);
        assert!(batch.facts.iter().any(|fact| {
            fact.code == EvidenceCode::ForeignProtocol && fact.outcome == EvidenceOutcome::Failed
        }));
    }

    #[test]
    fn thinking_without_signature_delta_fails_at_finish() {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "m"));
        tap.observe_chunk(
            b"data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"thinking\"}}\n\n",
        );
        let batch = tap.finish(true, 0);
        assert!(batch.facts.iter().any(|fact| {
            fact.code == EvidenceCode::ThinkingSignature && fact.outcome == EvidenceOutcome::Failed
        }));
    }

    #[test]
    fn thinking_with_signature_delta_passes() {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "m"));
        tap.observe_chunk(
            b"data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"thinking\"}}\n\n",
        );
        tap.observe_chunk(
            b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"SIG\"}}\n\n",
        );
        let batch = tap.finish(true, 0);
        assert!(batch.facts.iter().any(|fact| {
            fact.code == EvidenceCode::ThinkingSignature && fact.outcome == EvidenceOutcome::Passed
        }));
        assert!(!batch.facts.iter().any(|fact| {
            fact.code == EvidenceCode::ThinkingSignature && fact.outcome == EvidenceOutcome::Failed
        }));
    }

    #[test]
    fn oversized_event_stops_inspection_and_stays_bounded() {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "m"));
        tap.observe_chunk(&vec![b'x'; MAX_SSE_EVENT_BYTES + 1]);
        assert!(tap.retained_bytes() <= MAX_SSE_EVENT_BYTES + SELF_ID_TAIL_BYTES + 64);
        let batch = tap.finish(true, 0);
        assert!(!batch.completed);
        assert!(batch
            .facts
            .iter()
            .any(|f| f.outcome == EvidenceOutcome::Skipped));
    }

    #[test]
    fn long_content_does_not_grow_retained_memory() {
        let mut tap = AnthropicPassiveTap::new(TargetKey::new("p", "claude", "m"));
        for _ in 0..128 {
            tap.observe_chunk(b"data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"SENTINEL\"}}\n\n");
        }
        assert!(tap.retained_bytes() <= MAX_SSE_EVENT_BYTES + SELF_ID_TAIL_BYTES + 64);
    }

    #[test]
    fn large_non_streaming_response_is_inconclusive() {
        let body = vec![b'x'; MAX_RESPONSE_INSPECTION_BYTES + 1];
        let batch =
            AnthropicPassiveTap::reduce_non_streaming(TargetKey::new("p", "claude", "m"), &body, 0);
        assert!(!batch.completed);
        assert!(batch
            .facts
            .iter()
            .any(|f| f.outcome == EvidenceOutcome::Skipped));
    }

    #[test]
    fn non_streaming_message_uses_the_same_reducer() {
        let body = br#"{"type":"message","model":"claude-sonnet-5","stop_reason":"end_turn","usage":{},"content":[{"type":"tool_use"}]}"#;
        let batch = AnthropicPassiveTap::reduce_non_streaming(
            TargetKey::new("p", "claude", "claude-sonnet-5"),
            body,
            0,
        );
        assert!(batch.completed);
        assert!(batch.facts.iter().any(|fact| {
            fact.code == EvidenceCode::BasicEnvelope && fact.outcome == EvidenceOutcome::Passed
        }));
        assert!(batch.facts.iter().any(|fact| {
            fact.code == EvidenceCode::ToolCallShape && fact.outcome == EvidenceOutcome::Passed
        }));
    }
}
