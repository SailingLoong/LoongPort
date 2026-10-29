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
