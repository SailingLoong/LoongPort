import { expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import ja from "@/i18n/locales/ja.json";
import {
  overviewAccountDetailsName,
  nativeWorkspaceAccountName,
} from "../browser/unified-account-locators.mjs";
it.each([
  ["en", en, "View Synthetic account B"],
  ["zh", zh, "查看 Synthetic account B"],
  ["zh-TW", zhTW, "檢視 Synthetic account B"],
  ["ja", ja, "Synthetic account B の詳細"],
])(
  "keeps exact overview Details labels aligned with the real %s locale",
  (_language, translation, expected) => {
    const t = (key) =>
      key === "accountsOverview.viewAccount"
        ? translation.accountsOverview.viewAccount
        : key;
    expect(overviewAccountDetailsName(t, "Synthetic account B")).toBe(expected);
  },
);
it("keeps the original native workspace selection distinct from localized overview Details", () => {
  expect(nativeWorkspaceAccountName("Synthetic account B")).toBe(
    "查看 Synthetic account B",
  );
  expect(
    overviewAccountDetailsName(
      () => en.accountsOverview.viewAccount,
      "Synthetic account B",
    ),
  ).not.toBe(nativeWorkspaceAccountName("Synthetic account B"));
});
