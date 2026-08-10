# Browser-Assisted Relay Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow users to import third-party relay sites even when native probing is blocked by browser verification, without bypassing that verification or misclassifying future site protocols.

**Architecture:** Keep native probing as a fast path. On any native transport/protocol failure, open one protocol-neutral incognito WebView at the user-provided site, let the user complete any verification, then poll registered candidate endpoints from the page's same-origin browser context. Rust owns strict protocol detection; after a detector succeeds, the same WebView session continues into that adapter's registration/login flow and returns credentials.

**Tech Stack:** Rust, Tauri 2 WebView, reqwest, serde_json, React, TypeScript, Vitest.

## Global Constraints

- Never bypass, solve, copy, or export browser verification state.
- Do not branch on Cloudflare, HTTP 403, challenge wording, or cookies.
- The initial browser flow must not assume sub2api or new-api.
- A non-empty `version` alone must never identify sub2api.
- Detection, registration/login, and credential collection after fallback must stay in the same incognito WebView session.
- Preserve standalone re-login for existing relay rows.

---

### Task 1: Strict sub2api detector

**Files:**
- Modify: `src-tauri/src/relay/api.rs`

**Interfaces:**
- Produces: `parse_public_settings(body: &str) -> Result<PublicSettings, AppError>` as the single parser used by native and browser responses.

- [ ] Add tests proving a valid sub2api envelope parses, while HTML, new-api JSON, nonzero/malformed envelopes, and version-only lookalikes fail.
- [ ] Run the focused Rust tests and confirm the new tests fail because the shared strict parser does not exist yet.
- [ ] Implement the parser with stable typed sub2api fingerprint fields and make `probe_site` call it.
- [ ] Re-run the focused tests and confirm they pass.

### Task 2: Protocol-neutral browser discovery

**Files:**
- Create: `src-tauri/src/relay/discovery.rs`
- Modify: `src-tauri/src/relay/mod.rs`
- Modify: `src-tauri/src/relay/login.rs`

**Interfaces:**
- Produces: a registry of candidate detector IDs and paths, a custom-scheme response parser, and a same-origin polling initialization script.
- Consumes: `api::parse_public_settings` for the sub2api detector.

- [ ] Add tests for multiple configurable candidate paths, same-origin credentialed fetch, generic response callbacks, and absence of Cloudflare/403/cookie special cases.
- [ ] Run focused tests and confirm failure because discovery APIs do not exist.
- [ ] Implement the detector registry, callback decoding, and browser polling script.
- [ ] Re-run focused tests and confirm they pass.

### Task 3: Combined import command and same-session continuation

**Files:**
- Modify: `src-tauri/src/commands/relay.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `relay_import_site(site) -> ImportResult { relay_id, site_origin, site_name, logged_in }`.
- Consumes: native strict probe, browser discovery registry, existing sub2api login/credential helpers, and existing persistence APIs.

- [ ] Add unit-testable helper tests for preserving a valid user entry URL and for invalid URL rejection before browser fallback.
- [ ] Run focused tests and confirm failure on the missing helper/command behavior.
- [ ] Implement native fast path; on failure create one generic WebView, detect the protocol after user verification, set adapter-specific login behavior, navigate and collect credentials in that same window.
- [ ] Keep `relay_login` for existing rows and register the new command.
- [ ] Re-run Rust tests.

### Task 4: Frontend import flow

**Files:**
- Modify: `src/lib/api/relay.ts`
- Modify: `src/components/relay/AddSiteDialog.tsx`
- Modify: `tests/components/AddSiteDialogProvisionsAfterLogin.test.tsx`
- Modify: `tests/lib/relayApiTargeting.test.ts`

**Interfaces:**
- Consumes: `relay_import_site` and its `loggedIn` result.

- [ ] Change tests first so add-site uses one combined import call, provisions once only when `loggedIn` is true, waits for provision before refresh, and does not call standalone login.
- [ ] Run the focused Vitest files and confirm failure against the old API.
- [ ] Implement the TypeScript API and dialog changes.
- [ ] Re-run focused frontend tests.

### Task 5: Verification

**Files:**
- Review all modified files.

- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo test`.
- [ ] Run `cargo clippy --all-targets -- -D warnings`.
- [ ] Run `npx tsc --noEmit`.
- [ ] Run `pnpm format:check`.
- [ ] Run `npx vitest run`.
- [ ] Review `git diff` for protocol assumptions, duplicate sources of truth, and unrelated changes.
