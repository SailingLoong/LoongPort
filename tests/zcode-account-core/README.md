# ZCode account pure-core checks

This lightweight Rust 1.95 harness compiles the product sources under
`src-tauri/src/zcode_accounts/` plus the existing `secrets/crypto.rs`,
`secrets/error.rs` and application error types directly. It does not copy the
vault implementation or compile Tauri. Its entire source graph is test-only.

Run from the repository root:

```sh
cargo test --locked --manifest-path tests/zcode-account-core/Cargo.toml -- --test-threads=2
cargo clippy --locked --manifest-path tests/zcode-account-core/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path tests/zcode-account-core/Cargo.toml --check
```

Set `CARGO_TARGET_DIR` to an explicit disposable build directory if space is
limited. `tempfile` is used only to round-trip synthetic documents and encrypted
catalogs/journals in a new test directory. Test vault keys remain in memory;
no system keychain is used. Rusqlite is compiled solely to reuse the actual
application error types; no database is opened.

The module is preparation for a future adapter, not a user-accessible switch.
It is not registered in the main crate or any Tauri command. The caller must
resolve the correct native context, enforce version/process/lock gates, register
the files with the vault lifecycle, classify the live journal, and persist the
returned decisions safely. The codec tests verify actual native AES-GCM framing,
local identity extraction, existing-vault AEAD, context/scope validation, and
encrypted checkpoint reopening using synthetic data. They do not prove remote
OAuth/JWT validity, OS key custody, native filesystem locks, atomic durability,
process-crash recovery or power-failure safety. Dropping and reopening test
objects is not a native application restart acceptance test.

The native codec uses exactly `aes-gcm = 0.11.1` from RustCrypto (Apache-2.0 OR
MIT), locked with its registry checksum. Other cryptographic dependencies match
the existing product's version families. Unknown cipher/envelope/schema values
never fall back to plaintext. No new key manager, CLI login, or environment
secret resolver is present.

The independent Rust key implementation follows the interface at pinned
`zai-org/ZCode@29628c9acdb81b703bbd4080c207a0e7ce5e276e`:
`packages/services/src/model-provider/accountProviderCredentialKey.ts` and
`packages/services/src/oauth/repo/oauthCredentialRepo.ts`. No upstream source
implementation is vendored in this slice. The Unicode fixtures were checked
against that exact official TypeScript function using Node's type stripping.
