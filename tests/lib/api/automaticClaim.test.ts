import { beforeEach, expect, it, vi } from "vitest";
import { automaticClaimApi } from "@/lib/api/automaticClaim";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
});
it.each(["workbuddy", "minimaxCode"] as const)(
  "reads %s scheduler state without a mutation",
  async (provider) => {
    const snapshot = {
      provider,
      phase: "waiting",
      lastCheckedAt: null,
      nextCheckAt: null,
      result: null,
      reason: null,
    };
    invoke.mockResolvedValue(snapshot);
    expect(await automaticClaimApi.status(provider)).toBe(snapshot);
    expect(invoke.mock.calls).toEqual([
      ["get_automatic_claim_status", { provider }],
    ]);
  },
);
it("does not retry a failed read or expose a write", async () => {
  invoke.mockRejectedValue(new Error("unavailable"));
  await expect(automaticClaimApi.status("workbuddy")).rejects.toThrow(
    "unavailable",
  );
  expect(invoke).toHaveBeenCalledOnce();
  expect(Object.keys(automaticClaimApi)).toEqual(["status"]);
});
