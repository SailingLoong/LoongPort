import { describe, expect, it } from "vitest";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import zh from "@/i18n/locales/zh.json";

type TranslationTree = Record<string, unknown>;

function flattenStrings(
  value: unknown,
  path: string[] = [],
  result = new Map<string, string>(),
): Map<string, string> {
  if (typeof value === "string") {
    result.set(path.join("."), value);
  } else if (typeof value === "object" && value !== null) {
    for (const [key, child] of Object.entries(value)) {
      flattenStrings(child, [...path, key], result);
    }
  }
  return result;
}

function interpolationVariables(value: string): string[] {
  return Array.from(
    value.matchAll(/\{\{\s*([^}]+?)\s*\}\}/g),
    ([, name]) => name,
  ).sort();
}

const reference = flattenStrings(en);
const piKeysOutsideNamespace = new Set([
  "apps.pi",
  "deeplink.api",
  "sessionManager.piDiscoveryUnavailable",
  "sessionManager.piRelativeSessionDir",
  "settings.browsePlaceholderPi",
  "settings.piConfigDir",
  "settings.piConfigDirDescription",
]);
const piReference = new Map(
  [...reference].filter(
    ([key]) => key.startsWith("pi.") || piKeysOutsideNamespace.has(key),
  ),
);
const piProductReferences = new Map(
  [...reference].filter(([, value]) => /\bPi\b/.test(value)),
);
const locales = [
  ["zh", zh],
  ["ja", ja],
  ["zh-TW", zhTW],
] as const;

describe("locale coverage", () => {
  it.each(locales)("covers every Pi translation key in %s", (_name, tree) => {
    const translations = flattenStrings(tree as TranslationTree);
    const missing = [...piReference.keys()].filter(
      (key) => !translations.has(key),
    );

    expect(missing).toEqual([]);
  });

  it.each(locales)(
    "preserves every Pi interpolation variable in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      const mismatched = [...piReference].flatMap(([key, expected]) => {
        const actual = translations.get(key);
        return actual !== undefined &&
          interpolationVariables(actual).join("\0") !==
            interpolationVariables(expected).join("\0")
          ? [key]
          : [];
      });

      expect(mismatched).toEqual([]);
    },
  );

  it.each(locales)(
    "preserves explicit Pi product mentions in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree as TranslationTree);
      const missingMentions = [...piProductReferences.keys()].filter((key) => {
        const actual = translations.get(key);
        return actual === undefined || !/\bPi\b/.test(actual);
      });

      expect(missingMentions).toEqual([]);
    },
  );
});

describe("Gemini shared settings locale coverage", () => {
  const keys = [
    "title",
    "description",
    "manageMcp",
    "envEditor",
    "settingsEditor",
    "preservedEnv",
    "importDescription",
    "invalidFormat",
    "files.title",
    "files.geminiEnv",
    "files.geminiSettings",
    "files.write",
    "files.unchanged",
  ];
  it.each([["en", en], ...locales] as const)(
    "covers both Gemini original formats, preserved values, import, and preview labels in %s",
    (_name, tree) => {
      const translations = flattenStrings(tree);
      for (const key of keys) {
        expect(
          translations.get(`geminiSharedSettings.${key}`)?.trim(),
          key,
        ).toBeTruthy();
      }
      expect(tree.geminiSharedSettings.envEditor).toContain(".env");
      expect(tree.geminiSharedSettings.settingsEditor).toContain(
        "settings.json",
      );
      expect(tree.geminiSharedSettings.files.geminiEnv).toBe(".env");
      expect(tree.geminiSharedSettings.files.geminiSettings).toBe(
        "settings.json",
      );
    },
  );
});
