import { beforeAll, expect, it, vi } from "vitest";

const boundary = vi.hoisted(() => ({
  invoke: undefined as
    undefined | ((command: string, payload?: unknown) => unknown),
  emit: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/api/mocks", () => ({
  mockIPC: (handler: typeof boundary.invoke) => {
    boundary.invoke = handler;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ emit: boundary.emit }));
vi.mock("react-dom/client", () => ({
  createRoot: () => ({ render: vi.fn() }),
}));
beforeAll(async () => {
  await import("../browser/unified-accounts");
});

it("marks the fixture source unrecorded without the screenshot build injection", () => {
  expect((window as any).unifiedLayout.checkout).toBe("unrecorded");
});

for (const provider of ["workbuddy", "minimaxCode"] as const) {
  it(`serves ${provider} native-shaped snapshots and re-reads changed state`, async () => {
    const read = () =>
      boundary.invoke!("get_automatic_claim_status", { provider });
    expect(read()).toEqual({
      provider,
      phase: "waiting",
      lastCheckedAt: null,
      nextCheckAt: 1800000000000,
      result: null,
      reason: null,
    });
    const fixture = (window as any).unifiedLayout;
    await fixture.completeAutomaticCheck(provider);
    expect(read()).toEqual({
      provider,
      phase: "waiting",
      lastCheckedAt: 1800000000000,
      nextCheckAt: 1800000900000,
      result: "skipped",
      reason: "disabled",
    });
    expect(boundary.emit).toHaveBeenLastCalledWith(
      "automatic-claim-status-updated",
      expect.objectContaining({ provider, result: "skipped" }),
    );
    expect(fixture.automaticStatusReads[provider]).toBe(2);
  });
}
it("keeps unknown commands and unsupported providers hard rejected", () => {
  expect(() => boundary.invoke!("not_a_real_command")).toThrow(
    "Unexpected synthetic fixture command",
  );
  expect(() =>
    boundary.invoke!("get_automatic_claim_status", { provider: "other" }),
  ).toThrow();
  expect((window as any).unifiedLayout.rejectedCommands).toContain(
    "not_a_real_command",
  );
});

it("provides a synthetic WorkBuddy account so its detail status can render", () => {
  expect(boundary.invoke!("list_workbuddy_accounts")).toEqual([
    expect.objectContaining({
      id: "synthetic-workbuddy",
      label: "Synthetic WorkBuddy account",
      canClaim: false,
      autoClaimEnabled: false,
    }),
  ]);
});
