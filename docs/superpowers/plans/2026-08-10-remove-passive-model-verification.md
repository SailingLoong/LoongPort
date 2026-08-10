# Remove Passive Model Verification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve explicit active model verification while removing passive runtime verification and its automatic local-proxy takeover, with retryable cleanup for existing leased takeovers.

**Architecture:** Reduce model verification to an active-probe coordinator and active-only store. Remove passive observers from proxy request/response handling and remove runtime controls from the frontend. Add a narrow legacy startup cleanup that releases only takeover leases previously owned by model verification before ordinary proxy startup restoration runs.

**Tech Stack:** Rust, Tauri, SQLite/rusqlite, TypeScript, React, Vitest.

## Global Constraints

- Active verification behavior and active verification history must remain available.
- Only model-verification-owned automatic takeover is decommissioned; unrelated proxy features remain.
- Legacy lease deletion happens only after live-config restoration succeeds.
- Do not create new runtime settings, leases, passive results, or runtime history.
- Follow test-driven development and run the repository's full verification commands before completion.

---

### Task 1: Protect legacy takeover cleanup behavior

**Files:**
- Modify: `src-tauri/src/relay/model_verification/store.rs`
- Modify: `src-tauri/src/services/proxy.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/relay/model_verification/store.rs`
- Test: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: a legacy cleanup function that lists model-verification-owned leases, calls `ProxyService::set_takeover_for_app(app, false)`, and deletes each lease only on success.
- Produces: startup ordering of crash recovery → legacy lease cleanup → normal proxy restoration.

- [ ] **Step 1: Write failing tests for absent legacy tables, successful cleanup, failed cleanup retry, and unleased manual takeover preservation.**

- [ ] **Step 2: Run the focused Rust tests and verify failures are caused by the missing cleanup behavior.**

Run:
```bash
cd src-tauri
cargo test legacy_runtime_verification
```

- [ ] **Step 3: Implement the minimal legacy lease reader/deleter and startup cleanup without adding new lease writes.**

- [ ] **Step 4: Run the focused tests and verify they pass.**

Run:
```bash
cd src-tauri
cargo test legacy_runtime_verification
```

### Task 2: Make the backend active-only

