# Synthetic parser / crypto vectors

Regenerate with `node tests/zcode-bundle-limits/generate-fixtures.mjs`.
Node's built-in crypto creates the fixed pjpv outer envelope and native
`enc:v1` values from fabricated sessions; it does not run third-party code or
read environment, settings, credentials, Keychain or any native application.
No decrypted inner document is written to disk.

These are test-only deterministic salts/nonces; never use this generator with
real accounts. The fixed password is `synthetic-zsb-password`. The synthetic
local native secret is
`zcode-credential-fallback:darwin:/synthetic/local:fixture-user`.
The foreign-key vector uses
`zcode-credential-fallback:win32:C:/synthetic/foreign:fixture-user`.
These strings describe virtual inputs, not the machine running the tests.

Vectors cover multi-account structure, duplicate stable identity, foreign
inner key, unknown outer version/KDF, duplicate outer JSON key, unexpected
inner field, excessive account count, truncation and damaged authentication
tag. Wrong-password tests reuse valid-multiple.zsb with a different password.

Expected decoder behavior: authenticated bounded structure can proceed to
native compatibility/identity/scope review; malformed or unknown envelopes
are rejected; foreign inner keys cannot become switchable accounts. A valid
synthetic envelope does not prove actual target-build import, personal scope,
remote login, quotas or cross-machine compatibility. Product decoder tests,
preview/commit tests and isolated native schema verification remain pending.
