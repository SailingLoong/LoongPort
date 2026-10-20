# Qoder HTTP runtime (isolated replacement candidate)

This replaces the previous per-request SDK worker with the fixed International Qoder HTTP/SSE protocol derived from @magpie-community/opencode-qoder-auth 0.2.18 (plugins commit cdf45137d644602bd2452491a8ff79df1a255e59). LICENSE and LICENSE-CLIProxyAPI retain the upstream notices.

Node.js 22 or later is required. No Qoder SDK package, SDK root, Bun runtime, plugin account store, or caller-selected executable is used. `node worker.mjs --check-runtime` is an offline protocol probe.

The original LoongPort Provider remains the only PAT source. Private encrypted subordinate state owns access/refresh/UID/stable machine ID and exact refresh recovery. The worker receives only one private request snapshot through stdin. Incoming HTTP headers, options, nodePath, and sdkRoot cannot select credentials or a runtime. International PAT identity must be explicitly established; existing SDK tokens are not silently migrated. CN is unsupported.

Each request contains its complete admitted conversation history. External tools are returned to the client, which sends their results in its next request. The worker does not execute those tools and no cross-request Query, capability registry, or second account store is used. Tools with strict:true and unsupported input forms are rejected rather than weakened.

The worker emits one bounded private head, base64 standard Chat chunks, and an end record. Streaming success ends with a separately emitted exact `data: [DONE]\n\n` chunk. The Rust consumer retains that final chunk until the end record and real process exit 0 are confirmed; additional chunks after it are invalid. This is a contract with this bundled producer, not an arbitrary SSE parser. The existing pool, response fence, and Chat-to-Responses renderer remain the consumers.

Cancel and stdin EOF abort the single upstream request. Unknown submitted outcomes and partial output must not replay on another account. Refresh outcomes use the original private owner's durable pending/unknown state.

Status: this is an isolated candidate. Node protocol and subprocess tests have run; the new Rust integration and product-router tests have not yet compiled or executed. Production admission remains closed pending those tests and the explicit user entry. Do not describe this candidate as real-account or entitlement acceptance.
