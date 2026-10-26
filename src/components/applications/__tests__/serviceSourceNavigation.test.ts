import { expect, it } from "vitest";
import { serviceSourceDestination } from "../serviceSourceNavigation";
it("retains original numeric relay/vendor routes", () => {
  expect(serviceSourceDestination({ kind: "relay", id: 7 }, "codex")).toEqual({
    view: "services",
    app: "codex",
    account: { kind: "relay", id: 7 },
  });
});
it("keeps a local provider origin and platform without inventing a numeric account", () => {
  expect(
    serviceSourceDestination(
      {
        kind: "localProvider",
        platform: "openrouter",
        id: "string",
        originApp: "gemini",
        originProviderId: "original",
      },
      "codex",
    ),
  ).toEqual({ view: "services", app: "gemini", platform: "openrouter" });
});
it.each([
  [
    {
      kind: "nativeAccount",
      platform: "minimax_code",
      id: "native",
      region: "global",
    },
    "minimax_code_global",
  ],
  [
    {
      kind: "nativeAccount",
      platform: "minimax_code",
      id: "native",
      region: "cn",
    },
    "minimax_code",
  ],
  [{ kind: "nativeAccount", platform: "workbuddy", id: "native" }, "workbuddy"],
  [{ kind: "nativeAccount", platform: "qoder_sdk", id: "native" }, "qoder"],
  [{ kind: "nativeAccount", platform: "qoder_cn", id: "native" }, "qoder"],
  [
    { kind: "nativeAccount", platform: "zcode_start_plan", id: "native" },
    "zcode",
  ],
] as const)("uses the original platform route for %j", (account, platform) => {
  expect(serviceSourceDestination(account, "codex")).toEqual({
    view: "services",
    app: "codex",
    platform,
  });
});
