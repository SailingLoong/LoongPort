import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { runTauriDiagnostics } from "./check-tauri-compatibility.mjs";

const root = process.cwd();
const declared = [
  "@tauri-apps/api",
  "@tauri-apps/plugin-dialog",
  "@tauri-apps/plugin-log",
  "@tauri-apps/plugin-process",
];
for (const mode of ["failed", "partial", "malformed", "missing"]) {
  test(`actual official query fails closed with ${mode} resolution despite info exit0`, () => {
    const temporary = mkdtempSync(path.join(os.tmpdir(), "tauri-query-test-"));
    try {
      const entry = path.join(temporary, "pnpm.mjs");
      if (mode !== "missing")
        writeFileSync(
          entry,
          `const args=process.argv.slice(2);const versions=${JSON.stringify(Object.fromEntries(declared.map((name) => [name, name.endsWith("/api") ? "2.11.1" : { "@tauri-apps/plugin-dialog": "2.8.1", "@tauri-apps/plugin-log": "2.10.0", "@tauri-apps/plugin-process": "2.4.0" }[name]])))};if(args.includes('--json')){${mode === "failed" ? "process.exit(1)" : mode === "malformed" ? "console.log('bad json')" : "console.log(JSON.stringify([{dependencies:{'@tauri-apps/api':{version:'2.12.1'}}}]))"}}else if(args[0]==='list'){if(versions[args[1]])console.log(args[1]+'@'+versions[args[1]])}else if(args[0]==='-v')console.log('10.12.3');`,
        );
      const result = runTauriDiagnostics({ root, declared, pnpmEntry: entry });
      assert.equal(result.ok, false, result.reason);
      if (mode !== "missing")
        assert.match(result.reason, /bulk package resolution/);
      if (mode !== "missing")
        assert.equal(
          result.report.status,
          0,
          "official info swallows the injected bulk query failure",
        );
    } finally {
      rmSync(temporary, { recursive: true, force: true });
    }
  });
}

test("actual official bulk query with frozen installed packages is recorded and accepted", () => {
  const result = runTauriDiagnostics({
    root,
    declared,
    pnpmEntry: process.env.npm_execpath,
  });
  assert.equal(result.ok, true, result.reason);
});
