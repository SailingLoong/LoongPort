import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  writeFileSync,
  rmSync,
  readFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  createScreenshotSource,
  assertScreenshotSource,
} from "./screenshot-source.mjs";
function fixture(t) {
  const root = mkdtempSync(path.join(os.tmpdir(), "qoder-source-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) =>
    execFileSync("git", args, { cwd: root, stdio: "ignore" });
  git("init", "-q");
  git("config", "user.name", "Fixture");
  git("config", "user.email", "fixture@example.invalid");
  mkdirSync(path.join(root, "src"), { recursive: true });
  mkdirSync(path.join(root, "tests/browser"), { recursive: true });
  writeFileSync(
    path.join(root, "src/component.tsx"),
    "export const one = 1;\n",
  );
  writeFileSync(path.join(root, "tests/browser/fixture.tsx"), "fixture\n");
  git("add", ".");
  git("commit", "-qm", "baseline");
  return root;
}
const modules = ["src/component.tsx", "tests/browser/fixture.tsx"];
test("dirty tracked source is previewable and strict rejects it", (t) => {
  const root = fixture(t);
  writeFileSync(
    path.join(root, "src/component.tsx"),
    "export const one = 2;\n",
  );
  const preview = createScreenshotSource(root, modules);
  assert.equal(preview.dirty, true);
  assert.equal(preview.head.length, 40);
  assert.throws(
    () => createScreenshotSource(root, modules, { strict: true }),
    /dirty|untracked/i,
  );
});
test("related untracked Tailwind input rejects strict; unrelated output does not", (t) => {
  const root = fixture(t);
  writeFileSync(path.join(root, "tests/browser/unrelated.png"), "output");
  assert.equal(
    createScreenshotSource(root, modules, { strict: true }).dirty,
    false,
  );
  writeFileSync(path.join(root, "src/new.tsx"), "export const added = true;\n");
  assert.equal(createScreenshotSource(root, modules).dirty, true);
  assert.throws(
    () => createScreenshotSource(root, modules, { strict: true }),
    /dirty|untracked/i,
  );
});
test("different source after build cannot be mistaken for the built source", (t) => {
  const root = fixture(t);
  const built = createScreenshotSource(root, modules);
  writeFileSync(
    path.join(root, "src/component.tsx"),
    "export const one = 9;\n",
  );
  assert.throws(() => assertScreenshotSource(root, built), /mismatch/i);
});

test("an imported untracked browser module is an input, but an unused module is not", (t) => {
  const root = fixture(t);
  writeFileSync(path.join(root, "tests/browser/unused.tsx"), "unused\n");
  assert.equal(
    createScreenshotSource(root, modules, { strict: true }).dirty,
    false,
  );
  assert.throws(
    () =>
      createScreenshotSource(root, [...modules, "tests/browser/unused.tsx"], {
        strict: true,
      }),
    /untracked/i,
  );
});
test("strict verifies a clean ordinary build without claiming preview is an acceptance", (t) => {
  const root = fixture(t);
  const preview = createScreenshotSource(root, modules);
  assert.equal(assertScreenshotSource(root, preview).exactCommit, false);
  assert.equal(
    assertScreenshotSource(root, preview, { strict: true }).exactCommit,
    true,
  );
  writeFileSync(path.join(root, "src/component.tsx"), "dirty\n");
  const dirtyPreview = createScreenshotSource(root, modules);
  assert.throws(
    () => assertScreenshotSource(root, dirtyPreview, { strict: true }),
    /dirty|untracked/i,
  );
});
test("legacy captures run before and independently of unified strict checks", () => {
  const workflow = readFileSync(
    new URL("../../.github/workflows/docs-ui.yml", import.meta.url),
    "utf8",
  );
  const shared = workflow
    .split("- name: Check current ZCode conflict layout with synthetic data")[1]
    ?.split("      - name:")[0];
  const unified = workflow
    .split("- name: Check current unified account screenshots")[1]
    ?.split("      - name:")[0];
  assert.ok(shared && unified);
  assert.ok(
    workflow.indexOf(
      "- name: Check current ZCode conflict layout with synthetic data",
    ) < workflow.indexOf("- name: Check current unified account screenshots"),
  );
  assert.match(shared, /run-zcode-layout\.mjs/);
  assert.match(shared, /run-applications-c2\.mjs/);
  assert.doesNotMatch(
    shared,
    /UNIFIED_LAYOUT_STRICT|screenshot-source\.node|run-unified-accounts/,
  );
  assert.match(shared, /UNIFIED_LAYOUT_COLLECT=0/);
  assert.match(unified, /UNIFIED_LAYOUT_STRICT=1/);
  assert.match(unified, /screenshot-source\.node/);
  assert.match(unified, /run-unified-accounts\.mjs/);
});
