# ZCode account pure-core checks

This lightweight Rust 1.95 harness compiles the product source at
`src-tauri/src/zcode_accounts/core.rs` directly. It does not copy the
implementation or compile Tauri.

Run from the repository root:

```sh
cargo test --locked --manifest-path tests/zcode-account-core/Cargo.toml
cargo clippy --locked --manifest-path tests/zcode-account-core/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tests/zcode-account-core/Cargo.toml --check
```

Set `CARGO_TARGET_DIR` to an explicit disposable build directory if space is
limited. `tempfile` is used only to round-trip synthetic documents in a new test
directory; all other tests are pure in-memory decisions.

The module is preparation for a future adapter, not a user-accessible switch.
It is not registered in the main crate or any Tauri command. The caller must
authenticate the native identity, enforce version/cipher/process/lock gates,
protect and persist snapshots, classify the live journal, and then apply the
returned decisions. This harness does not validate AES-GCM credentials, OS
keychains, native journal AEAD, fsync/atomic replacement, process crash or power
failure. Synthetic phase tests are not native transaction acceptance.

The independent Rust key implementation follows the interface at pinned
`zai-org/ZCode@29628c9acdb81b703bbd4080c207a0e7ce5e276e`:
`packages/services/src/model-provider/accountProviderCredentialKey.ts` and
`packages/services/src/oauth/repo/oauthCredentialRepo.ts`. No upstream source
implementation is vendored in this slice. The Unicode fixtures were checked
against that exact official TypeScript function using Node's type stripping.
