import { expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import ja from "@/i18n/locales/ja.json";
const paths = [
  "title",
  "loading",
  "unavailable",
  "reload",
  "waiting",
  "checking",
  "lastCheckedAt",
  "nextCheckAt",
  "receiptNotice",
  "result.completed",
  "result.failed",
  "result.skipped",
  "reason.locked",
  "reason.disabled",
  "reason.pending",
  "reason.notEligible",
  "reason.checkFailed",
  "enable",
  "disable",
  "enabled",
  "disabled",
  "notice",
  "actionRequired",
];
const inferencePaths = [
  "authorization",
  "authorize",
  "availableModels",
  "cancel",
  "failed",
  "finish",
  "identityNotice",
  "models",
  "open",
  "state.needsAuthorization",
  "state.ready",
  "state.refreshRequired",
  "state.expired",
];
function at(tree: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce(
      (value: unknown, key) =>
        typeof value === "object" && value !== null
          ? (value as Record<string, unknown>)[key]
          : undefined,
      tree,
    );
}
it.each(Object.entries({ en, zh, "zh-TW": zhTW, ja }))(
  "provides automatic status and WorkBuddy inference translations in %s",
  (_name, locale) => {
    for (const key of paths)
      expect(at(locale, `automaticStatus.${key}`), key).toEqual(
        expect.any(String),
      );
    for (const key of inferencePaths)
      expect(at(locale, `workbuddy.inference.${key}`), key).toEqual(
        expect.any(String),
      );
    expect(at(locale, "automaticStatus.lastCheckedAt")).toContain("{{time}}");
    expect(at(locale, "automaticStatus.nextCheckAt")).toContain("{{time}}");
  },
);
it("distinguishes Chinese automatic checks from claims and inference authorization", () => {
  expect(at(zh, "automaticStatus.notice")).toContain("15 分钟检查");
  expect(at(zh, "automaticStatus.receiptNotice")).toContain(
    "检查完成不等于领取成功",
  );
  expect(at(zh, "workbuddy.inference.identityNotice")).toContain(
    "保留原有签到登录",
  );
});
