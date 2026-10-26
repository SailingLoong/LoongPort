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

function assertOriginalRecoveryContract(startup: string, registration: string) {
  const view = startup.match(
    /pub\(crate\) struct StartupRecoveryView\s*\{([^}]+)\}/,
  )?.[1];
  expect(view).toBeDefined();
  expect(view).toMatch(/token:\s*String/);
  expect(view).toMatch(/status:\s*&'static str/);
  expect(view).toMatch(/can_recover:\s*bool/);
  expect(view).toMatch(/restart_required:\s*bool/);
  // The native owner is generic over the Tauri runtime for its real IPC tests.
  // Generics do not change the wire arguments; never make those arguments optional.
  const command = startup.match(
    /async\s+fn\s+recover_startup_operation\s*(?:<[^(){}]*>)?\s*\(([^)]+)\)/,
  )?.[1];
  expect(command).toBeDefined();
  expect(command).toMatch(/token:\s*String/);
  expect(command).toMatch(/password:\s*String/);
  expect(registration).toContain("secrets::startup::get_startup_recovery");
  expect(registration).toContain("secrets::startup::recover_startup_operation");
}

async function readNativeContract() {
  const { readFile } = await import("node:fs/promises");
  return Promise.all([
    readFile("src-tauri/src/secrets/startup.rs", "utf8"),
    readFile("src-tauri/src/lib.rs", "utf8"),
  ]);
}

it("keeps the frontend recovery DTO and original command names aligned with the Rust owner", async () => {
  const [startup, registration] = await readNativeContract();
  assertOriginalRecoveryContract(startup, registration);
});

it("accepts the same original wire contract with or without a runtime generic", async () => {
  const [startup, registration] = await readNativeContract();
  const ordinary = startup.replace(
    /recover_startup_operation\s*<[^(){}]*>\s*\(/,
    "recover_startup_operation(",
  );
  assertOriginalRecoveryContract(ordinary, registration);
  assertOriginalRecoveryContract(
    ordinary.replace(
      "async fn recover_startup_operation(",
      "async fn recover_startup_operation <R: tauri::Runtime> (",
    ),
    registration,
  );
});

it.each(["token", "password"])(
  "still rejects a renamed recovery wire argument: %s",
  async (argument) => {
    const [startup, registration] = await readNativeContract();
    const broken = startup.replace(
      /(async\s+fn\s+recover_startup_operation\s*(?:<[^(){}]*>)?\s*\()([^)]+)(\))/,
      (_, head: string, args: string, tail: string) =>
        head +
        args.replace(new RegExp(`\\b${argument}:`), "changed_argument:") +
        tail,
    );
    expect(broken).not.toBe(startup);
    expect(() =>
      assertOriginalRecoveryContract(broken, registration),
    ).toThrow();
  },
);

it("still rejects a missing recovery capability field or command registration", async () => {
  const [startup, registration] = await readNativeContract();
  expect(() =>
    assertOriginalRecoveryContract(
      startup.replace("can_recover: bool", "changed_capability: bool"),
      registration,
    ),
  ).toThrow();
  expect(() =>
    assertOriginalRecoveryContract(
      startup,
      registration.replaceAll(
        "secrets::startup::recover_startup_operation",
        "removed_command",
      ),
    ),
  ).toThrow();
});
