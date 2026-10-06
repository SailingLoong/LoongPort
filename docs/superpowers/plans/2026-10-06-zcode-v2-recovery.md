# ZCode V2 Recovery Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task by task. Each source slice is tested and independently reviewed before publication.

**Goal:** Complete the approved Z1 / U1–U6 account workflow on the published beta.3 adapter.

**Architecture:** Keep the existing encrypted catalog, vault session, request owner and native transaction. Add ZCode-specific official protocol and capability facts; do not build a second account store or switching engine. Separate vault-only account work from native activation admission.

**Tech Stack:** Rust / Tauri 2, existing reqwest and vault primitives, React / TypeScript, Radix UI / Tailwind 3, Vitest and actual-source Rust harness.

**Spec:** [ZCode account workflow V2](../../zcode-accounts-v2.md).

## Global Constraints

- Base: `4a26a0c2d0f095ae4b4ad13f4bfe8165d9bb570e`, tree `c52bbd7dfe101daece676a93a2100fe07be31b1a`.
- Z1 / U1–U6 only; no new product behavior or key UI flow without explicit version review.
- Native session owner remains `zcode_accounts`; no edits to the separate 4.x mode/startup lifecycle.
- Fixed references: official ZCode `29628c9acdb81b703bbd4080c207a0e7ce5e276e`; pjpv `f34225686dfef05d84c256a56f868719248f15ff`.
- No real credentials, official authentication, Key creation or model calls in synthetic tests.
- No merge or release. Public source must be tested, independently reviewed and free of private material.

## Existing Code to Reuse

| Existing owner | Already available at beta.3 | Required extension |
| --- | --- | --- |
| `core.rs`, `native.rs`, `checkpoint.rs` | Stable scope/family/cache identity, strict credential image, native codec, encrypted catalog | Official session construction, credential-bound provenance/capability evidence and encrypted local labels |
| `transaction.rs`, `operation_log.rs`, `recovery.rs` | Vault account IO, atomic/native transaction, original request and recovery facts | Vault-only account lifecycle and capability admission without a blanket imported-row ban |
| `bundle.rs`, `bundle_import.rs`, `bundle_limits.rs`, `import_reviews.rs` | Authenticated bounded `.zsb` decode, exact selected row, duplicate preview | Matching encoder, explicit per-capability checks, completed-candidate update rule |
| `runtime.rs`, `api.rs`, `commands/zcode_accounts.rs` | Existing database/vault session ownership and IPC boundary | Official login/check/backup orchestration under the same owner |
| `ZCodeAccountPanel`, `ZCodeBundleImport`, `ZCodeProviderPanel` | Capture, manual native switch/recovery, single API configuration | Persistent account actions; login, checks and backup dialogs; distinct login/API tabs |
| `tests/zcode-account-core` and existing component tests | Actual product core harness and synthetic IPC fixtures | Extend these graphs and affected behavior tests; do not create substitute product implementations |

## Review Focus

1. A new unverified imported candidate must not erase an existing ready record.
2. Native capture with unchanged credential plaintext must retain matching V2 evidence; unrelated credential changes must not erase all capabilities.
3. Cancellation between HTTP requests must prevent later credentials from being sent, including within a row.
4. A dropped waiter or old poll destructor must not release a successor operation; acquire ownership before spawning work and bind release to an individual operation generation.
5. Potential remote Key creation and committed local save must survive lost replies; recovery queries must remain reachable and must not replay writes.

## Slice 1: Official Protocol and Owned Login

**Files:** Add narrow `oauth.rs`, `official.rs`, `official_http.rs` and their product-module tests under `src-tauri/src/zcode_accounts/`; extend the existing module list and core harness only as required.

**Interfaces:** `OAuthFamily`, `AccountIdentity`, `CredentialDocument` and `AccountSnapshot` remain existing shared types. A single `LoginFlowStore` owns a `FlowId`, per-operation generation and cancellation state; `LoginProgress` is a secret-free backend enum for waiting, preparing, Key consent, review, saved and terminal errors. `OfficialTransport::send(OfficialRequest)` is the bounded injectable HTTP boundary; secrets stay in zeroizing backend values.

