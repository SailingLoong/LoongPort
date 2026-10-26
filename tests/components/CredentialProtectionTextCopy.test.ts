import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";

describe.each([en, zh, ja, zhTW])("literal key text copy", (locale) => {
  it("has no retired hex/byte format or strength requirement in the key-input copy", () => {
    const copy = locale.storageProtection;
    expect(
      copy.keyLabel + copy.keyFormat + copy.recovery.binaryNotice,
    ).not.toMatch(/64|32|hex|十六|16進/i);
    expect(copy.keyFormat).not.toMatch(
      /random|随机|隨機|ランダム|strong|strength|强度|強度/i,
    );
    expect(copy).not.toHaveProperty("weakKey");
    expect(copy).not.toHaveProperty("keyStrength");
  });
});
