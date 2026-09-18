import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import zh from "@/i18n/locales/zh.json";

/** 模型对齐告警横幅的全部 i18n key：四语言必须齐全，漏一个 locale 会直接
 *  显示 key 名（照 codexResetLocales.test.ts 的形状）。 */
const requiredKeys = [
  "modelMismatch.body",
  "modelMismatch.notAvailable",
  "modelMismatch.keep",
  "modelMismatch.adopt",
  "modelMismatch.switchFailed",
];

const locales: Record<string, unknown> = { zh, en, ja, "zh-TW": zhTW };

describe("modelMismatch locales", () => {
  it.each(Object.keys(locales))("%s 覆盖全部 modelMismatch key", (lang) => {
    const dict = locales[lang] as Record<string, unknown>;
    const has = (path: string) =>
      path
        .split(".")
        .reduce<unknown>(
          (acc, k) => (acc as Record<string, unknown>)?.[k],
          dict,
        ) != null;
    for (const key of requiredKeys) {
      expect(has(key), `${lang} 缺少 ${key}`).toBe(true);
    }
  });
});
