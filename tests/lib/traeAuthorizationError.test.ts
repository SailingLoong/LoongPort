import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, it } from "vitest";
import {
  TRAE_AUTHORIZATION_ERROR_CODES,
  TRAE_AUTHORIZATION_FAILURE_MESSAGES,
  traeAuthorizationFailure,
} from "@/lib/traeAuthorizationError";

it("pins the safe diagnostic allowlist to the serialized native entry error enum", () => {
  const rust = readFileSync(
    resolve(process.cwd(), "src-tauri/src/trae_accounts/entry.rs"),
    "utf8",
  );
  const declared = rust.match(
    /#\[serde\(rename_all = "camelCase"\)\]\s*pub\(crate\) enum Error \{([^}]+)\}/,
  );
  expect(declared).not.toBeNull();
  const codes = declared![1]
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean)
    .map((value) => value[0].toLowerCase() + value.slice(1));
  expect([...TRAE_AUTHORIZATION_ERROR_CODES]).toEqual(codes);
});

it.each(TRAE_AUTHORIZATION_ERROR_CODES)(
  "maps only the exact native code %s to fixed Chinese copy",
  (code) => {
    expect(traeAuthorizationFailure(code)).toBe(code);
    expect(TRAE_AUTHORIZATION_FAILURE_MESSAGES[code]).toMatch(
      /[\u4e00-\u9fff]/,
    );
    expect(traeAuthorizationFailure(` ${code}`)).toBe("unknown");
    expect(traeAuthorizationFailure(`${code}: SYNTHETIC_SECRET`)).toBe(
      "unknown",
    );
    expect(traeAuthorizationFailure({ code })).toBe("unknown");
  },
);

it("never reads error-object properties or invokes their string conversion", () => {
  const error = {
    get message(): string {
      throw new Error("must not read native messages");
    },
    toString(): string {
      throw new Error("must not stringify native errors");
    },
  };
  expect(traeAuthorizationFailure(error)).toBe("unknown");
  expect(traeAuthorizationFailure("receiptUnknown")).toBe("unknown");
  expect(traeAuthorizationFailure("__proto__")).toBe("unknown");
});

it("does not narrow the aggregate authorization failure into an identity diagnosis", () => {
  expect(
    TRAE_AUTHORIZATION_FAILURE_MESSAGES.authorizationUnavailable,
  ).toContain("现有信息不足以判断具体原因");
  expect(
    TRAE_AUTHORIZATION_FAILURE_MESSAGES.authorizationUnavailable,
  ).toContain("不能据此认定账号不同");
});
