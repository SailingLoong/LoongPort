import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { workbuddyApi } from "@/lib/api/workbuddy";
import { zcodeClaimApi } from "@/lib/api/zcodeClaim";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

it("keeps WorkBuddy explicit single-account claims separate from ZCode automatic participants", async () => {
  await zcodeClaimApi.setAuto("/synthetic/zcode", true, ["zcode-a"]);
  await workbuddyApi.list();
  await workbuddyApi.refresh("workbuddy-a");
  await workbuddyApi.refreshAll();
  expect(vi.mocked(invoke).mock.calls).toEqual([
    [
      "set_zcode_claim_auto",
      {
        dataRoot: "/synthetic/zcode",
        enabled: true,
        participants: ["zcode-a"],
      },
    ],
    ["list_workbuddy_accounts"],
    ["refresh_workbuddy_account", { id: "workbuddy-a" }],
    ["refresh_all_workbuddy_accounts"],
  ]);
  await workbuddyApi.claim("workbuddy-a");
  await zcodeClaimApi.start("/synthetic/zcode", ["zcode-a"], true);
  expect(vi.mocked(invoke).mock.calls.slice(-2)).toEqual([
    ["claim_workbuddy_today", { id: "workbuddy-a" }],
    [
      "start_zcode_claim",
      {
        dataRoot: "/synthetic/zcode",
        ids: ["zcode-a"],
        previewOnly: true,
      },
    ],
  ]);
});

it("preserves each backend's nullable data and does not retry failed claims", async () => {
  const workbuddy = {
    id: "workbuddy-a",
    credited: null,
    credits: { totalRemaining: null },
  };
  const zcode = { enabled: false, participants: [], records: {}, busy: false };
  vi.mocked(invoke)
    .mockResolvedValueOnce(workbuddy)
    .mockResolvedValueOnce(zcode);
  expect(await workbuddyApi.refresh("workbuddy-a")).toBe(workbuddy);
  expect(await zcodeClaimApi.state("/synthetic/zcode")).toBe(zcode);
  vi.mocked(invoke).mockRejectedValueOnce(new Error("synthetic failure"));
  await expect(workbuddyApi.claim("workbuddy-a")).rejects.toThrow(
    "synthetic failure",
  );
  expect(vi.mocked(invoke)).toHaveBeenCalledTimes(3);
});
