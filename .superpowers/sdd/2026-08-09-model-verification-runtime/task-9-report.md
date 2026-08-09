# Task 9 Report — Global Runtime Verification UI

## Implementation

- Extended the existing model verification API with global setting/status snapshots and anomaly events.
- Added a local, unsaved checkbox draft to the existing dialog; the checkbox is persisted only when starting verification, and the persisted value is loaded on every open.
- Rendered actual Codex/Claude states independently from the checkbox and added the required global scope/privacy copy in all locales.
- Added one app-level anomaly listener with a sanitized localized warning toast; no frontend notification store was introduced.
- Preserved existing tier-row fingerprint spinner/run ownership behavior.

## Verification

- `pnpm typecheck`
- `pnpm test:unit -- src/components/relay/model-verification/__tests__/ModelVerificationDialog.test.tsx src/components/relay/__tests__/RelaySection.modelVerification.test.tsx` — passed
- Full Vitest run — 117 files / 732 tests passed
- `git diff --check`

## Review

The UI treats the backend snapshot as the single source of truth for persisted intent and actual app state. Closing the dialog does not write the draft, and anomaly notifications contain only app/model identifiers.
