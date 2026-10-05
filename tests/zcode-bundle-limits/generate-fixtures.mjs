// Test-only fixed pjpv format vectors. All identities, sessions and keys are
// fabricated; this script never reads a file, environment or native account.
import { createCipheriv, createHash, pbkdf2Sync } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("./fixtures/", import.meta.url));
mkdirSync(root, { recursive: true });
const password = "synthetic-zsb-password";
const localSecret =
  "zcode-credential-fallback:darwin:/synthetic/local:fixture-user";
const foreignSecret =
  "zcode-credential-fallback:win32:C:/synthetic/foreign:fixture-user";
let ivCounter = 0;

function nativeValue(value, secret) {
  const iv = Buffer.alloc(12);
  iv.writeUInt32BE(++ivCounter, 8);
  const key = createHash("sha256").update(secret).digest();
  const cipher = createCipheriv("aes-256-gcm", key, iv);
  const data = Buffer.concat([cipher.update(value, "utf8"), cipher.final()]);
  return `enc:v1:${iv.toString("base64url")}.${cipher.getAuthTag().toString("base64url")}.${data.toString("base64url")}`;
}

function account(id, secret = localSecret) {
  const credentials = {};
  for (const [key, value] of Object.entries({
    "oauth:active_provider": "bigmodel",
    "oauth:bigmodel:access_token": `synthetic-session-${id}`,
    "oauth:bigmodel:user_info": JSON.stringify({
      id,
      username: "fixture-user",
      displayName: "Synthetic account",
    }),
  }))
    credentials[key] = nativeValue(value, secret);
  return {
    name: "Synthetic account",
    createdAt: "2026-10-05T00:00:00Z",
    credentials,
    config: null,
  };
}

function seal(inner) {
  const salt = Buffer.alloc(16, 0x51);
  const nonce = Buffer.alloc(12, 0x62);
  const key = pbkdf2Sync(password, salt, 100_000, 32, "sha256");
  const cipher = createCipheriv("aes-256-gcm", key, nonce);
  const data = Buffer.concat([
    cipher.update(JSON.stringify(inner)),
    cipher.final(),
  ]);
  return {
    format: "zsw-accounts-bundle",
    version: 1,
    kdf: {
      algo: "pbkdf2-hmac-sha256",
      iters: 100_000,
      salt: salt.toString("base64"),
    },
    cipher: {
      algo: "aes-256-gcm",
      nonce: nonce.toString("base64"),
      tag: cipher.getAuthTag().toString("base64"),
      data: data.toString("base64"),
    },
  };
}

const inner = (accounts) => ({
  format: "zcode-accounts-bundle",
  version: 2,
  exportedAt: "2026-10-05T00:00:00Z",
  accounts,
});
const valid = seal(inner([account("synthetic-one"), account("synthetic-two")]));
const cases = {
  "valid-multiple.zsb": JSON.stringify(valid),
  "duplicate-identity.zsb": JSON.stringify(
    seal(inner([account("synthetic-one"), account("synthetic-one")])),
  ),
  "foreign-inner-key.zsb": JSON.stringify(
    seal(inner([account("synthetic-foreign", foreignSecret)])),
  ),
  "unknown-outer-version.zsb": JSON.stringify({ ...valid, version: 2 }),
  "unknown-kdf.zsb": JSON.stringify({
    ...valid,
    kdf: { ...valid.kdf, algo: "unknown" },
  }),
  "duplicate-outer-key.zsb": `{"version":99,${JSON.stringify(valid).slice(1)}`,
  "unexpected-inner-field.zsb": JSON.stringify(
    seal({ ...inner([account("synthetic-one")]), unexpected: true }),
  ),
  "over-account-limit.zsb": JSON.stringify(
    seal(
      inner(Array.from({ length: 51 }, (_, i) => account(`synthetic-${i}`))),
    ),
  ),
  "truncated.zsb": JSON.stringify(valid).slice(0, 80),
};
const tag = Buffer.from(valid.cipher.tag, "base64");
tag[0] ^= 1;
cases["damaged-authentication-tag.zsb"] = JSON.stringify({
  ...valid,
  cipher: { ...valid.cipher, tag: tag.toString("base64") },
});
const manifest = {};
for (const [name, text] of Object.entries(cases)) {
  const bytes = Buffer.from(text + "\n");
  writeFileSync(root + name, bytes);
  manifest[name] = {
    bytes: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
}
writeFileSync(root + "manifest.json", JSON.stringify(manifest, null, 2) + "\n");
console.log(
  `Generated ${Object.keys(cases).length} synthetic encrypted format vectors; no product decoder acceptance asserted.`,
);
