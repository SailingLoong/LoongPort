# WorkBuddy native account ownership implementation plan

> For agentic workers: execute inline as the sole backend writer; read-only review only.

Goal: represent API and native-only credentials in the original encrypted WorkBuddy account owner, preserving legacy API wire data and refusing native credentials in HTTP.
Architecture: Account keeps the original uid/domain identity and ID algorithm. A backend credential enum serializes API credentials to the legacy token field, native-only snapshots to a distinct version1 native field. Strict deserialization rejects missing/ambiguous/unknown credential forms. Original registered FILE/load/save/deletion/migration paths remain authoritative.
Tech stack: existing Rust/serde/base64/zeroize/vault; no dependency changes.
Spec: parent delegated storage-contract instruction, 2026-10-29. No trusted capture producer/controller/UI/real credentials.

Files: model.rs, store.rs, native.rs (private snapshot persistence access), http.rs, engine.rs, inference.rs/authorization.rs and existing fixture call sites as required by the typed API. New native_account_tests.rs registered in mod.rs.
Input/output: load/save keep Vec<SavedAccount> and original StorageIdentity contract. Account::api_token returns Result<&str,Failure> and rejects native-only. Native row serialization has explicit native.version1/context/snapshot, no token. ID remains owner+domain+uid. Native capture context remains caller-verified, never authenticated by storage parsing.

Review focus/tests: legacy rows remain lossless; encrypted mixed rows reopen; unknown/missing versions and conflicting credential fields reject; no API capability/DTO secrets for native rows; repeated save/deletion and wrong binding/key preserve original owner and bytes. Historical duplicate API-only identities remain readable and persist without merging or field loss. Identity ambiguity involving any native snapshot rejects before write.

- [x] Write real production load/save regressions using fixture-native wire records and verify actual RED.
- [x] Add typed credential serde and original store validation, preserving legacy API format.
- [x] Gate original HTTP/engine/inference consumers with explicit API-only access; retain existing ID/delete and vault ownership.
- [x] Run new GREEN and affected original WorkBuddy suites, fmt and all-targets Clippy; record full-suite/platform unrun limits.
- [x] Independent read-only review; package exact diff, base, blobs, source and terminal logs. No remote/main/release writes.


Final representation: legacy API-only retains exactly the token-string Account JSON, including empty-token shared rows. Native-only has native.version1/context/snapshot without token. Coexistence after same-ID official API reauthorization requires credential_version1 plus both fields, retaining original opaque snapshot; empty hybrid token requires the original structurally valid shared grant. Native source authenticity is not inferred. StoredNativeSnapshot cannot convert to NativeSnapshot. Original HTTP Owner generation and fingerprint fence all retained snapshot/type changes; no second epoch or transaction protocol.

Additional real regression: authorization::finish snapshot loss reproduced before retention fix (0pass/1fail). Original routing feature target discovered28 but initially27pass/1fail in SQL-dump equality. Deterministic same-case cross-second diagnostic retained original assertion and proved only generated time header differed while complete logical digest and serialized physical DB matched. Final test-only snapshot checks both complete logical and physical state; live settings assertions remain. No production export, timeout or pause guard changed. Prior689 candidate:235+29+28 tests, all-targets Clippy, fmt/diff checks passed. No full entire-application rerun or real client/capture/controller acceptance claimed.

Next scope: see delivered NEXT_SLICE.txt. Root exclusively owns remote integration/CI; no local commit or remote write made by this implementation agent.

Compatibility review follow-up: identical duplicate-API encrypted store fixture passes original689 and fails the initial f5 composite candidate (real RED). Validation is narrowed to identity conflicts involving NativeOnly or ApiWithNative; legacy pure API duplicates remain unchanged, preserving original IDs and complete rows across repeated saves. Final composite verification is recorded separately in native-account-compat-evidence. Base is f5c171d212e337062e032fd404708dd5e3970c9a; no main sync.
