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

describe("readonly original-operation metadata", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
  });
  it("returns verified failed evidence from the same request after forgetting its pointer", async () => {
    let requestId = "";
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "switch_zcode_saved_account") {
        requestId = (args as { requestId: string }).requestId;
        throw {
          code: "zcode.account.operation_failed",
          remedy: "refreshContext",
          committed: false,
          secret: "hidden-payload",
        };
      }
      return {
        ...complete(requestId),
        phase: "failed",
        secret: "hidden-status",
      };
    });
    const failure = await zcodeAccountsApi
      .switch(source, "context", "target", "catalog")
      .catch((value) => value);
    expect(failure).toMatchObject({
      source,
      contextRevision: "context",
      operation: { requestId, target: "target", phase: "failed" },
    });
    expect(JSON.stringify(failure)).not.toContain("hidden-");
    expect(
      await zcodeAccountsApi.queryLastOperationState(source, "context"),
    ).toEqual({ operation: null, request: null });
  });
  it.each(["request", "target"])(
    "does not certify or forget mismatched failed %s evidence",
    async (mismatch) => {
      let requestId = "";
      vi.mocked(invoke).mockImplementation(async (command, args) => {
        if (command === "switch_zcode_saved_account") {
          requestId = (args as { requestId: string }).requestId;
          throw {};
        }
        return {
          ...complete(mismatch === "request" ? "wrong-request" : requestId),
          target: mismatch === "target" ? "wrong-target" : "target",
          phase: "failed",
        };
      });
      const failure = await zcodeAccountsApi
        .switch(source, "context", "target", "catalog")
        .catch((value) => value);
      expect(failure).not.toHaveProperty("operation");
      vi.mocked(invoke).mockResolvedValue(null);
      expect(
        await zcodeAccountsApi.queryLastOperationState(source, "context"),
      ).toMatchObject({
        operation: null,
        request: { requestId, target: "target", contextRevision: "context" },
      });
    },
  );
  it("exposes an unresolved pointer with a missing record across reopening, without replay or cross-source leakage", async () => {
    let requestId = "";
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "switch_zcode_saved_account") {
        requestId = (args as { requestId: string }).requestId;
        throw {};
      }
      return null;
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog"),
    ).rejects.toMatchObject({ code: "zcode.account.operation_failed" });
    vi.resetModules();
    const reopened = (await import("@/lib/api/zcodeAccounts")).zcodeAccountsApi;
    expect(
      await reopened.queryLastOperationState(source, "fresh-context"),
    ).toEqual({
      operation: null,
      request: { requestId, target: "target", contextRevision: "context" },
    });
    expect(
      await reopened.queryLastOperationState(
        { ...source, dataRoot: "/other" },
        "fresh-context",
      ),
    ).toEqual({ operation: null, request: null });
    expect(
      vi
        .mocked(invoke)
        .mock.calls.filter(([cmd]) => cmd === "switch_zcode_saved_account"),
    ).toHaveLength(1);
  });
  it("does not certify restartFailed as a retryable failed write", async () => {
    let requestId = "";
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "switch_zcode_saved_account") {
        requestId = (args as { requestId: string }).requestId;
        throw {};
      }
      return { ...complete(requestId), phase: "restartFailed" };
    });
    const failure = await zcodeAccountsApi
      .switch(source, "context", "target", "catalog")
      .catch((value) => value);
    expect(failure).not.toHaveProperty("operation");
    expect(
      await zcodeAccountsApi.queryLastOperationState(source, "context"),
    ).toMatchObject({
      operation: { phase: "restartFailed" },
      request: { requestId },
    });
  });
});

it.each(["request", "target"])(
  "does not return a mismatched %s query result as terminal evidence",
  async (mismatch) => {
    localStorage.clear();
    vi.mocked(invoke).mockReset().mockResolvedValueOnce("switched");
    await zcodeAccountsApi.switch(source, "context", "target", "catalog");
    const args = vi.mocked(invoke).mock.calls[0][1] as { requestId: string };
    vi.mocked(invoke).mockResolvedValue({
      ...complete(mismatch === "request" ? "wrong" : args.requestId),
      target: mismatch === "target" ? "wrong" : "target",
    });
    expect(
      await zcodeAccountsApi.queryLastOperationState(source, "context"),
    ).toMatchObject({
      operation: null,
      request: { requestId: args.requestId, target: "target" },
    });
  },
);

