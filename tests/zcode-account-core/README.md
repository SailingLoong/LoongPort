# ZCode account checks

This lightweight Rust 1.95 harness compiles the actual product account modules,
existing vault crypto/error modules, OwnedFile registry/codec, private file writer,
shared ZCode directory lock and native context probe. It contains no parallel vault
or IO implementation. The harness source graph is test-only; the Tauri commands,
API and physical runtime owner require the normal main-crate build and tests.

```sh
cargo test --locked --manifest-path tests/zcode-account-core/Cargo.toml -- --test-threads=2
cargo clippy --locked --manifest-path tests/zcode-account-core/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tests/zcode-account-core/Cargo.toml --check
python3 tests/zcode-account-core/check-openat-abi.py
```

Use an explicit `CARGO_TARGET_DIR` when disk space is limited. All account fixtures
use newly created private temporary directories. The subprocess matrix supplies
synthetic vault keys through a pipe, exits without destructors at each publication
point, and then authenticates/reopens actual files. Three ignored child fixtures
on Linux, and a fourth on macOS, are invoked by ordinary parent tests; they are
not skipped coverage. The macOS probe tests read OS account metadata, enumerate
PIDs and inspect their own fixture process and temporary local mount. No real
home configuration, credentials or keychain is opened, no ZCode app is launched,
and the lightweight harness does not open a database.

Coverage includes native AES-GCM framing and local identity extraction, scoped
A/B/A refresh, existing-vault AEAD, OwnedFile AAD and registration, bounded payloads,
file permissions/CAS, uncertain commit-marker classification, bounded encrypted
keep-native recovery, whole-before/whole-after confirmation, restored-journal
preservation, private path checks, and dead-owner lock reclamation.
Windows account IO refuses admission until a native adapter is implemented and
verified. Unix synthetic tests do not establish macOS live application acceptance.
Unknown/ownerless locks are preserved. Process-crash recovery is not a cross-file
power-loss guarantee or protection against a malicious process under the same UID.

The actual account modules participate in the main crate's production and
unit-test graphs. Thin commands and an explicit-action UI use the same runtime
owner. The exact supported macOS contract has passed its native implementation
gate; other platforms and unknown builds refuse native account operations.
Production registry and lifecycle guards have main-crate regressions for rotation,
rewrap, reset and bootstrap restore. That main/native validation is not replaced
by a green lightweight harness. The backend checks selected context, artifacts,
stopped writers and individual-account admission, then holds the existing sync
owner and SecretSession read guard before account IO. Actual user capture and
switching require their explicit actions. API-key cards, local JWT parsing and
cached official account labels do not prove remote OAuth validity or actual
account A/B/A acceptance.

Current SQL/sync imports contain database and skills/settings payloads, not these
account files. Recovery never replays a saved journal over native credentials:
authenticated source/target/preimage evidence is retained in the existing vault,
and unresolved records block ordinary switching. Arbitrary manual filesystem
copies do not establish that an old journal describes the current native state.

The native codec pins RustCrypto `aes-gcm = 0.11.1` (Apache-2.0 OR MIT); the main and
harness locks use the same added cryptographic package versions and checksums.
Other cryptographic families match the existing product. No new key manager,
login flow or environment-secret resolver is present. Contract facts come from
`zai-org/ZCode@29628c9acdb81b703bbd4080c207a0e7ce5e276e`; no upstream source
implementation is vendored. Existing license files remain unchanged.

The ABI probe compiles the actual product openat call expression with synthetic
16-bit and 32-bit mode_t declarations. It performs no file operation and does not
replace the required macOS native compile/test gate.
