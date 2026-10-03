# ZCode account checks

This lightweight Rust 1.95 harness compiles the actual product account modules,
existing vault crypto/error modules, OwnedFile registry/codec, private file writer
and shared ZCode directory lock. It contains no parallel vault or IO implementation.
The harness source graph is test-only and does not compile the Tauri application.

```sh
cargo test --locked --manifest-path tests/zcode-account-core/Cargo.toml -- --test-threads=2
cargo clippy --locked --manifest-path tests/zcode-account-core/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tests/zcode-account-core/Cargo.toml --check
```

Use an explicit `CARGO_TARGET_DIR` when disk space is limited. All account fixtures
use newly created private temporary directories. The subprocess matrix supplies
synthetic vault keys through a pipe, exits without destructors at each publication
point, and then authenticates/reopens actual files. Both ignored child fixtures
are invoked by ordinary parent tests; they are not skipped coverage. No real home,
credentials, keychain, ZCode process or database is accessed by this harness.

Coverage includes native AES-GCM framing and local identity extraction, scoped
A/B/A refresh, existing-vault AEAD, OwnedFile AAD and registration, bounded payloads,
file permissions/CAS, scoped rollback, uncertain commit-marker reconciliation,
restored-origin rejection, private path checks, and dead-owner lock reclamation.
Windows account IO refuses admission until a native adapter is implemented and
verified. Unix synthetic tests do not establish macOS live application acceptance.
Unknown/ownerless locks are preserved. Process-crash recovery is not a cross-file
power-loss guarantee or protection against a malicious process under the same UID.

The actual account modules also participate in the main crate's unit-test graph.
They have no live command or capture entry yet. Production registry and lifecycle
guards are present, with main-crate regressions for rotation, rewrap, reset and
bootstrap restore; these require the normal platform build. Main/native validation
is a separate gate and is not replaced by a green lightweight harness. The caller
must eventually supply verified contract/process/individual-account admission,
trusted journal provenance, and the existing sync owner plus SecretSession read
guard before entering account IO. API-key cards and local JWT parsing are not
proof of those conditions or of remote OAuth validity.

Current SQL/sync imports contain database and skills/settings payloads, not these
account files. A future account-file import must quarantine journals/recovery and
archive profiles before activation. Arbitrary manual filesystem copies are not
classified as trusted local journals by this test suite.

The native codec pins RustCrypto `aes-gcm = 0.11.1` (Apache-2.0 OR MIT); the main and
harness locks use the same added cryptographic package versions and checksums.
Other cryptographic families match the existing product. No new key manager,
login flow or environment-secret resolver is present. Contract facts come from
`zai-org/ZCode@29628c9acdb81b703bbd4080c207a0e7ce5e276e`; no upstream source
implementation is vendored. Existing license files remain unchanged.
