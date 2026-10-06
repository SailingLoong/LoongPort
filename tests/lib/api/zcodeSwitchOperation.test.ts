import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const source = {
  installPath: "/synthetic/ZCode.app",
  dataRoot: "/synthetic/data",
  keyMode: "standard" as const,
};
const complete = (requestId: string) => ({
  requestId,
  target: "target",
  phase: "restartVerified",
  refreshed: false,
  restartRequested: true,
});
describe("persistent switch request", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
  });
  it("recovers a lost reply by querying the original ID without repeating effects", async () => {
    let id = "";
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "switch_zcode_saved_account") {
        id = (args as { requestId: string }).requestId;
        throw new Error("lost transport private detail");
      }
      expect(command).toBe("get_zcode_switch_operation");
      expect(args).toMatchObject({ requestId: id });
      return complete(id);
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).resolves.toBe("switched");
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).resolves.toBe("switched");
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "switch_zcode_saved_account",
        ),
    ).toHaveLength(1);
  });
  it("retains unresolved original identity across reopening and refuses a new target", async () => {
    let id = "";
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "switch_zcode_saved_account") {
        id = (args as { requestId: string }).requestId;
        throw { code: "zcode.account.operation_failed" };
      }
      return { ...complete(id), phase: "transactionUncertain" };
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).rejects.toMatchObject({ code: "zcode.account.operation_failed" });
    vi.resetModules();
    const reopened = (await import("@/lib/api/zcodeAccounts")).zcodeAccountsApi;
    await expect(
      reopened.queryLastOperation(source, "fresh-context"),
    ).resolves.toMatchObject({ requestId: id, phase: "transactionUncertain" });
    await expect(
      reopened.switch(source, "fresh-context", "other", "new-catalog"),
    ).rejects.toMatchObject({ code: "zcode.account.operation_already_known" });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "switch_zcode_saved_account",
        ),
    ).toHaveLength(1);
  });
  it("queries without replay and isolates source selections", async () => {
    vi.mocked(invoke).mockResolvedValue("switched");
    await zcodeAccountsApi.switch(source, "context", "target", "catalog");
    vi.mocked(invoke).mockClear();
    await expect(
      zcodeAccountsApi.queryLastOperation(
        { ...source, dataRoot: "/other" },
        "context",
      ),
    ).resolves.toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
  it("keeps an unresolved source A pointer when source B submits and the page reopens", async () => {
    const identities = new Map<string, string>();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      const values = args as { source: typeof source; requestId: string };
      if (command === "switch_zcode_saved_account") {
        identities.set(values.source.dataRoot, values.requestId);
        if (values.source.dataRoot === source.dataRoot) throw {};
        return "switched";
      }
      return {
        ...complete(values.requestId),
        phase:
          values.source.dataRoot === source.dataRoot
            ? "transactionUncertain"
            : "restartVerified",
      };
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).rejects.toMatchObject({ code: "zcode.account.operation_failed" });
    await zcodeAccountsApi.switch(
      { ...source, dataRoot: "/source-B" },
      "context-B",
      "target-B",
      "catalog-B",
    );
    vi.resetModules();
    const reopened = (await import("@/lib/api/zcodeAccounts")).zcodeAccountsApi;
    await expect(
      reopened.queryLastOperation(source, "fresh-context"),
    ).resolves.toMatchObject({
      requestId: identities.get(source.dataRoot),
      phase: "transactionUncertain",
    });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "switch_zcode_saved_account",
        ),
    ).toHaveLength(2);
  });
  it("allocates a fresh ID after Failed even if clearing its old pointer failed", async () => {
    const ids: string[] = [];
    let failed = true;
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      const id = (args as { requestId: string }).requestId;
      if (command === "switch_zcode_saved_account") {
        ids.push(id);
        if (failed) throw {};
        return "switched";
      }
      return { ...complete(id), phase: "failed" };
    });
    const set = Storage.prototype.setItem;
    const writing = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(function (this: Storage, key, value) {
        if (value === "[]") throw new Error("clear denied");
        return set.call(this, key, value);
      });
    try {
      await expect(
        zcodeAccountsApi.switch(source, "context", "target", "catalog"),
      ).rejects.toMatchObject({ code: "zcode.account.operation_failed" });
      failed = false;
      await expect(
        zcodeAccountsApi.switch(source, "context", "target", "catalog"),
      ).resolves.toBe("switched");
      expect(ids).toHaveLength(2);
      expect(ids[1]).not.toBe(ids[0]);
    } finally {
      writing.mockRestore();
    }
  });
  it("preserves committed restart failure as the original outcome", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "switch_zcode_saved_account") throw {};
      return { ...complete("id"), phase: "restartFailed" };
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).rejects.toMatchObject({
      code: "zcode.account.committed_restart_failed",
      committed: true,
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).rejects.toMatchObject({
      code: "zcode.account.committed_restart_failed",
      committed: true,
    });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(
          ([command]) => command === "switch_zcode_saved_account",
        ),
    ).toHaveLength(1);
  });
});
