# Qoder single-turn task runtime

Product entry: `startQoderTask` from `task-entry.mjs`. It calls the official
`@qoder-ai/qoder-agent-sdk` query factory and returns the owned bridge's
`snapshot()`, `drainEvents()`, `diagnostics()` and `cancel()` methods.

The caller supplies a trusted binding (accountId, sessionId, turnId), prompt,
and explicit SDK options including auth. The binding's sessionId is passed to
SDK options; conflicting sessionId, daemon and resume are rejected before
startup. This entry is for one exclusive Query and one external request.
It does not select an account or establish credential/account provenance.

Authentication must be snapshotable explicit data (for example the SDK's direct
accessToken or direct serviceAccount-key configuration). It is copied in memory
before SDK loading, never logged/persisted by this module. Callback-based token
refresh objects are rejected rather than retained by reference. Auth acquisition
and any authentication refresh remain the account owner's responsibilities.
No environment/local-login fallback is selected automatically by this entry.

The pinned SDK peer is 1.0.53. It is used as an external application-owned dependency. The SDK license is governed by Qoder Product Service Terms, not MIT;
no SDK implementation is vendored here. Public API shapes are used; SDK implementation is not copied into this repository.
The lib converter and lifecycle wrapper are the existing LoongPort synthetic
candidates ported without behavior changes other than local import paths.

Validation: `pnpm --dir scripts/qoder test` runs product-path tests with an injected
SDK factory, covering startup -> Query -> converter -> returned events/EOF,
pre-start validation, generic startup failures, frozen auth/session selection,
exclusive cancellation/cleanup and Query ownership. This does not prove a live
Qoder account, SDK process, tool execution, or model response.

`worker.mjs` exposes one task over private stdin/stdout JSONL pipes: one `start`
request, optional idempotent `cancel`, then bound `event` frames and an `end`
frame after actual source EOF. Startup/protocol failures use a generic error
frame. Frames are limited to 1 MiB. `waitForSourceEnd()` distinguishes source
completion from the presentation terminal event emitted during cancellation.

The Rust `proxy::qoder_host` module starts the configured Node worker, forwards
events only when all three binding fields match, and reports premature EOF,
transport and cleanup failures. Drop sends cancellation and allows two seconds
for local process cleanup before killing it. This does not prove remote inference
has stopped. Node/script paths must come from trusted backend configuration.

Directed Rust tests spawn real Node children using synthetic SDK factories.
The existing `/v1/chat/completions` route now dispatches to this host when the
selected provider has `meta.providerType = "qoder_sdk"`. The existing Codex
RequestContext chooses the provider and its auth adapter reads the explicit
provider token. Incoming HTTP headers/body cannot override that token, account,
Node path, worker path, or SDK options. This slice adds no account enrollment UI.

Supported requests contain an ordered text-only `messages` history ending in a
user message. Every HTTP request sends that complete history as a JSON transcript
to a new exclusive Query with fresh session/turn IDs; SDK resume is not used.
This is explicit transcript replay, not verified native SDK conversation state.
Tools/media/previous_response_id are rejected; SDK tools, inherited settings and
MCP configuration are disabled by the supplied options. Other generation tuning
fields are currently ignored. No automatic task retry or provider failover is
performed after this dispatch, to avoid duplicate agent work.

Text deltas/snapshots become Chat Completions JSON or SSE. Success finish/DONE
wait for real EOF and cleanup. Late transport/cleanup errors stay failures;
client body drop cancels the task-owned host. Existing active-connection guarding
and per-app timeout policy are reused. SDK usage is not yet integrated into the
application usage logger.

GUI bundles list the seven runtime source files as `qoder/` resources, resolved
through Tauri's runtime resource directory. Only development builds can fall
back to the source directory. Only Rust test builds accept fixture HostConfig extensions. Production resolves
Node through the existing bounded CLI discovery helper, canonicalizes its absolute
path and checks Node >=22. SDK/runtime admission uses a separate no-account probe
with a cleared environment and bounded timeout. Missing Node/SDK, old versions,
escaped paths and load failures return specific HTTP503 codes/messages.

The SDK location is fixed beneath the current application data directory:
`runtimes/qoder/1.0.53/node_modules/@qoder-ai/qoder-agent-sdk`. The data directory's
existing override/migration mechanism remains authoritative. No HTTP field or
environment module lookup can choose this SDK. Its version and matching global
Worker runtime identity are checked; an explicit official WorkerTransport pins
the runtime entry, preventing an ambient runtime fallback. The SDK directory must
be moved intact (preserving relative dependency symlinks) when migrating app data.

Node and SDK are external prerequisites and are not redistributed in the app.
An owner can provision the fixed runtime directory with the normal package manager:

```sh
npm install --prefix "<application-data-dir>/runtimes/qoder/1.0.53" @qoder-ai/qoder-agent-sdk@1.0.53 zod@4.6.5
```

Keep the official postinstall runtime download and checksum checks enabled. SDK
1.0.53 selects Worker runtime1.1.71. Provisioning and these admission checks do not
need an account or start a Query. Loading the real SDK, missing/corrupt installs,
version rejection and relocation have separate no-account integration tests.
Supply `LOONGPORT_QODER_SDK_TEST_ROOT` to the Node tests and the ignored Rust
runtime integration tests; absent provisioning is explicitly skipped/ignored,
not counted as real-SDK validation. Model responses, auth validity, usage and
remote cancellation still require the owner's separate live-account acceptance.

The small directed harness compiles the exact host/HTTP implementation and
exercises an Axum Chat Completions router with real synthetic Node children.
A separate full-product router test in `server.rs` covers RequestContext and auth
selection; run it with an isolated `CC_SWITCH_TEST_HOME` and `QODER_HOST_NODE`.
Passing the small harness does not establish that full-product test passed.
