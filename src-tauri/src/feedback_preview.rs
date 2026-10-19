//! A single ephemeral, one-shot feedback preview. No disk store or replay queue.
use crate::feedback_receipt::FeedbackSafetySummary;
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(crate) const PREVIEW_TTL: Duration = Duration::from_secs(10 * 60);

pub(crate) struct PreparedFeedback {
    pub endpoint: String,
    pub form: reqwest::multipart::Form,
    pub report_id: String,
    pub requires_extended_fields: bool,
    pub summary: FeedbackSafetySummary,
    pub bundle: Option<Vec<u8>>,
    pub preview: Value,
}
struct Entry {
    token: String,
    deadline: Instant,
    payload: PreparedFeedback,
}
#[derive(Default)]
struct State {
    generation: u64,
    entry: Option<Entry>,
}
#[derive(Default)]
pub(crate) struct FeedbackPreviewSlot(Mutex<State>);
impl FeedbackPreviewSlot {
    /// Starting a new draft invalidates the previous preview even if collection
    /// later fails. A late collector cannot publish across this generation.
    pub fn begin(&self) -> Result<u64, &'static str> {
        let mut state = self.0.lock().map_err(|_| "feedback_preview_unavailable")?;
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or("feedback_preview_unavailable")?;
        state.entry = None;
        Ok(state.generation)
    }
    pub fn publish(
        &self,
        generation: u64,
        payload: PreparedFeedback,
        now: Instant,
    ) -> Result<Value, &'static str> {
        let mut state = self.0.lock().map_err(|_| "feedback_preview_unavailable")?;
        if state.generation != generation || state.entry.is_some() {
            return Err("feedback_preview_changed");
        }
        let token = uuid::Uuid::new_v4().to_string();
        let mut preview = payload.preview.clone();
        preview["previewId"] = Value::String(token.clone());
        preview["expiresInSeconds"] = Value::from(PREVIEW_TTL.as_secs());
        state.entry = Some(Entry {
            token,
            deadline: now + PREVIEW_TTL,
            payload,
        });
        Ok(preview)
    }
    /// The caller already owns the original upload admission guard. Taking a
    /// token consumes it before any HTTP. Unknown outcomes never restore it.
    pub fn take(&self, token: &str, now: Instant) -> Result<PreparedFeedback, &'static str> {
        let mut state = self.0.lock().map_err(|_| "feedback_preview_unavailable")?;
        if state.entry.as_ref().is_some_and(|e| now >= e.deadline) {
            state.entry = None;
        }
        if state.entry.as_ref().is_none_or(|e| e.token != token) {
            return Err("feedback_preview_changed");
        }
        Ok(state
            .entry
            .take()
            .ok_or("feedback_preview_changed")?
            .payload)
    }
    pub fn cancel(&self, token: &str) -> Result<(), &'static str> {
        let mut state = self.0.lock().map_err(|_| "feedback_preview_unavailable")?;
        if state.entry.as_ref().is_some_and(|e| e.token == token) {
            state.entry = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn payload(bytes: &[u8]) -> PreparedFeedback {
        PreparedFeedback {
            endpoint: "https://example.invalid/feedback".into(),
            form: reqwest::multipart::Form::new(),
            report_id: "synthetic-report".into(),
            requires_extended_fields: false,
            summary: FeedbackSafetySummary {
                description_included: true,
                contact_included: false,
                screenshot_count: 0,
                screenshot_bytes: 0,
                diagnostics_included: true,
                diagnostics_bytes: bytes.len() as u64,
            },
            bundle: Some(bytes.to_vec()),
            preview: serde_json::json!({"files":[{"name":"diagnostics.zip","bytes":bytes.len()}]}),
        }
    }
    fn prepare(slot: &FeedbackPreviewSlot, bytes: &[u8], now: Instant) -> String {
        let g = slot.begin().unwrap();
        slot.publish(g, payload(bytes), now).unwrap()["previewId"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    #[test]
    fn same_snapshot_bytes_are_consumed_exactly_once() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let id = prepare(&slot, b"frozen zip", now);
        assert_eq!(slot.take(&id, now).unwrap().bundle.unwrap(), b"frozen zip");
        assert!(slot.take(&id, now).is_err());
    }
    #[test]
    fn old_collector_cannot_replace_new_window_snapshot() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let old = slot.begin().unwrap();
        let id = prepare(&slot, b"new", now);
        assert_eq!(
            slot.publish(old, payload(b"old"), now).unwrap_err(),
            "feedback_preview_changed"
        );
        assert_eq!(slot.take(&id, now).unwrap().bundle.unwrap(), b"new");
    }
    #[test]
    fn cancel_old_token_does_not_clear_new_preview() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let old = prepare(&slot, b"old", now);
        let new = prepare(&slot, b"new", now);
        slot.cancel(&old).unwrap();
        assert_eq!(slot.take(&new, now).unwrap().bundle.unwrap(), b"new");
    }
    #[test]
    fn monotonic_deadline_rejects_expired_preview() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let id = prepare(&slot, b"zip", now);
        assert!(slot.take(&id, now + PREVIEW_TTL).is_err());
        assert!(slot.take(&id, now).is_err());
    }
    #[test]
    fn failed_new_collection_leaves_old_token_invalid() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let old = prepare(&slot, b"old", now);
        slot.begin().unwrap();
        assert!(slot.take(&old, now).is_err());
    }
    #[test]
    fn wrong_token_cannot_consume_current_preview() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let id = prepare(&slot, b"zip", now);
        assert!(slot.take("forged", now).is_err());
        assert_eq!(slot.take(&id, now).unwrap().bundle.unwrap(), b"zip");
    }
    #[test]
    fn concurrent_consumers_only_one_wins() {
        let slot = std::sync::Arc::new(FeedbackPreviewSlot::default());
        let now = Instant::now();
        let id = prepare(&slot, b"zip", now);
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let slot = slot.clone();
                let id = id.clone();
                std::thread::spawn(move || slot.take(&id, now).is_ok())
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter(|ok| *ok)
                .count(),
            1
        );
    }
    #[test]
    fn poisoned_preview_owner_fails_closed() {
        let slot = std::sync::Arc::new(FeedbackPreviewSlot::default());
        let other = slot.clone();
        let _ = std::thread::spawn(move || {
            let _guard = other.0.lock().unwrap();
            panic!("synthetic poison");
        })
        .join();
        assert!(slot.begin().is_err());
        assert!(slot.take("id", Instant::now()).is_err());
        assert!(slot.cancel("id").is_err());
    }

    #[test]
    fn duplicate_publication_cannot_replace_shown_token() {
        let slot = FeedbackPreviewSlot::default();
        let now = Instant::now();
        let generation = slot.begin().unwrap();
        let first = slot.publish(generation, payload(b"first"), now).unwrap();
        assert!(slot.publish(generation, payload(b"second"), now).is_err());
        assert_eq!(
            slot.take(first["previewId"].as_str().unwrap(), now)
                .unwrap()
                .bundle
                .unwrap(),
            b"first"
        );
    }
}
