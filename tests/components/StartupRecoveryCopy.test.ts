import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
describe.each([en, zh, ja, zhTW])(
  "original-operation recovery copy",
  (locale) => {
    it("provides complete local recovery copy, not a strength/hex requirement or current-key promise", () => {
      const copy = (
        locale as unknown as { startupRecovery: Record<string, string> }
      ).startupRecovery;
      expect(copy).toBeDefined();
      for (const key of [
        "title",
        "checkTitle",
        "description",
        "sourceKey",
        "sourceHelp",
        "resume",
        "recheck",
        "statusFailed",
        "keyRejected",
        "sourceChanged",
        "unknownResult",
        "notCompleted",
        "verificationRequired",
        "completed",
        "restartDescription",
        "restartFailed",
      ]) {
        expect(copy[key]?.length).toBeGreaterThan(0);
      }
      expect(copy.sourceHelp).toMatch(
        /cannot guarantee|无法保证|無法保證|保証しません/,
      );
      expect(copy.sourceHelp + copy.sourceKey).not.toMatch(
        /64|32|hex|强度|強度|strength|复杂度|複雜度/i,
      );
    });
  },
);

it("keeps the frontend recovery DTO and original command names aligned with the Rust owner", async () => {
  const { readFile } = await import("node:fs/promises");
  const startup = await readFile("src-tauri/src/secrets/startup.rs", "utf8");
  const view = startup.match(
    /pub\(crate\) struct StartupRecoveryView\s*\{([^}]+)\}/,
  )?.[1];
  expect(view).toMatch(/token:\s*String/);
  expect(view).toMatch(/status:\s*&'static str/);
  expect(view).toMatch(/can_recover:\s*bool/);
  expect(view).toMatch(/restart_required:\s*bool/);
  const command = startup.match(
    /async fn recover_startup_operation\(([^)]+)\)/,
  )?.[1];
  expect(command).toMatch(/token:\s*String/);
  expect(command).toMatch(/password:\s*String/);
  const registration = await readFile("src-tauri/src/lib.rs", "utf8");
  expect(registration).toContain("secrets::startup::get_startup_recovery");
  expect(registration).toContain("secrets::startup::recover_startup_operation");
});
