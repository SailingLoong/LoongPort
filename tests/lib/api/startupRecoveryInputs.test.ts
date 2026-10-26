import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { startupRecoveryApi } from "@/lib/api/startupRecovery";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const base = {
  token: "synthetic-journal",
  status: "pending",
  canRecover: true,
  restartRequired: false,
};
beforeEach(() => {
  vi.mocked(invoke).mockReset();
});
it.each([
  { inputRequired: false, inputKind: "none" },
  { inputRequired: true, inputKind: "source" },
  { inputRequired: true, inputKind: "current" },
])(
  "retains the native input authority and strips unrelated fields (%j)",
  async (input) => {
    vi.mocked(invoke).mockResolvedValue({
      ...base,
      ...input,
      password: "synthetic-private-extra",
    });
    expect(await startupRecoveryApi.status()).toEqual({ ...base, ...input });
  },
);
it("preserves the original source-input contract only when both fields are absent", async () => {
  vi.mocked(invoke).mockResolvedValue(base);
  expect(await startupRecoveryApi.status()).toEqual({
    ...base,
    inputRequired: true,
    inputKind: "source",
  });
});
it.each([
  { inputRequired: false },
  { inputKind: "none" },
  { inputRequired: true, inputKind: "none" },
  { inputRequired: false, inputKind: "source" },
  { inputRequired: false, inputKind: "current" },
  { inputRequired: "false", inputKind: "none" },
  { inputRequired: true, inputKind: "future" },
  { inputRequired: null, inputKind: null },
  { inputRequired: true, inputKind: "source", canRecover: false },
  {
    inputRequired: true,
    inputKind: "current",
    status: "completed",
    canRecover: false,
    restartRequired: true,
  },
])(
  "rejects partial, contradictory or nonrecoverable input authority (%j)",
  async (input) => {
    vi.mocked(invoke).mockResolvedValue({ ...base, ...input });
    await expect(startupRecoveryApi.status()).rejects.toThrow(
      "startupRecovery.statusFailed",
    );
  },
);
it.each(["", " \r\n中文🔑e\u0301 MiXeD \r "])(
  "keeps the original token/password wire byte-for-byte (%j)",
  async (password) => {
    const completed = {
      ...base,
      status: "completed",
      canRecover: false,
      restartRequired: true,
      inputRequired: false,
      inputKind: "none",
    };
    vi.mocked(invoke).mockResolvedValue(completed);
    expect(await startupRecoveryApi.recover(base.token, password)).toEqual(
      completed,
    );
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "recover_startup_operation",
      { token: base.token, password },
    );
  },
);
it("does not accept a resolved malformed input contract as successful recovery", async () => {
  vi.mocked(invoke).mockResolvedValue({
    ...base,
    status: "completed",
    canRecover: false,
    restartRequired: true,
    inputRequired: true,
    inputKind: "none",
  });
  await expect(startupRecoveryApi.recover(base.token, "")).rejects.toThrow(
    "startupRecovery.invalidStatus",
  );
});
it("preserves the structured native rejection for the existing consumer without reissuing", async () => {
  const rejected = {
    code: "secret.input_not_required",
    restartRequired: false,
  };
  vi.mocked(invoke).mockRejectedValue(rejected);
  await expect(
    startupRecoveryApi.recover(base.token, "unexpected"),
  ).rejects.toEqual(rejected);
  expect(invoke).toHaveBeenCalledTimes(1);
});
