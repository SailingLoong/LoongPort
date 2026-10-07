import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { zcodeLoginApi, type LoginProgress } from "@/lib/api/zcodeLogin";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const progress: LoginProgress = {
  flowId: "synthetic-flow",
  phase: "waiting",
  family: "bigmodel",
  authorization: {
    url: "https://example.test/authorize",
    expiresAt: 2_000_000_000,
    pollIntervalSec: 5,
  },
  account: null,
  connections: null,
  project: null,
  keyCreated: false,
  keyMayExist: false,
  keyManagementUrl: "https://example.test/keys",
  error: null,
  saved: null,
};

describe("ZCode official login IPC", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("uses the six backend operations and preserves their returned progress", async () => {
    vi.mocked(invoke).mockResolvedValue(progress);
    const results = [
      await zcodeLoginApi.begin("zai"),
      await zcodeLoginApi.progress("synthetic-flow"),
      await zcodeLoginApi.confirmKey("synthetic-flow", "org", "project"),
      await zcodeLoginApi.declineKey("synthetic-flow"),
      await zcodeLoginApi.save("synthetic-flow", false),
      await zcodeLoginApi.cancel("synthetic-flow"),
    ];
    expect(results).toEqual(Array(6).fill(progress));
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["begin_zcode_official_login", { family: "zai" }],
      ["get_zcode_login_progress", { flowId: "synthetic-flow" }],
      [
        "confirm_zcode_login_key",
        {
          flowId: "synthetic-flow",
          organizationId: "org",
          projectId: "project",
        },
      ],
      ["decline_zcode_login_key", { flowId: "synthetic-flow" }],
      [
        "save_zcode_login_account",
        { flowId: "synthetic-flow", updateDuplicate: false },
      ],
      ["cancel_zcode_official_login", { flowId: "synthetic-flow" }],
    ]);
  });

  it("projects only the public progress fields at every boundary", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...progress,
      token: "private-raw-token",
      authorization: { ...progress.authorization, pollToken: "private-poll" },
      account: {
        id: "opaque",
        label: "a…",
        duplicate: false,
        identitySource: "officialLogin",
        businessToken: "private-business",
      },
      connections: {
        start: "ready",
        coding: "ready",
        needsKey: false,
        jwt: "private-jwt",
        key: "private-key",
      },
      project: {
        organizationId: "org",
        organizationName: null,
        projectId: "project",
        projectName: null,
        key: "private-project-key",
      },
      saved: { id: "opaque", outcome: "saved", credential: "private-saved" },
      error: {
        code: "raw-private-code",
        remedy: "raw-private-remedy",
        committed: true,
        raw: "private-error",
      },
    });
    const result = await zcodeLoginApi.progress("synthetic-flow");
    expect(JSON.stringify(result)).not.toContain("private");
    expect(result.error).toEqual({
      code: "zcode.account.operation_failed",
      remedy: "queryOriginal",
      committed: true,
    });
    expect(result.account?.label).toBe("a…");
    expect(result.saved).toEqual({ id: "opaque", outcome: "saved" });
  });

  it("passes an optional library data root without adding undefined fields", async () => {
    vi.mocked(invoke).mockResolvedValue(progress);
    await zcodeLoginApi.begin("bigmodel", "/synthetic/account-library");
    expect(invoke).toHaveBeenLastCalledWith("begin_zcode_official_login", {
      family: "bigmodel",
      dataRoot: "/synthetic/account-library",
    });
    await zcodeLoginApi.begin("zai", undefined);
    expect(invoke).toHaveBeenLastCalledWith("begin_zcode_official_login", {
      family: "zai",
    });
  });

  it("does not replay a lost save or Key response and exposes only safe errors", async () => {
    vi.mocked(invoke).mockRejectedValue(new Error("private response"));
    await expect(zcodeLoginApi.save("synthetic-flow", true)).rejects.toEqual({
      code: "zcode.account.operation_failed",
      remedy: "queryOriginal",
      committed: false,
    });
    await expect(
      zcodeLoginApi.confirmKey("synthetic-flow", "org", "project"),
    ).rejects.toEqual({
      code: "zcode.account.operation_failed",
      remedy: "queryOriginal",
      committed: false,
    });
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it.each([
    "retryKeyConsent",
    "retrySave",
    "queryOriginal",
    "refreshContext",
    "private-remedy",
  ])("only retains explicit backend retry remedies (%s)", async (remedy) => {
    vi.mocked(invoke).mockResolvedValue({
      ...progress,
      error: { code: "zcode.account.storage_failed", remedy, committed: false },
    });
    const actual = await zcodeLoginApi.progress("synthetic-flow");
    expect(actual.error?.remedy).toBe(
      ["retryKeyConsent", "retrySave"].includes(remedy)
        ? remedy
        : "queryOriginal",
    );
  });

  it("never treats an IPC rejection as permission to repeat a write", async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: "zcode.account.storage_failed",
      remedy: "retrySave",
      committed: false,
    });
    await expect(
      zcodeLoginApi.save("synthetic-flow", false),
    ).rejects.toMatchObject({ remedy: "queryOriginal" });
  });
});

it("preserves bounded original-result errors without retaining raw backend details", async () => {
  vi.mocked(invoke).mockResolvedValue({
    ...progress,
    phase: "cancelled",
    error: {
      code: "zcode.account.save_result_unknown",
      remedy: "queryOriginal",
      committed: false,
      detail: "secret-canary",
    },
  });
  const result = await zcodeLoginApi.progress("synthetic-flow");
  expect(result.error).toEqual({
    code: "zcode.account.save_result_unknown",
    remedy: "queryOriginal",
    committed: false,
  });
  expect(JSON.stringify(result)).not.toContain("secret-canary");
});