**Files:**
- Delete: `src-tauri/src/relay/model_verification/passive.rs`
- Delete: `src-tauri/src/relay/model_verification/protocols/anthropic_passive.rs`
- Delete: `src-tauri/src/relay/model_verification/protocols/openai_responses_passive.rs`
- Modify: `src-tauri/src/relay/model_verification/mod.rs`
- Modify: `src-tauri/src/relay/model_verification/protocols/mod.rs`
- Modify: `src-tauri/src/relay/model_verification/coordinator.rs`
- Modify: `src-tauri/src/relay/model_verification/store.rs`
- Modify: `src-tauri/src/relay/model_verification/types.rs`
- Modify: `src-tauri/src/relay/model_verification/history.rs`
- Modify: `src-tauri/src/commands/model_verification.rs`
- Modify: `src-tauri/src/store.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: existing model-verification Rust test modules

**Interfaces:**
- Preserves: active `start`, `cancel`, result listing, and active history listing APIs.
- Removes: runtime setting/status APIs, passive worker/ingress APIs, takeover reconciliation, passive aggregation, and runtime history writes.

- [ ] **Step 1: Change tests to assert active reports are the sole current result and history source, and remove runtime-specific expectations.**

- [ ] **Step 2: Run focused tests and confirm the old merged/passive behavior fails the new expectations.**

Run:
```bash
cd src-tauri
cargo test relay::model_verification
cargo test commands::model_verification
```

- [ ] **Step 3: Remove runtime coordinator APIs/types and passive storage paths; simplify active upsert/read logic.**

- [ ] **Step 4: Remove runtime Tauri commands and application-state/startup wiring.**

- [ ] **Step 5: Run focused Rust tests until the active-only backend passes.**

### Task 3: Remove passive observation from proxy traffic

**Files:**
- Modify: `src-tauri/src/services/proxy.rs`
- Modify: `src-tauri/src/proxy/server.rs`
- Modify: `src-tauri/src/proxy/handler_context.rs`
- Modify: `src-tauri/src/proxy/response_processor.rs`
- Modify: affected proxy tests and fixtures

**Interfaces:**
- Preserves: proxy routing and response streaming behavior.
- Removes: verification ingress injection, passive request metadata, `VerificationTap`, evidence parsing, and evidence submission.

- [ ] **Step 1: Update proxy tests so construction and response processing require no verification ingress and produce no verification side effects.**

- [ ] **Step 2: Run focused proxy tests and verify they fail against the old ingress-dependent interfaces.**

Run:
```bash
cd src-tauri
cargo test proxy::response_processor
cargo test proxy::server
```

- [ ] **Step 3: Remove passive verification parameters and branches from request/response processing while preserving ordinary streaming.**

- [ ] **Step 4: Run focused proxy tests and model-verification privacy tests.**

### Task 4: Remove runtime verification from the frontend

**Files:**
- Modify: `src/lib/api/modelVerification.ts`
- Modify: `src/components/relay/model-verification/ModelVerificationDialog.tsx`
- Modify: `src/components/relay/RelaySection.tsx`
- Modify: `src/App.tsx`
- Modify: `src/i18n/locales/en.json`
- Modify: `src/i18n/locales/ja.json`
- Modify: `src/i18n/locales/zh.json`
- Modify: `src/i18n/locales/zh-TW.json`
- Modify: `src/components/relay/model-verification/__tests__/ModelVerificationDialog.test.tsx`
- Modify: `src/components/relay/__tests__/RelaySection.modelVerification.test.tsx`
- Modify: other affected model-verification tests

**Interfaces:**
- Preserves: active dialog, run progress, cancellation, current result, and active history rendering.
- Removes: runtime setting API/types, automatic verification controls/status, runtime history labels, and anomaly toast listener.

- [ ] **Step 1: Update component tests to expect an active-only dialog and no runtime controls or startup runtime-setting requests.**

- [ ] **Step 2: Run focused Vitest tests and verify the old runtime UI fails the new expectations.**

Run:
```bash
pnpm vitest run src/components/relay/model-verification src/components/relay/__tests__/RelaySection.modelVerification.test.tsx
```

- [ ] **Step 3: Remove runtime API calls, state, controls, event listener, and locale keys.**

- [ ] **Step 4: Run focused Vitest tests and TypeScript typecheck.**

### Task 5: Migrate legacy database state and verify the full change

**Files:**
- Modify: `src-tauri/src/database/loongport_schema.rs`
- Modify: `LOONGPORT.md` if product documentation mentions passive/automatic verification
- Test: `src-tauri/src/database/loongport_schema.rs`

**Interfaces:**
- Produces: the next LoongPort schema migration that disables legacy runtime settings and removes runtime history without deleting unresolved proxy leases.
- Preserves: active result/history data and upstream `PRAGMA user_version`.

- [ ] **Step 1: Write a failing migration test seeded with active history, runtime history, an enabled runtime setting, and an unresolved lease.**

- [ ] **Step 2: Run the migration test and verify the current schema leaves runtime state intact.**

Run:
```bash
cd src-tauri
cargo test loongport_schema
```

- [ ] **Step 3: Add the minimal schema migration, keeping unresolved leases available to startup cleanup.**

- [ ] **Step 4: Run all required verification commands.**

Run:
```bash
cd src-tauri
export PATH="$HOME/.cargo/bin:$PATH"
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cd ..
npx tsc --noEmit
pnpm format:check
npx vitest run
```

- [ ] **Step 5: Review the final diff for accidental deletion of global/manual proxy functionality and for remaining passive/runtime references.**
