import { expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
function leaves(
  value: Record<string, unknown>,
  prefix = "",
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(value).flatMap(([key, value]) =>
      typeof value === "string"
        ? [[prefix + key, value]]
        : Object.entries(
            leaves(value as Record<string, unknown>, prefix + key + "."),
          ),
    ),
  );
}
it.each([en, zh, ja, zhTW])(
  "provides complete C2 copy without demonstration accounts or invented request success",
  (locale) => {
    const copy = leaves({
      routingWorkspace: locale.routingWorkspace,
      sharedGroups: locale.sharedGroups,
    });
    expect(Object.keys(copy).sort()).toEqual(
      Object.keys(
        leaves({
          routingWorkspace: en.routingWorkspace,
          sharedGroups: en.sharedGroups,
        }),
      ).sort(),
    );
    for (const text of Object.values(copy))
      expect(text.length).toBeGreaterThan(0);
    expect(Object.values(copy).join(" ")).not.toMatch(
      /fixture|MiniMax A|Qoder C|WorkBuddy B|模拟数据|設計提案|设计提案|当前正在响应/,
    );
    expect(locale.client.sharedGroups).toBe(locale.sharedGroups.title);
    expect(locale.routingWorkspace.title).toContain("{{app}}");
  },
);
