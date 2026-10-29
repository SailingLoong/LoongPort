import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { workbuddyApi } from "@/lib/api/workbuddy";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const selection = {
  id: "synthetic-native",
  generation: 3,
  incarnation: "d54c6503-ae85-4ec5-b591-6393e9d41930",
};
const account = { selection, label: "Synthetic", realm: "www.workbuddy.cn" };
const saved = { state: "saved", account, activationAvailable: false };
beforeEach(() => {
  vi.mocked(invoke).mockReset();
});
it("sends explicit null or a complete selection, once, without touching preview", async () => {
  vi.mocked(invoke).mockResolvedValue(saved);
  expect(
    await workbuddyApi.captureNative({ label: "Synthetic", expected: null }),
  ).toEqual(saved);
  await workbuddyApi.captureNative({ label: "Synthetic", expected: selection });
  await workbuddyApi.captureNative({ label: "Synthetic" });
  expect(vi.mocked(invoke).mock.calls).toEqual([
    [
      "capture_workbuddy_native_account",
      { input: { label: "Synthetic", expected: null } },
    ],
    [
      "capture_workbuddy_native_account",
      { input: { label: "Synthetic", expected: selection } },
    ],
    [
      "capture_workbuddy_native_account",
      { input: { label: "Synthetic", expected: null } },
    ],
  ]);
});
it.each([
  {},
  { label: 12 },
  { label: "x", expected: { id: "x", generation: 1 } },
  { label: "x", expected: { ...selection, incarnation: "bad" } },
  { label: "x", expected: { ...selection, generation: -1 } },
  { label: "x", token: "secret" },
])("rejects invalid input before IPC %#", async (input) => {
  await expect(workbuddyApi.captureNative(input as any)).rejects.toMatchObject({
    code: "invalidInput",
  });
  expect(invoke).not.toHaveBeenCalled();
});
it.each([
  null,
  {},
  { ...saved, state: "waiting" },
  { ...saved, activationAvailable: true },
  { ...saved, account: { ...account, selection: { id: "x", generation: 1 } } },
  { ...saved, account: { ...account, label: null } },
  { ...saved, account: { ...account, realm: null } },
  {
    ...saved,
    account: {
      ...account,
      selection: { ...selection, generation: Number.MAX_SAFE_INTEGER + 1 },
    },
  },
])("fails closed on invalid capture DTO %#", async (value) => {
  vi.mocked(invoke).mockResolvedValue(value);
  await expect(
    workbuddyApi.captureNative({ label: "x" }),
  ).rejects.toMatchObject({ code: "invalidResponse" });
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("copies only reviewed fields at every reply level", async () => {
  vi.mocked(invoke).mockResolvedValue({
    ...saved,
    token: "secret",
    account: {
      ...account,
      snapshot: "secret",
      selection: { ...selection, token: "secret" },
    },
  });
  const result = await workbuddyApi.captureNative({ label: "Synthetic" });
  expect(result).toEqual(saved);
  expect(JSON.stringify(result)).not.toContain("secret");
});
it.each([
  "unsafePath",
  "io",
  "journal",
  "sourceChanged",
  "recoveryRequired",
  "requestConflict",
  "rolledBack",
])("preserves fixed refusal %s without replay", async (code) => {
  vi.mocked(invoke).mockRejectedValue(code);
  await expect(
    workbuddyApi.captureNative({ label: "x" }),
  ).rejects.toMatchObject({ code });
  expect(invoke).toHaveBeenCalledTimes(1);
});
it("redacts arbitrary rejection material", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("secret /private/path"));
  const result = await workbuddyApi
    .captureNative({ label: "x" })
    .catch((e) => e);
  expect(result.code).toBe("unknown");
  expect(result.message).toBe("unknown");
  expect(result.cause).toBeUndefined();
});
it("lists a snapshot without claiming a lost capture request completed", async () => {
  vi.mocked(invoke).mockResolvedValue({
    accounts: [account],
    requestOutcome: "unknown",
    activationAvailable: false,
    token: "secret",
  });
  expect(await workbuddyApi.listNativeCaptures()).toEqual({
    accounts: [account],
    requestOutcome: "unknown",
    activationAvailable: false,
  });
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["list_workbuddy_native_captures"],
  ]);
});
it.each([
  { accounts: [], requestOutcome: "saved", activationAvailable: false },
  { accounts: [{}], requestOutcome: "unknown", activationAvailable: false },
  { accounts: [], requestOutcome: "unknown", activationAvailable: true },
])("rejects invalid list DTO %#", async (value) => {
  vi.mocked(invoke).mockResolvedValue(value);
  await expect(workbuddyApi.listNativeCaptures()).rejects.toMatchObject({
    code: "invalidResponse",
  });
});
it("cancels waiting, observes a late worker commit only through an unknown-outcome list, and never replays", async () => {
  let finish!: (v: unknown) => void;
  vi.mocked(invoke).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const controller = new AbortController();
  const waiting = workbuddyApi
    .captureNative({ label: "Synthetic" }, controller.signal)
    .catch((e) => e);
  controller.abort();
  expect(await waiting).toMatchObject({ code: "cancelled" });
  finish(saved);
  await Promise.resolve();
  vi.mocked(invoke).mockResolvedValue({
    accounts: [account],
    requestOutcome: "unknown",
    activationAvailable: false,
  });
  const snapshot = await workbuddyApi.listNativeCaptures();
  expect(snapshot.requestOutcome).toBe("unknown");
  expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
    "capture_workbuddy_native_account",
    "list_workbuddy_native_captures",
  ]);
});
it("keeps independent requests isolated when old responses arrive last", async () => {
  let finish!: (v: unknown) => void;
  vi.mocked(invoke)
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValueOnce({
      ...saved,
      account: { ...account, label: "Newer" },
    });
  const old = workbuddyApi.captureNative({ label: "Older" });
  expect(
    (await workbuddyApi.captureNative({ label: "Newer" })).account.label,
  ).toBe("Newer");
  finish(saved);
  expect((await old).account.label).toBe("Synthetic");
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("does not dispatch a pre-cancelled request", async () => {
  const controller = new AbortController();
  controller.abort();
  await expect(
    workbuddyApi.captureNative({ label: "x" }, controller.signal),
  ).rejects.toMatchObject({ code: "cancelled" });
  expect(invoke).not.toHaveBeenCalled();
});

it.each(["applied", "alreadyApplied"] as const)(
  "accepts activation transaction result %s",
  async (result) => {
    vi.mocked(invoke).mockResolvedValue(result);
    expect(await workbuddyApi.activateNative(selection)).toBe(result);
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["activate_workbuddy_native_account", { target: selection }],
    ]);
  },
);
it.each(["restored", "alreadyApplied"] as const)(
  "accepts recovery transaction result %s",
  async (result) => {
    vi.mocked(invoke).mockResolvedValue(result);
    expect(await workbuddyApi.recoverNative()).toBe(result);
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["recover_workbuddy_native_switch"],
    ]);
  },
);
it.each([
  null,
  {},
  { id: "x", generation: 1 },
  { ...selection, generation: 1.5 },
  { ...selection, incarnation: "not-uuid" },
  { ...selection, token: "secret" },
])(
  "rejects an incomplete or extended activation target before IPC %#",
  async (target) => {
    await expect(
      workbuddyApi.activateNative(target as any),
    ).rejects.toMatchObject({ code: "invalidInput" });
    expect(invoke).not.toHaveBeenCalled();
  },
);
it.each([
  "saved",
  "restored",
  null,
  { state: "applied", token: "secret" },
  "secret",
])("rejects invalid activation result %#", async (result) => {
  vi.mocked(invoke).mockResolvedValue(result);
  await expect(workbuddyApi.activateNative(selection)).rejects.toMatchObject({
    code: "invalidResponse",
  });
  expect(invoke).toHaveBeenCalledTimes(1);
});
it.each([
  "applied",
  "saved",
  null,
  { state: "restored", token: "secret" },
  "secret",
])("rejects invalid recovery result %#", async (result) => {
  vi.mocked(invoke).mockResolvedValue(result);
  await expect(workbuddyApi.recoverNative()).rejects.toMatchObject({
    code: "invalidResponse",
  });
  expect(invoke).toHaveBeenCalledTimes(1);
});
it.each(["absent", "clear", "pending"] as const)(
  "preserves consistent inspection %s without claiming execution admission",
  async (phase) => {
    const value = {
      phase,
      requestOutcome: "unknown",
      canRequestActivation: phase !== "pending",
    };
    vi.mocked(invoke).mockResolvedValue({
      ...value,
      token: "secret",
      snapshot: { token: "secret" },
    });
    expect(await workbuddyApi.inspectNativeRecovery()).toEqual(value);
    expect(vi.mocked(invoke).mock.calls).toEqual([
      ["inspect_workbuddy_native_recovery"],
    ]);
  },
);
it.each([
  { phase: "pending", requestOutcome: "unknown", canRequestActivation: true },
  { phase: "absent", requestOutcome: "unknown", canRequestActivation: false },
  { phase: "clear", requestOutcome: "unknown", canRequestActivation: false },
])("rejects contradictory phase/activation snapshots %#", async (value) => {
  vi.mocked(invoke).mockResolvedValue(value);
  await expect(workbuddyApi.inspectNativeRecovery()).rejects.toMatchObject({
    code: "invalidResponse",
  });
  expect(invoke).toHaveBeenCalledTimes(1);
});

