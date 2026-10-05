import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";

test("installed counterparts pass the official Tauri compatibility diagnostic", () => {
  const result = spawnSync(
    process.execPath,
    ["node_modules/@tauri-apps/cli/tauri.js", "info"],
    {
      encoding: "utf8",
      timeout: 120_000,
      env: {
        ...process.env,
        NO_COLOR: "1",
        npm_config_offline: "true",
        npm_config_fetch_retries: "0",
        npm_config_fetch_timeout: "5000",
      },
    },
  );
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(
    result.stdout + result.stderr,
    /Found version mismatched Tauri packages/,
    "the official checker must accept the installed Rust/NPM package generation",
  );
});
