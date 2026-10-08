import { expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { zcodeClaimApi } from "@/lib/api/zcodeClaim";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
it("preserves library scope and returns backend-owned claim state", async () => {
  const state = { enabled: false, participants: [], records: {}, busy: false };
  vi.mocked(invoke).mockResolvedValue(state);
  expect(await zcodeClaimApi.state("/synthetic/library")).toEqual(state);
  await zcodeClaimApi.setAuto("/synthetic/library", true, ["a"]);
  await zcodeClaimApi.start("/synthetic/library", ["a"], true);
  await zcodeClaimApi.cancel("/synthetic/library");
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["get_zcode_claim_state", { dataRoot: "/synthetic/library" }],
    [
      "set_zcode_claim_auto",
      { dataRoot: "/synthetic/library", enabled: true, participants: ["a"] },
    ],
    [
      "start_zcode_claim",
      { dataRoot: "/synthetic/library", ids: ["a"], previewOnly: true },
    ],
    ["cancel_zcode_claim", { dataRoot: "/synthetic/library" }],
  ]);
});
