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

/// A bounded reducer for OpenAI Responses SSE. It deliberately ignores additive events and
/// retains only finite evidence outcomes.
pub(crate) struct OpenAiResponsesPassiveTap {
    target: TargetKey,
    generation: u64,
    pending: Vec<u8>,
    outcomes: [Option<EvidenceOutcome>; EvidenceCode::CARDINALITY],
    tail: Vec<u8>,
    terminal: bool,
    oversized: bool,
}

impl OpenAiResponsesPassiveTap {
    pub(crate) fn new(target: TargetKey, generation: u64) -> Self {
        Self {
            target,
            generation,
            pending: Vec::new(),
            outcomes: [None; EvidenceCode::CARDINALITY],
            tail: Vec::with_capacity(SELF_ID_TAIL_BYTES),
            terminal: false,
            oversized: false,
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
        if data.is_empty() || data == "[DONE]" {
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
            .is_some_and(|kind| kind.starts_with("message_"))
            || value.get("object").and_then(Value::as_str) == Some("chat.completion")
            || value.get("choices").is_some()
        {
            self.record(EvidenceCode::ForeignProtocol, EvidenceOutcome::Failed);
            return;
        }

        // Non-stream Responses use the same envelope without an event `type`.
        if value.get("object").and_then(Value::as_str) == Some("response") {
            self.record(EvidenceCode::BasicEnvelope, EvidenceOutcome::Passed);
            self.observe_model(value.get("model"));
            self.observe_usage(value.get("usage"));
            self.observe_output(value.get("output"));
            self.terminal = value.get("status").and_then(Value::as_str) == Some("completed");
            return;
        }

        match value.get("type").and_then(Value::as_str) {
            Some("response") => {
                self.record(EvidenceCode::BasicEnvelope, EvidenceOutcome::Passed);
                self.observe_model(value.get("model"));
                self.observe_usage(value.get("usage"));
                self.observe_output(value.get("output"));
                self.terminal = value.get("status").and_then(Value::as_str) == Some("completed");
            }
            Some("response.created") => {
                self.record(EvidenceCode::BasicEnvelope, EvidenceOutcome::Passed);
                let response = value.get("response").unwrap_or(value);
                self.observe_model(response.get("model"));
                self.observe_usage(response.get("usage"));
            }
            Some("response.output_item.added" | "response.output_item.done") => {
                self.observe_output_item(value.get("item"));
            }
            Some(
                "response.function_call_arguments.delta" | "response.function_call_arguments.done",
            ) => {
                self.record(EvidenceCode::ToolCallShape, EvidenceOutcome::Passed);
            }
            Some("response.completed") => {
                let response = value.get("response").unwrap_or(value);
                self.observe_model(response.get("model"));
                self.observe_usage(response.get("usage"));
                self.observe_output(response.get("output"));
                self.terminal = true;
                self.record(EvidenceCode::StreamLifecycle, EvidenceOutcome::Passed);
            }
            Some("response.usage") => {
                self.observe_usage(value.get("usage").or_else(|| value.get("response")));
            }
            _ => {}
        }
    }

    fn observe_output(&mut self, output: Option<&Value>) {
        if let Some(items) = output.and_then(Value::as_array) {
            for item in items {
                self.observe_output_item(Some(item));
            }
        }
    }

    fn observe_output_item(&mut self, item: Option<&Value>) {
        let Some(item) = item else { return };
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                self.record(EvidenceCode::ToolCallShape, EvidenceOutcome::Passed)
            }
            Some("message") => {
                if item
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) == Some("output_text")
                        })
                    })
                {
                    self.record(EvidenceCode::StructuredOutput, EvidenceOutcome::Passed);
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
        let facts = EvidenceCode::ALL.into_iter().filter_map(|code| {
            self.outcomes[code.index()].map(|outcome| EvidenceFact { code, outcome })
        });
        EvidenceBatch::new(
            self.target,
            self.generation,
            completed && self.terminal,
            facts,
            observed_at,
        )
    }

    pub(crate) fn reduce_non_streaming(
        target: TargetKey,
        generation: u64,
        body: &[u8],
        observed_at: i64,
    ) -> EvidenceBatch {
        let mut tap = Self::new(target, generation);
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
    use super::OpenAiResponsesPassiveTap;
    use crate::relay::model_verification::{
        passive::{MAX_RESPONSE_INSPECTION_BYTES, MAX_SSE_EVENT_BYTES, SELF_ID_TAIL_BYTES},
        types::{EvidenceCode, EvidenceOutcome, TargetKey},
    };

    const STREAM: &str = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"object\":\"response\",\"model\":\"gpt-5.6-sol\"}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"content\":[]}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\"}}\n\n",
        "event: response.function_call_arguments.delta\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"SENTINEL\"}\n\n",
        "event: response.usage\n",
        "data: {\"type\":\"response.usage\",\"usage\":{}}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"object\":\"response\",\"status\":\"completed\",\"model\":\"gpt-5.6-sol\",\"usage\":{}}}\n\n"
    );

    fn run(split: usize) -> serde_json::Value {
        let mut tap =
            OpenAiResponsesPassiveTap::new(TargetKey::new("p", "codex", "gpt-5.6-sol"), 3);
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
        let mut tap =
            OpenAiResponsesPassiveTap::new(TargetKey::new("p", "codex", "gpt-5.6-sol"), 1);
        tap.observe_chunk(b"data: {\"type\":\"message_start\"}\n\n");
        tap.observe_chunk(
            b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"x\"}}\n\n",
        );
        tap.observe_chunk(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"},\"text\":\"I am Claude\"}\n\n");
        let batch = tap.finish(true, 0);
        let facts = batch.facts.iter().collect::<Vec<_>>();
        assert!(facts.iter().any(
            |f| f.code == EvidenceCode::ForeignProtocol && f.outcome == EvidenceOutcome::Failed
        ));
        assert!(facts
            .iter()
            .any(|f| f.code == EvidenceCode::ForeignSelfIdentification
                && f.outcome == EvidenceOutcome::Failed));
        assert!(!serde_json::to_string(&batch)
            .unwrap()
            .contains("I am Claude"));
    }

    #[test]
    fn oversized_event_stops_inspection_and_stays_bounded() {
        let mut tap = OpenAiResponsesPassiveTap::new(TargetKey::new("p", "codex", "m"), 1);
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
        let mut tap = OpenAiResponsesPassiveTap::new(TargetKey::new("p", "codex", "m"), 1);
        for _ in 0..128 {
            tap.observe_chunk(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"SENTINEL\"}\n\n",
            );
        }
        assert!(tap.retained_bytes() <= MAX_SSE_EVENT_BYTES + SELF_ID_TAIL_BYTES + 64);
    }

    #[test]
    fn large_non_streaming_response_is_inconclusive() {
        let body = vec![b'x'; MAX_RESPONSE_INSPECTION_BYTES + 1];
        let batch = OpenAiResponsesPassiveTap::reduce_non_streaming(
            TargetKey::new("p", "codex", "m"),
            1,
            &body,
            0,
        );
        assert!(!batch.completed);
        assert!(batch
            .facts
            .iter()
            .any(|f| f.outcome == EvidenceOutcome::Skipped));
    }

    #[test]
    fn non_streaming_response_uses_the_same_reducer() {
        let body = br#"{"object":"response","status":"completed","model":"gpt-5.6-sol","usage":{},"output":[{"type":"function_call"}]}"#;
        let batch = OpenAiResponsesPassiveTap::reduce_non_streaming(
            TargetKey::new("p", "codex", "gpt-5.6-sol"),
            1,
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
