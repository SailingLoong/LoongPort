import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";

// Routing owns these reason codes; every UI locale must explain them.
const eligibility = readFileSync(
  "src-tauri/src/proxy/application_routing.rs",
  "utf8",
);
const presentation = readFileSync(
  "src-tauri/src/commands/application_routing.rs",
  "utf8",
);
const reasons = new Set([
  ...Array.from(
    eligibility.matchAll(/return Some\("([a-z_]+)"\)/g),
    (match) => match[1],
  ),
  ...Array.from(
    presentation.matchAll(/Some\("([a-z_]+)"\.to_string\(\)\)/g),
    (match) => match[1],
  ),
]);
describe("application routing reason contract", () => {
  it.each([en, ja, zh, zhTW])(
    "translates every backend exclusion",
    (locale) => {
      expect(reasons.size).toBeGreaterThan(0);
      const copy: Record<string, string> = locale.applications.skipReasons;
      for (const reason of reasons)
        expect(copy[reason], reason).toEqual(expect.any(String));
    },
  );
});
