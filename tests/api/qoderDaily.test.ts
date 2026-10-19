import { expect, it, vi } from "vitest";
import { qoderDailyApi } from "@/lib/api/qoderDaily";
const invoke = vi.hoisted(() => vi.fn().mockResolvedValue({}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
it("passes only explicit source and the previously read generation to the real command", async () => {
  await qoderDailyApi.context("provider-a");
  expect(invoke).toHaveBeenCalledWith("qoder_daily_claim_context", {
    providerId: "provider-a",
  });
  await qoderDailyApi.claim({
    providerId: "provider-a",
    credentialGeneration: "generation-before-click",
    deviceDependency: null,
  });
  expect(invoke).toHaveBeenCalledWith("claim_qoder_daily_credits", {
    providerId: "provider-a",
    declaration: {
      providerId: "provider-a",
      credentialGeneration: "generation-before-click",
      source: "internationalQoderPat",
    },
  });
});
