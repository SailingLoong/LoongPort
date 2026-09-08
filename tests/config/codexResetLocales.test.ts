import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import zh from "@/i18n/locales/zh.json";

/** Codex 全局重置预告（额度面板下方一行）：四语言必须齐全，漏一个 locale
 *  会直接显示 key 名（照 xaiOauthLocales.test.ts 的形状）。 */
const requiredKeys = [
  "loongport.codexReset.label",
  "loongport.codexReset.eta",
  "loongport.codexReset.etaTitle",
  "loongport.codexReset.lastReset",
  "loongport.codexReset.lastResetTitle",
  "loongport.codexReset.announcement",
  "loongport.codexReset.source",
];

const locales: Record<string, unknown> = { zh, en, ja, "zh-TW": zhTW };

describe("codexReset locales", () => {
  it.each(Object.keys(locales))("%s 覆盖全部 codexReset key", (lang) => {
    const dict = locales[lang] as Record<string, unknown>;
    const has = (path: string) =>
      path
        .split(".")
        .reduce<unknown>(
          (acc, k) => (acc as Record<string, unknown>)?.[k],
          dict,
        ) != null;
    for (const key of requiredKeys) {
      expect(has(key), `${lang} 缺 ${key}`).toBe(true);
    }
  });
});