- [ ] Pin official init/poll, z.ai token normalization, project/Key and capability contracts from the fixed source. Test actual request/response shapes before implementing transport.
- [ ] RED: separate business/JWT/Key credentials; expired flow; repeated begin; duplicate account; cancellation before/after each request; old poll Drop after a successor starts. Implement generation-bound leases and bounded flows; GREEN.
- [ ] RED: existing Key reuse; exact account/project consent; potentially successful POST plus dropped IPC waiter; cancellation/no POST; lost save response. Implement durable intent before POST, read-only uncertainty recovery and independent saved receipts; GREEN.
- [ ] Compile the actual modules, review this source slice independently, then publish its exact source checkpoint to the recovery branch.

## Slice 2: Catalog Evidence, Complete Import and Backup

**Files:** Extend `core.rs`, `native.rs`, `checkpoint.rs`, `transaction.rs`, `bundle.rs`, `bundle_import.rs`, `import_reviews.rs`; add narrow `session_checks.rs` and `bundle_export.rs` with corresponding actual-source tests.

**Interfaces:** `SessionCheckReport` carries independent business-token / Start / Coding results and display-source labels; it never establishes identity from a cache slot. `ProfileCatalog` remains the sole catalog and binds each capability to the consumed credential revision. `ImportChoice { index, update_duplicate }` remains the exact-row choice. Export consumes selected catalog snapshots and produces the existing `.zsb` envelope.

- [ ] RED: valid external complete session without separate same-person proof; business rejection with valid JWT/Key; unknown vs expired; zero vs absent quota; multiple plan instances. Implement independent checks without field stitching; GREEN.
- [ ] RED: unverified duplicate overwrite; native recapture of identical plaintext with new encryption nonce; changed JWT vs changed Key. Add narrow provenance retention and replacement rules; GREEN.
- [ ] RED: import worker cancellation before first poll and midway through a row, abandoned worker lease, stale vault/catalog binding. Acquire ownership before handoff and check cancellation at each send; GREEN.
- [ ] RED: bad password/tamper/limits, export roundtrip, write failure and authenticated read-back failure. Reuse current codec/OwnedFile/durable IO; no plaintext temporary file or exported local labels; GREEN.
- [ ] Review and persist the tested source slice on the same recovery branch.

## Slice 3: Existing Runtime and UI Integration

**Files:** Extend existing `runtime.rs`, `api.rs`, `commands/zcode_accounts.rs`, `src/lib/api/zcodeAccounts.ts`, ZCode components and tests. Add only necessary handler registrations to `src-tauri/src/lib.rs` and only `zcode.*` keys to the four locale files.

**Interfaces:** Commands are thin adapters to the same runtime owner. Backend DTOs directly supply action eligibility, capability/identity source, quota units/times and original-result status. Existing `settingsApi.openExternal`, dialogs, sheets, buttons and request IDs are reused.

- [ ] RED: catalog/add/import/backup remain reachable without an admitted native client; native capture/switch show exact blocking reasons. Integrate vault-only commands while preserving native admission; GREEN.
- [ ] RED: repeated/late OAuth, Key consent cancellation, lost confirmation response, original saved receipt, read cancellation and list refresh exactly once. Implement U1–U4 using backend progress without write replay; GREEN.
- [ ] RED: local label edit, source/focus invalidation of an explicit current-identity read, switch capability matching actual Start/Coding selection, existing API configuration regression. Integrate U5–U6 without a second current-account authority; GREEN.
- [ ] Verify actual new handlers and runtime modules enter the compile graph. If a private no-GUI graph is necessary, record adaptations and demonstrate it catches a temporary compile canary, then remove it; never call this a GUI/native pass.
- [ ] Run affected frontend type/format/component checks and Rust checks, independently review, and persist this slice.

## Final Candidate

- [ ] Freeze one new commit; run repository-required affected aggregate checks and record precise pass/fail/skips and environment limits.
- [ ] Whole-branch independent review covers OAuth/Key uncertainty, imports, vault/native boundaries, UI result recovery and public-source privacy. Fix findings with regression evidence.
- [ ] Create/update the draft PR, verify remote tree equals the reviewed tree, and follow exact-SHA CI to its actual result.
- [ ] Keep full GUI/browser, actual official credentials/model inference, native macOS and independent Windows acceptance explicitly pending until genuinely exercised with authority.

Source checkpoint recording is maintained in the [existing delivery checklist](../../zcode-quick-account-v1-delivery.md#v2-恢复实施与源码检查点), not a new admission system. Historical unavailable candidates and their tests are not evidence for this replacement branch.
