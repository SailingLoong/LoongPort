import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import zh from "@/i18n/locales/zh.json";

const required = [
  "nativeImport.open",
  "nativeImport.title",
  "nativeImport.description",
  "nativeImport.choose",
  "nativeImport.fileFilter",
  "nativeImport.source",
  "nativeImport.readConsent",
  "nativeImport.read",
  "nativeImport.unverified",
  "nativeImport.expires",
  "nativeImport.verified",
  "nativeImport.verifyConsent",
  "nativeImport.verify",
  "nativeImport.accessOnly",
  "nativeImport.expired",
  "nativeImport.saveConsent",
  "nativeImport.save",
  "nativeImport.error",
  "nativeImport.cancel",
  "credentials.accessOnly",
  "credentials.expires",
  "credentials.reimport",

  "description",
  "region",
  "regions.cn",
  "regions.global",
  "add",
  "reload",
  "empty",
  "enterCode",
  "authorize",
  "expiresAt",
  "finishAuthorization",
  "authorizationWaiting",
  "refresh",
  "claim",
  "recheck",
  "retire",
  "creditAmount",
  "entitlement.active",
  "entitlement.expires",
  "entitlement.inactive",
  "entitlement.plan",
  "entitlement.quotaFiveHour",
  "entitlement.quotaNotSubscribed",
  "entitlement.quotaPercent",
  "entitlement.quotaReset",
  "entitlement.quotaUnlimited",
  "entitlement.quotaWeekly",
  "entitlement.remaining",
  "entitlement.unknown",
  "startedAt",
  "attempt",
  "lastCheckedAt",
  "unknownHint",
  "status.unavailable",
  "status.claimable",
  "status.alreadyClaimed",
  "status.claimed",
  "status.unknown",
  "status.needsLogin",
  "operation.pending",
  "operation.unknown",
  "operation.claimed",
  "operation.alreadyClaimed",
  "observed.claimable",
  "observed.alreadyClaimed",
  "observed.unavailable",
  "errors.storageUnavailable",
  "errors.busy",
  "errors.needsLogin",
  "errors.unsupportedContext",
  "errors.staleIdentity",
  "errors.invalidResponse",
  "errors.networkUnavailable",
  "errors.expiredAuthorization",
  "errors.authorizationCompleted",
  "errors.notClaimable",
];
function read(tree: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (value, key) =>
        typeof value === "object" && value !== null
          ? (value as Record<string, unknown>)[key]
          : undefined,
      tree,
    );
}
function leaves(tree: object, prefix = ""): string[] {
  return Object.entries(tree).flatMap(([key, value]) =>
    typeof value === "object"
      ? leaves(value, `${prefix}${key}.`)
      : [`${prefix}${key}`],
  );
}

describe("MiniMax Code localization contract", () => {
  it.each([
    ["zh", zh],
    ["zh-TW", zhTW],
    ["en", en],
    ["ja", ja],
  ] as const)(
    "covers all status, safety and device authorization text in %s",
    (_, locale) => {
      expect(leaves(locale.minimaxCode).sort()).toEqual([...required].sort());
      for (const key of required) {
        const value = read(locale.minimaxCode, key);
        expect(typeof value, key).toBe("string");
        expect((value as string).trim().length, key).toBeGreaterThan(0);
        expect((value as string).match(/\{\{\w+\}\}/g) ?? [], key).toEqual(
          (read(zh.minimaxCode, key) as string).match(/\{\{\w+\}\}/g) ?? [],
        );
      }
      expect(locale.settings.authCenter.minimaxCodeDescription).toBeTruthy();
      expect(locale.minimaxCode.status.unknown).not.toBe(
        locale.minimaxCode.status.claimed,
      );
      expect(locale.minimaxCode.status.alreadyClaimed).not.toBe(
        locale.minimaxCode.status.claimed,
      );
    },
  );
});

it.each([
  ["en", en],
  ["ja", ja],
  ["zh", zh],
  ["zh-TW", zhTW],
] as const)(
  "names the supported CLI source paths and explicit recipients in %s",
  (_, locale) => {
    const copy = locale.minimaxCode.nativeImport;
    expect(copy.choose).toContain("auth.json");
    expect(copy.choose).not.toContain("oauth.json");
    expect(copy.description).toContain("prod/cn/mcode-public/auth.json");
    expect(copy.description).toContain("prod/en/mcode-public/auth.json");
    expect(copy.description).not.toContain("oauth.json");
    expect(copy.verifyConsent).toContain("MiniMax");
    expect(copy.saveConsent).toContain("LoongPort");
  },
);