describe("original-operation query failures", () => {
  const pointer = {
    requestId: "original-A",
    sourceKey: JSON.stringify([
      source.installPath,
      source.dataRoot,
      source.keyMode,
    ]),
    contextRevision: "original-context",
    target: "target",
    catalogRevision: "private-catalog",
    secret: "private-pointer-payload",
  };
  beforeEach(() => {
    localStorage.clear();
    localStorage.setItem(
      "loongport:zcode-switch-request-v1",
      JSON.stringify([pointer]),
    );
    vi.mocked(invoke).mockReset();
  });
  it("preserves the same allowlisted request snapshot on native query failure", async () => {
    vi.mocked(invoke).mockImplementation(async () => {
      localStorage.setItem(
        "loongport:zcode-switch-request-v1",
        JSON.stringify([{ ...pointer, requestId: "replacement-B" }]),
      );
      throw {
        code: "zcode.account.operation_failed",
        remedy: "queryOriginal",
        committed: false,
        message: "private-message",
        stack: "private-stack",
        token: "private-token",
      };
    });
    await expect(
      zcodeAccountsApi.queryLastOperationState(source, "fresh-context"),
    ).resolves.toEqual({
      operation: null,
      request: {
        requestId: "original-A",
        target: "target",
        contextRevision: "original-context",
      },
      error: {
        code: "zcode.account.operation_failed",
        remedy: "queryOriginal",
        committed: false,
      },
    });
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "get_zcode_switch_operation",
      { source, contextRevision: "fresh-context", requestId: "original-A" },
    );
  });
  it("preserves legacy query rejection with only sanitized error fields", async () => {
    vi.mocked(invoke).mockRejectedValue({
      code: "zcode.account.operation_failed",
      remedy: "queryOriginal",
      committed: false,
      message: "private-message",
    });
    await expect(
      zcodeAccountsApi.queryLastOperation(source, "fresh-context"),
    ).rejects.toEqual({
      code: "zcode.account.operation_failed",
      remedy: "queryOriginal",
      committed: false,
    });
    expect(invoke).toHaveBeenCalledTimes(1);
  });
  it("reports unreadable pointer storage without inventing a request or terminal operation", async () => {
    localStorage.setItem("loongport:zcode-switch-request-v1", "{");
    await expect(
      zcodeAccountsApi.queryLastOperationState(source, "fresh-context"),
    ).resolves.toEqual({
      operation: null,
      request: null,
      error: {
        code: "zcode.account.storage_failed",
        remedy: "checkLocalStorage",
        committed: false,
      },
    });
    await expect(
      zcodeAccountsApi.queryLastOperation(source, "fresh-context"),
    ).rejects.toEqual({
      code: "zcode.account.storage_failed",
      remedy: "checkLocalStorage",
      committed: false,
    });
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe("switch invocation request evidence", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
  });
  it.each(["success", "unknown", "failed"] as const)(
    "reports immutable request identity before native dispatch for %s",
    async (outcome) => {
      const evidence: unknown[] = [];
      let id = "";
      vi.mocked(invoke).mockImplementation(async (command, args) => {
        id = (args as { requestId: string }).requestId;
        expect(evidence).toHaveLength(1);
        expect(evidence[0]).toEqual({
          source,
          contextRevision: "context",
          requestId: id,
          target: "target",
        });
        expect(Object.isFrozen(evidence[0])).toBe(true);
        expect(
          Object.isFrozen((evidence[0] as { source: unknown }).source),
        ).toBe(true);
        if (command === "switch_zcode_saved_account") {
          if (outcome === "success") return "switched";
          throw {};
        }
        return outcome === "failed"
          ? { ...complete(id), phase: "failed" }
          : null;
      });
      const promise = zcodeAccountsApi.switch(
        source,
        "context",
        "target",
        "catalog",
        (value) => {
          evidence.push(value);
        },
      );
      if (outcome === "success")
        await expect(promise).resolves.toBe("switched");
      else
        await expect(promise).rejects.toMatchObject({
          code: "zcode.account.operation_failed",
        });
      expect(evidence).toHaveLength(1);
      expect(
        vi
          .mocked(invoke)
          .mock.calls.filter(
            ([command]) => command === "switch_zcode_saved_account",
          ),
      ).toHaveLength(1);
    },
  );
  it("reports an existing request before preflight can fail or storage can change", async () => {
    localStorage.setItem(
      "loongport:zcode-switch-request-v1",
      JSON.stringify([
        {
          requestId: "previous-A",
          target: "old-target",
          sourceKey: JSON.stringify([
            source.installPath,
            source.dataRoot,
            source.keyMode,
          ]),
          contextRevision: "old-context",
          catalogRevision: "old-catalog",
        },
      ]),
    );
    const observer = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command) => {
      expect(command).toBe("get_zcode_switch_operation");
      expect(observer).toHaveBeenCalledExactlyOnceWith({
        source,
        contextRevision: "context",
        requestId: "previous-A",
        target: "old-target",
      });
      localStorage.setItem("loongport:zcode-switch-request-v1", "[]");
      throw {};
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog", observer),
    ).rejects.toMatchObject({ code: "zcode.account.operation_failed" });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(observer).toHaveBeenCalledExactlyOnceWith({
      source,
      contextRevision: "context",
      requestId: "previous-A",
      target: "old-target",
    });
  });
  it("reports previous then new identity only after the existing wrapper permits allocation", async () => {
    localStorage.setItem(
      "loongport:zcode-switch-request-v1",
      JSON.stringify([
        {
          requestId: "previous-A",
          target: "target",
          sourceKey: JSON.stringify([
            source.installPath,
            source.dataRoot,
            source.keyMode,
          ]),
          contextRevision: "context",
          catalogRevision: "catalog",
        },
      ]),
    );
    const observer = vi.fn();
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "get_zcode_switch_operation")
        return { ...complete("previous-A"), phase: "failed" };
      expect(observer).toHaveBeenCalledTimes(2);
      expect(observer.mock.calls[1][0].requestId).toBe(
        (args as { requestId: string }).requestId,
      );
      expect(observer.mock.calls[1][0].requestId).not.toBe("previous-A");
      return "switched";
    });
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog", observer),
    ).resolves.toBe("switched");
    expect(invoke).toHaveBeenCalledTimes(2);
  });
  it("does not let an observer exception change legacy native behavior", async () => {
    vi.mocked(invoke).mockResolvedValue("switched");
    await expect(
      zcodeAccountsApi.switch(source, "context", "target", "catalog", () => {
        throw new Error("observer failed");
      }),
    ).resolves.toBe("switched");
    expect(invoke).toHaveBeenCalledTimes(1);
  });
});

it.each(["replaced", "deleted"] as const)(
  "queries captured request A instead of a %s latest pointer",
  async (change) => {
    localStorage.clear();
    localStorage.setItem(
      "loongport:zcode-switch-request-v1",
      JSON.stringify(
        change === "deleted"
          ? []
          : [
              {
                requestId: "unrelated-B",
                target: "target",
                sourceKey: JSON.stringify([
                  source.installPath,
                  source.dataRoot,
                  source.keyMode,
                ]),
                contextRevision: "context",
                catalogRevision: "catalog",
              },
            ],
      ),
    );
    vi.mocked(invoke).mockReset().mockResolvedValue(complete("captured-A"));
    await expect(
      zcodeAccountsApi.queryLastOperationState(source, "context", {
        requestId: "captured-A",
        target: "target",
      }),
    ).resolves.toEqual({
      operation: complete("captured-A"),
      request: {
        requestId: "captured-A",
        target: "target",
        contextRevision: "context",
      },
    });
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "get_zcode_switch_operation",
      { source, contextRevision: "context", requestId: "captured-A" },
    );
  },
);
