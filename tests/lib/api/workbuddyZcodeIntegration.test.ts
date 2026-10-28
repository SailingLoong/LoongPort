import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { workbuddyApi } from "@/lib/api/workbuddy";
import { zcodeClaimApi } from "@/lib/api/zcodeClaim";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

it("batch WorkBuddy claims use one explicit backend command without ZCode participation", async () => {
  const results = [{ account: { id: "workbuddy-a" }, error: "busy" }];
  vi.mocked(invoke).mockResolvedValueOnce(results);
  expect(await workbuddyApi.claimAll()).toBe(results);
  expect(vi.mocked(invoke).mock.calls).toEqual([["claim_all_workbuddy_today"]]);
});

it("keeps WorkBuddy explicit single-account claims separate from ZCode automatic participants", async () => {
  await zcodeClaimApi.setAuto(
    "/synthetic/zcode",
    true,
    ["zcode-a"],
    "context:0",
  );
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
        expectedRevision: "context:0",
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

it("inference commands use the selected original account and opaque flow identifiers", async () => {
  await workbuddyApi.listInferenceAccounts();
  await workbuddyApi.beginInferenceLogin("original-account");
  await workbuddyApi.pollInferenceLogin("opaque-flow");
  await workbuddyApi.cancelInferenceLogin("opaque-flow");
  await workbuddyApi.discoverModels("original-account");
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["list_workbuddy_inference_accounts"],
    ["begin_workbuddy_inference_authorization", { id: "original-account" }],
    ["poll_workbuddy_inference_authorization", { flowId: "opaque-flow" }],
    ["cancel_workbuddy_inference_authorization", { flowId: "opaque-flow" }],
    ["discover_workbuddy_inference_models", { id: "original-account" }],
  ]);
});

it("inference permission is a dedicated command, not another login", async () => {
  await workbuddyApi.setInferenceConsent("account-a", true);
  await workbuddyApi.setInferenceConsent("account-a", false);
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["set_workbuddy_inference_consent", { id: "account-a", enabled: true }],
    ["set_workbuddy_inference_consent", { id: "account-a", enabled: false }],
  ]);
});

it("native observation invokes only the explicit no-argument read command", async () => {
  await workbuddyApi.previewNative();
  expect(vi.mocked(invoke).mock.calls).toEqual([
    ["preview_workbuddy_native_capture"],
  ]);
});