it.each([
  null,
  {},
  { phase: "unknown", requestOutcome: "unknown", canRequestActivation: false },
  { phase: "clear", requestOutcome: "applied", canRequestActivation: true },
  { phase: "pending", requestOutcome: "unknown" },
  { phase: "clear", requestOutcome: "unknown", canRequestActivation: "true" },
  { requestOutcome: "unknown", canRequestActivation: false },
])("rejects invalid recovery inspection %#", async (value) => {
  vi.mocked(invoke).mockResolvedValue(value);
  await expect(workbuddyApi.inspectNativeRecovery()).rejects.toMatchObject({
    code: "invalidResponse",
  });
});
it.each([
  "unsafePath",
  "io",
  "journal",
  "sourceChanged",
  "recoveryRequired",
  "requestConflict",
  "rolledBack",
])("keeps fixed native operation error %s without replay", async (code) => {
  vi.mocked(invoke).mockRejectedValue(code);
  for (const call of [
    () => workbuddyApi.activateNative(selection),
    () => workbuddyApi.recoverNative(),
    () => workbuddyApi.inspectNativeRecovery(),
  ])
    await expect(call()).rejects.toMatchObject({ code });
  expect(invoke).toHaveBeenCalledTimes(3);
});
const nativeOperations = [
  [
    "activate_workbuddy_native_account",
    (signal?: AbortSignal) => workbuddyApi.activateNative(selection, signal),
    "applied",
  ],
  [
    "recover_workbuddy_native_switch",
    (signal?: AbortSignal) => workbuddyApi.recoverNative(signal),
    "restored",
  ],
  [
    "inspect_workbuddy_native_recovery",
    (signal?: AbortSignal) => workbuddyApi.inspectNativeRecovery(signal),
    { phase: "clear", requestOutcome: "unknown", canRequestActivation: true },
  ],
] as const;
it.each(nativeOperations)(
  "cancels only the wait for %s and does not replay its late result",
  async (command, call, result) => {
    let finish!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const controller = new AbortController();
    const pending = call(controller.signal).catch((e) => e);
    controller.abort();
    expect(await pending).toMatchObject({ code: "cancelled" });
    finish(result);
    await Promise.resolve();
    vi.mocked(invoke).mockResolvedValue({
      phase: "clear",
      requestOutcome: "unknown",
      canRequestActivation: true,
    });
    expect((await workbuddyApi.inspectNativeRecovery()).requestOutcome).toBe(
      "unknown",
    );
    expect(vi.mocked(invoke).mock.calls.map(([name]) => name)).toEqual([
      command,
      "inspect_workbuddy_native_recovery",
    ]);
  },
);
it.each(nativeOperations)(
  "does not send pre-cancelled %s",
  async (_command, call) => {
    const controller = new AbortController();
    controller.abort();
    await expect(call(controller.signal)).rejects.toMatchObject({
      code: "cancelled",
    });
    expect(invoke).not.toHaveBeenCalled();
  },
);
it.each(nativeOperations)(
  "consumes a late rejected %s after cancellation",
  async (_command, call) => {
    let fail!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject;
        }),
    );
    const controller = new AbortController();
    const pending = call(controller.signal).catch((e) => e);
    controller.abort();
    expect(await pending).toMatchObject({ code: "cancelled" });
    fail(new Error("secret late rejection"));
    await Promise.resolve();
    await Promise.resolve();
    expect(invoke).toHaveBeenCalledTimes(1);
  },
);
it("redacts unrecognized activation/recovery/inspection errors", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("secret /private/path"));
  for (const [, call] of nativeOperations) {
    const error = await call().catch((e) => e);
    expect(error.code).toBe("unknown");
    expect(error.message).toBe("unknown");
    expect(error.cause).toBeUndefined();
  }
  expect(invoke).toHaveBeenCalledTimes(3);
});
it("does not let an older inspection change another caller's newer snapshot", async () => {
  let finish!: (value: unknown) => void;
  vi.mocked(invoke)
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValueOnce({
      phase: "pending",
      requestOutcome: "unknown",
      canRequestActivation: false,
    });
  const old = workbuddyApi.inspectNativeRecovery();
  const newer = await workbuddyApi.inspectNativeRecovery();
  finish({
    phase: "clear",
    requestOutcome: "unknown",
    canRequestActivation: true,
  });
  expect((await old).phase).toBe("clear");
  expect(newer).toEqual({
    phase: "pending",
    requestOutcome: "unknown",
    canRequestActivation: false,
  });
  expect(invoke).toHaveBeenCalledTimes(2);
});
