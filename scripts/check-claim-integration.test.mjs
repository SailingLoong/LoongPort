import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const root = new URL("../", import.meta.url);
const read = (path) => readFileSync(new URL(path, root), "utf8");
const workbuddyCommands = [
  "list_workbuddy_accounts", "refresh_workbuddy_account", "refresh_all_workbuddy_accounts",
  "claim_workbuddy_today", "begin_workbuddy_authorization", "finish_workbuddy_authorization",
];
const zcodeCommands = [
  "get_zcode_claim_state", "set_zcode_claim_auto", "start_zcode_claim", "cancel_zcode_claim",
  "get_zcode_claim_captcha", "submit_zcode_claim_captcha", "show_zcode_claim_captcha",
];

test("both approved command sets are registered exactly once behind the captcha guard", () => {
  const source = read("src-tauri/src/lib.rs");
  const guard = source.indexOf("claim::allowed_command(");
  const handler = source.indexOf("tauri::generate_handler![", guard);
  assert.ok(guard > 0 && handler > guard);
  for (const command of [...workbuddyCommands, ...zcodeCommands]) {
    assert.equal(source.match(new RegExp(`commands::${command}\\b`, "g"))?.length ?? 0, 1, command);
  }
  const modules = read("src-tauri/src/commands/mod.rs");
  assert.match(modules, /mod workbuddy;/);
  assert.match(modules, /mod zcode_claim;/);
});

test("both translation namespaces coexist in all supported locales", () => {
  for (const locale of ["en", "zh", "zh-TW", "ja"]) {
    const content = JSON.parse(read(`src/i18n/locales/${locale}.json`));
    assert.ok(content.workbuddy?.claim, `${locale}: WorkBuddy check-in copy`);
    assert.ok(content.zcode?.claim, `${locale}: ZCode claim copy`);
  }
});

test("WorkBuddy has no automatic maintenance entry and ZCode keeps its 10-minute schedule", () => {
  const maintenance = read("src-tauri/src/maintenance/mod.rs");
  assert.match(maintenance, /start_zcode_claim\(app\.clone\(\)\)/);
  assert.doesNotMatch(maintenance, /workbuddy/i);
  assert.match(read("src-tauri/src/zcode_accounts/claim_runtime.rs"), /INTERVAL: Duration = Duration::from_secs\(600\)/);
});

test("WorkBuddy encrypted file and ZCode protected setting remain separate", () => {
  assert.match(read("src-tauri/src/secrets/owned_file.rs"), /WORKBUDDY_FILE: &str = "workbuddy_accounts\.json"/);
  assert.match(read("src-tauri/src/secrets/inventory.rs"), /"zcode_claim_v1"/);
});

test("newer baseline retains the macOS foreground restart fix", () => {
  assert.match(read("src-tauri/src/lib.rs"), /fn relaunch_macos_bundle\(/);
  assert.match(read("src-tauri/src/commands/settings.rs"), /crate::restart_process\(&app\)/);
});

test("explicit exit and restart close claim admission before asynchronous cleanup", () => {
  const source = read("src-tauri/src/lib.rs");
  const exit = source.slice(source.indexOf("if let RunEvent::ExitRequested"), source.indexOf("if let RunEvent::ExitRequested") + 500);
  assert.ok(exit.includes("if code.is_some()"), "background tray close must stay active");
  assert.ok(exit.includes("zcode_accounts::claim_runtime::shutdown();"), "explicit exit must stop admission immediately");
  for (const signature of ["pub async fn cleanup_before_exit(", "pub fn restart_process("]) {
    const body = source.slice(source.indexOf(signature)).split("{")[1];
    assert.ok(body.trimStart().startsWith("zcode_accounts::claim_runtime::shutdown();"), signature);
  }
});

test("failed Windows update explains that claim admission requires a restart", () => {
  assert.ok(read("src-tauri/src/services/app_update.rs").includes("套餐领取已暂停至重启"));
});
