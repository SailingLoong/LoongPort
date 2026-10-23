# Qoder HTTP runtime

The HTTP/SSE worker reuses the protocol derived from `@magpie-community/opencode-qoder-auth` 0.2.18 (plugins commit `cdf45137d644602bd2452491a8ff79df1a255e59`). `LICENSE` and `LICENSE-CLIProxyAPI` retain upstream notices. CN PAT endpoint facts additionally retain the attribution in `licenses/dsh-provider-qoder-MIT.txt`.

Node.js 22 or later is required. No Qoder SDK package, SDK root, Bun runtime, plugin account store, or caller-selected executable is used. `node worker.mjs --check-runtime` is an offline protocol probe.

## Credential ownership and regions

The original LoongPort Provider and encrypted subordinate owner remain the only account store. The private request snapshot explicitly names one credential kind:

- `international_qoder_pat`: explicitly established International PAT identity.
- `china_qoder_pat`: the user's independently authorized CN PAT identity.
- `china_qoder_device`: a new CN device grant from LoongPort's independent authorization entry.

CN snapshots use only the fixed CN gateway; mismatched kind/region is rejected. No token-prefix inference or International fallback is allowed. The worker receives access token, verified UID and machine ID through its private stdin pipe, never a PAT or refresh token. Incoming HTTP headers/options cannot choose credentials, a host, or a runtime. The worker does not exchange device tokens for job tokens or refresh credentials.

## Independent CN authorization

The account panel and original provider card invoke the backend's device+PKCE session. New account creation is DB-only and does not select a provider or rewrite official/Codex login files. The flow uses a fresh nonce and ephemeral verifier, the public production client ID, and a machine ID generated and owned by LoongPort. It does not copy the official client's tokens or authentication files.

The fixed authorization URL is `https://qoder.cn/device/selectAccounts`; polling uses `https://openapi.qoder.com.cn/api/v1/deviceToken/poll`. A token response must match authenticated `/api/v1/userinfo` identity before publication. Refresh stays with the original encrypted owner at `/api/v1/deviceToken/refresh`, using the same machine ID and strict server-provided expiry. No guessed refresh expiry, client secret or mandatory UMID is invented.

Protocol facts were cross-checked against existing static artifacts: `@qodercn-ai/qoderclicn` 1.1.65 bundle SHA-256 `b5f6e7150b49eb885469337876069ee7b06ba7776c13be7f890723395a45b9c4`, and MIT `@magpie-community/opencode-qoder-auth` 0.2.27 `index.mjs` SHA-256 `f5add3379aa9ffaa1c245b4daeb18e6e52352a07e51c8f71d89c0b9c8652fa0f`. This slice uses their protocol facts; no vendor bundle is redistributed. The public production client identifier is not a client secret or evidence of a partnership.

Cancel/timeout, provider changes, replacement attempts and vault epochs fence both polling and publication. Cancelling a pending session retires that local attempt; it does not claim server-side revocation of an already committed grant. Recovery preserves known UID and machine ID; changing the bound account requires explicit confirmation and a new machine ID. Unknown refresh outcomes require explicit new authorization, never blind refresh replay.

## Requests and failures

Each request carries complete admitted conversation history. External tools are returned to the client for execution and their results are included in the next request. No cross-request Query or second account store exists. Unsupported input forms and `strict:true` tools are rejected rather than weakened.

The worker emits a bounded private head, base64 Chat chunks, and an end record. Rust withholds the final exact `data: [DONE]\n\n` chunk until clean end and real process exit 0; extra frames are rejected. The original pool, response fence and Chat-to-Responses renderer remain the consumers. Cancel/stdin EOF abort the single request; unknown or partial outcomes cannot replay on another account.

Only a verified device authentication rejection can retire the matching snapshot generation. Qoder's queue/quota/request business errors can also use HTTP 403 and must retain credentials. Unclassifiable or oversized error bodies do not trigger revocation. A late rejection cannot delete a newer grant.

## CN free-benefit claims

The existing CN account panel and Provider card offer an explicit status query, a manual free-benefit batch, and a separate per-account automatic opt-in. Opening the view only reads local context. Automatic claiming defaults off and uses the existing maintenance owner while the app is running and the vault is available; enabling it does not itself issue a claim. Reauthorization revokes the previous consent. Routine access-token refresh remains owned by the existing encrypted credential owner.

The batch reads the fixed CN campaign listing and handles each explicit `CLAIM_BENEFIT` + `CLAIMABLE` activity independently. It sends only `POST /sash/api/v1/me/campaigns/{encoded campaignId}/claim` with `{}`. It does not follow activity/detail URLs, buy or subscribe, spend credits, accept extra terms, or complete required achievements. Explicit extra conditions and unsupported actions are skipped and explained. There is no invented mandatory `isFree`, price, amount, campaign key, interval, or listing UID field. Any returned UID must match the authenticated original owner.

Endpoint and response facts were checked against MIT Magpie `packages/qoder/index.mjs`, commit `08ff18a43853ccf23b30fc6dd39b3f10591331ee`, lines 1984–2061. The service implementation is independent; existing MIT notices remain in `scripts/qoder/LICENSE`. The request uses an existing CN PAT-derived job token or independently authorized device access token, with fixed origin and source/generation/UID/vault/policy fences. It never fabricates official client runtime identity. Service authentication or challenge failures stop safely rather than switching identities or bypassing restrictions.

The existing durable campaign ledger records intent before dispatch. A result-unknown activity is never posted again: subsequent runs can only query the same listing target, including its original optional key and interval. A matching `CLAIMED` status confirms observed state, not a newly added amount. Missing activity identifiers, changed target facts, unrecognized responses, or an unresolved prior request remain unconfirmed. For a directly confirmed POST receipt, a separately observed `CLAIMED` listing is persisted; a still later same-target `CLAIMABLE` listing can then authorize a new intent. Repeated `CLAIMABLE` alone never advances the receipt. A prior Unknown recovered through GET does not acquire this no-interval reopening permission. This observed-state sequence is not proof of a daily round or exactly-once server delivery: stale/inconsistent listings can still lead to a repeated submission, which relies on the fixed claim service’s own eligibility/idempotency checks. No local-calendar reset is used. Other independent activities can still proceed. Each account batch has a 90-second total budget, so a slow account cannot hold the original loop for all 100 activities; unsent activities remain eligible for a later run. Account deletion and backup restore do not erase local anti-replay facts. An unwindowed marker cannot be transferred to a different provider or credential-source generation, even for the same UID; the UI reports this limitation instead of silently treating it as completed.

These are generic free benefits, not a promised daily 100-credit reward. Activity start/end are not assumed to be daily rounds, and success never increments credits locally. The status-only query remains read-only and never refreshes credentials or claims.

## Validation boundary

Synthetic owner/session, protocol, real Node subprocess, and focused Rust owner-to-worker tests cover these paths. Focused Rust harnesses reuse cached dependencies and do not replace the full locked Tauri build or complete product-router suite. No real CN OAuth/refresh/inference account acceptance was performed. Production service eligibility, entitlements and coexistence with other server sessions remain live acceptance questions, not claims made by this implementation. No real CN campaign claim was executed during development. Copying/importing official credential files remains outside this implementation.
