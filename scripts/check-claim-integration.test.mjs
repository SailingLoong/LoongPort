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

test("invalid account balance is recorded without aborting the remaining claim queue", () => {
  const source = read("src-tauri/src/zcode_accounts/claim_runtime.rs");
  const start = source.indexOf("let prior_plan_ids = match super::claim::prior_plan_ids(&balance)");
  assert.ok(start > 0, "balance shape must be validated before selecting or submitting a plan");
  const guarded = source.slice(start, source.indexOf("let raw = match client", start));
  assert.match(guarded, /failure\(&mut r, OfficialError::InvalidResponse\)/);
  assert.match(guarded, /record\(db, p, &r\)\.await\?/);
  assert.match(guarded, /return Ok\(\(\)\)/);
  assert.match(source, /r\.prior_plan_ids = prior_plan_ids;/);
  assert.doesNotMatch(source, /\.ok_or\("invalidResponse"\)\?/);
});

test("claim round still stops on cancellation or failure to preserve a result", () => {
  const source = read("src-tauri/src/zcode_accounts/claim_runtime.rs");
  const round = source.slice(source.indexOf("async fn round("), source.indexOf("pub(crate) fn shutdown()"));
  assert.match(round, /if lease\.cancelled\(\)\s*\{\s*break;/);
  assert.match(round, /if one\([\s\S]*?\.await\s*\.is_err\(\)\s*\{\s*break;/);
});

test("uncertain claim recovery returns before balance validation and any new POST", () => {
  const source = read("src-tauri/src/zcode_accounts/claim_runtime.rs");
  const one = source.slice(source.indexOf("async fn one("), source.indexOf("fn failure("));
  const pending = one.indexOf("if r.status == Status::ResultPending");
  const recoveryEnd = one.indexOf("let balance = match balance", pending);
  const validation = one.indexOf("let prior_plan_ids = match");
  const submit = one.indexOf("r.mark_submitted()");
  assert.ok(pending > 0 && recoveryEnd > pending && validation > recoveryEnd && submit > validation);
  assert.match(one.slice(pending, recoveryEnd), /r\.reconcile\(&balance\)/);
  assert.match(one.slice(pending, recoveryEnd), /record\(db, p, &r\)\.await\?;\s*return Ok\(\(\)\)/);
});

test("preflight persistence must succeed at both result layers before another account is admitted", () => {
  const source = read("src-tauri/src/zcode_accounts/claim_runtime.rs");
  const round = source.slice(source.indexOf("async fn round("), source.indexOf("pub(crate) fn shutdown()"));
  const preflight = round.slice(round.indexOf("Err(reason) =>"));
  assert.match(preflight, /let persisted = run_owned\(/);
  assert.match(preflight, /save\(db, &saved\)/);
  assert.match(preflight, /if !matches!\(persisted, Ok\(Ok\(\(\)\)\)\)\s*\{\s*break;/);
});
