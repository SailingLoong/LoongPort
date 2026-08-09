# Task 8 Report — Notification Claims, Active Resolution, and Reset Barrier

## Implementation

- Added atomic bounded notification claims alongside passive aggregate/verdict persistence.
- Only unresolved high-confidence fingerprints are claimed; repeats remain suppressed until an applicable active pass clears them.
- Added sanitized `MODEL_VERIFICATION_ANOMALY` event payload containing target identifiers and finite fingerprint only.
- Extended the coordinator worker to emit anomaly events after persistence and result-change events after the same commit.
- Reset now bumps the passive generation before deleting the scope row, dropping queued pre-reset observations without touching runtime settings or leases.

## Verification

- `cargo fmt --check`
- `cargo check --lib`
- `cargo test events::consistency_tests --lib` — passed
- `cargo test model_verification::store --lib` — 8 passed
- `git diff --check`

## Review

Notification state is owned by the result row; no second client-side fact store was introduced. Event payloads contain no response content, URLs, headers, credentials, or free-form errors.
