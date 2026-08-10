# Remove Passive Model Verification Design

## Goal

Keep user-triggered model verification and remove runtime/passive verification, including the automatic local-proxy takeover that existed only to observe real Codex and Claude traffic.

## Product behavior

- Users can still open the verification dialog and explicitly start or cancel a verification run.
- Active probes continue to call the relay directly and continue to persist active results and active history.
- LoongPort no longer offers an automatic-verification switch or runtime verification status.
- Normal Codex and Claude usage is not inspected for model-verification evidence and does not trigger proxy takeover.
- Runtime anomaly notifications are removed.

## Backend shape

`ModelVerificationCoordinator` becomes an active-run coordinator only. It no longer owns a passive ingress, worker, runtime setting, proxy lease reconciliation, or a `ProxyService` dependency.

The proxy server no longer receives a model-verification ingress. Request handling and response streaming no longer construct passive metadata, protocol observers, verification taps, or evidence batches. The independent proxy subsystem, global outbound proxy settings, failover, OAuth, and manual takeover APIs remain outside this change.

The result store treats the active report as the only current verification result. Runtime-only result/history writes and passive aggregation are removed. Existing active history remains readable.

## Upgrade safety

Older installations may contain rows in `model_verification_proxy_leases`. A row is ownership evidence that runtime model verification enabled takeover; the old coordinator did not create a lease when takeover was already enabled independently.

During startup, after generic crash recovery and before normal proxy-state restoration:

1. Read legacy model-verification leases when the legacy table exists.
2. For each leased app, disable takeover through `ProxyService::set_takeover_for_app(app, false)` so restoration uses the existing backup → provider SSOT → placeholder cleanup fallback.
3. Delete the lease only after restoration succeeds.
4. Leave a failed lease in place so the next startup retries.
5. Disable the legacy runtime setting and prevent normal startup restoration from re-enabling takeover owned by model verification.

The compatibility reader tolerates databases where the legacy runtime tables do not exist. Database migrations may clean runtime settings/history, but must not discard an unresolved lease before external live-config restoration succeeds.

## Data compatibility

- Keep active result rows and active history.
- Runtime history is no longer returned or created; legacy runtime history may be deleted by migration.
- Passive aggregate data is ignored by the active-only reader and removed from the final schema where a safe table rebuild is practical.
- Legacy lease storage remains readable solely for retryable decommission cleanup; no new runtime lease is ever created.

## Non-goals

- Do not remove the global outbound network proxy.
- Do not remove the independent/manual local proxy, failover, OAuth, Claude Desktop routing, or usage logging in this change.
- Do not change active probe protocols or model-verification verdict rules.

## Acceptance criteria

- Active Codex and Claude verification still starts, reports progress, persists a result, records active history, and can be cancelled.
- There are no runtime-verification commands, UI controls, events, passive workers, protocol observers, or verification taps in the normal request path.
- `AppState` no longer injects model verification into `ProxyService` or vice versa.
- A legacy leased takeover is restored and cleared on startup; a failed restoration keeps its lease for retry.
- A manually enabled proxy takeover without a model-verification lease is not disabled by compatibility cleanup.
- Rust tests, clippy, fmt, TypeScript typecheck, Prettier check, and Vitest pass.
