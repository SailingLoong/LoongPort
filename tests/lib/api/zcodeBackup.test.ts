import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { zcodeBackupApi } from "@/lib/api/zcodeBackup";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const request = {
  requestId: "10000000-0000-4000-8000-000000000001",
  catalogRevision: "catalog-one",
  profileIds: ["account-one"],
  destination: "/synthetic/backup.zsb",
  password: "  synthetic password  ",
  passwordConfirmation: "  synthetic password  ",
};
const saved = {
  requestId: request.requestId,
  status: "saved",
  destination: request.destination,
  count: 1,
  error: null,
};

describe("ZCode backup IPC boundary", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset().mockResolvedValue(saved);
  });

  it("projects the exact export request and preserves password bytes", async () => {
    const result = await zcodeBackupApi.exportBundle({
      ...request,
      dataRoot: "/synthetic/library",
      ...{ privateField: "secret-canary" },
    });
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "export_zcode_account_bundle",
      { input: { ...request, dataRoot: "/synthetic/library" } },
    );
    expect(result).toEqual(saved);
  });

  it("omits an unspecified data root", async () => {
    await zcodeBackupApi.exportBundle(request);
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "export_zcode_account_bundle",
      { input: request },
    );
  });

  it("queries only the original request ID and strips private result fields", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...saved,
      password: "secret-canary",
      credentials: { accessToken: "secret-canary" },
    });
    expect(await zcodeBackupApi.result(request.requestId)).toEqual(saved);
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "get_zcode_bundle_export_result",
      { requestId: request.requestId },
    );
  });

  it.each(["working", "failed", "unknown"])(
    "never exposes an unverified filename or count for %s",
    async (status) => {
      vi.mocked(invoke).mockResolvedValue({
        ...saved,
        status,
        destination: "/private/unverified.zsb",
        count: 20,
      });
      expect(await zcodeBackupApi.result(request.requestId)).toEqual({
        ...saved,
        status,
        destination: null,
        count: null,
      });
    },
  );

  it("projects safe errors and never turns a lost reply into another write", async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: "zcode.account.storage_failed",
      remedy: "retrySave",
      committed: false,
      message: "secret-canary",
    });
    await expect(zcodeBackupApi.exportBundle(request)).rejects.toEqual({
      code: "zcode.account.storage_failed",
      remedy: "queryOriginal",
      committed: false,
    });
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("strips raw errors returned inside the status DTO", async () => {
    vi.mocked(invoke).mockResolvedValue({
      ...saved,
      status: "failed",
      error: {
        code: "zcode.account.storage_failed",
        remedy: "retrySave",
        committed: false,
        detail: "secret-canary",
      },
    });
    expect(await zcodeBackupApi.result(request.requestId)).toEqual({
      requestId: request.requestId,
      status: "failed",
      destination: null,
      count: null,
      error: {
        code: "zcode.account.storage_failed",
        remedy: "queryOriginal",
        committed: false,
      },
    });
  });

  it.each([
    { requestId: "another-request" },
    { status: "done" },
    { destination: null },
    { count: 0 },
    { count: 51 },
    { count: 1.5 },
    { error: { code: "zcode.account.storage_failed" } },
  ])("rejects inconsistent completion evidence: %j", async (change) => {
    vi.mocked(invoke).mockResolvedValue({ ...saved, ...change });
    await expect(
      zcodeBackupApi.result(request.requestId),
    ).rejects.toMatchObject({
      code: "zcode.account.saved_data_invalid",
      remedy: "queryOriginal",
    });
  });
});
