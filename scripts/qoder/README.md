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

## Explicit CN campaign status

The existing CN account panel and Provider card can explicitly read the fixed CN campaign listing using the original encrypted credential owner. Opening a view does not make a service request. The read never establishes or refreshes credentials, sends a claim, changes the daily ledger, or reads official client files. It uses an existing CN PAT-derived job token or independently authorized device access token, with source/generation/UID/vault fences before and after the request. No International token fallback or caller-selected destination is allowed.

Endpoint and listing field facts were checked against MIT Magpie `packages/qoder/index.mjs`, commit `08ff18a43853ccf23b30fc6dd39b3f10591331ee`, lines 1984–2061. No implementation code was copied in this slice. The existing MIT notices remain in `scripts/qoder/LICENSE`. `CLAIM_BENEFIT`, `CLAIMABLE` and `CLAIMED` describe generic activities, not proven daily eligibility. Activity start/end are not treated as daily claim rounds. Missing listing UID is permitted because the original authenticated owner binds the request; any returned UID must match. A successful GET neither proves desktop/runtime eligibility nor a daily reward. Automatic CN claim stays disabled until the daily activity and round semantics are evidenced.

## Validation boundary

Synthetic owner/session, protocol, real Node subprocess, and focused Rust owner-to-worker tests cover these paths. Focused Rust harnesses reuse cached dependencies and do not replace the full locked Tauri build or complete product-router suite. No real CN OAuth/refresh/inference account acceptance was performed. Production service eligibility, entitlements and coexistence with other server sessions remain live acceptance questions, not claims made by this implementation. CN campaign check-in and copying/importing official credential files are not implemented by this slice.
