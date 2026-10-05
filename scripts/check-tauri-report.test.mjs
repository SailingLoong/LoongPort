import assert from "node:assert/strict";
import test from "node:test";
import { evaluateTauriInfo } from "./check-tauri-compatibility.mjs";

const declared = [
  "@tauri-apps/api",
  "@tauri-apps/plugin-dialog",
  "@tauri-apps/plugin-log",
  "@tauri-apps/plugin-process",
];
const accepted = `[-] Packages
    - tauri 🦀: 2.12.1
    - @tauri-apps/api  ⱼₛ: 2.12.1
[-] Plugins
    - tauri-plugin-dialog 🦀: 2.8.1
    - @tauri-apps/plugin-dialog  ⱼₛ: 2.8.1
    - tauri-plugin-log 🦀: 2.10.0
    - @tauri-apps/plugin-log  ⱼₛ: 2.10.0
    - tauri-plugin-process 🦀: 2.4.0
    - @tauri-apps/plugin-process  ⱼₛ: 2.4.0
`;
const report = (stdout = accepted, status = 0, stderr = "") => ({
  stdout,
  stderr,
  status,
});

test("accepts completed official diagnostics with every declared counterpart reported", () => {
  assert.equal(evaluateTauriInfo(report(), declared).ok, true);
});
test("rejects official four-package mismatch even when info exits zero", () => {
  const diagnostic = `Error: Found version mismatched Tauri packages.
 tauri (v2.12.1) : @tauri-apps/api (v2.11.1)
 tauri-plugin-dialog (v2.8.1) : @tauri-apps/plugin-dialog (v2.7.3)
 tauri-plugin-log (v2.10.0) : @tauri-apps/plugin-log (v2.9.2)
 tauri-plugin-process (v2.4.0) : @tauri-apps/plugin-process (v2.3.1)`;
  assert.equal(
    evaluateTauriInfo(report(accepted + diagnostic), declared).ok,
    false,
  );
});
test("rejects a fifth plugin mismatch diagnosed by the official tool", () => {
  assert.equal(
    evaluateTauriInfo(
      report(
        accepted,
        0,
        "Found version mismatched Tauri packages: tauri-plugin-opener",
      ),
      declared,
    ).ok,
    false,
  );
});
test("rejects command failure and timeout without accepting partial reports", () => {
  assert.equal(evaluateTauriInfo(report(accepted, 2), declared).ok, false);
  assert.equal(
    evaluateTauriInfo(
      { ...report(), status: null, error: new Error("ETIMEDOUT") },
      declared,
    ).ok,
    false,
  );
});
test("rejects empty successful output", () => {
  assert.equal(evaluateTauriInfo(report(""), declared).ok, false);
});
test("rejects the wrong project without the desktop API declaration", () => {
  assert.equal(evaluateTauriInfo(report(), []).ok, false);
});
test("rejects declared but unresolved or unreported plugin counterparts", () => {
  assert.equal(
    evaluateTauriInfo(report(), [...declared, "@tauri-apps/plugin-opener"]).ok,
    false,
  );
  assert.equal(
    evaluateTauriInfo(
      report(
        accepted.replace(
          "tauri-plugin-dialog 🦀: 2.8.1",
          "tauri-plugin-dialog 🦀: not installed!",
        ),
      ),
      declared,
    ).ok,
    false,
  );
  assert.equal(
    evaluateTauriInfo(
      report(
        accepted.replace(
          "@tauri-apps/plugin-dialog  ⱼₛ: 2.8.1",
          "@tauri-apps/plugin-dialog  ⱼₛ: not installed!",
        ),
      ),
      declared,
    ).ok,
    false,
  );
});
test("does not reject irrelevant native environment warnings or compare patch versions", () => {
  const stdout =
    accepted.replace(
      "@tauri-apps/api  ⱼₛ: 2.12.1",
      "@tauri-apps/api  ⱼₛ: 2.12.0",
    ) + "WARNING: optional environment package is not installed";
  assert.equal(evaluateTauriInfo(report(stdout), declared).ok, true);
});
