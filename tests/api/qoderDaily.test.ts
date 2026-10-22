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

it("reads local automatic context and binds enable consent to its exact revision", async () => {
  const context = {
    providerId: "provider-a",
    credentialGeneration: "generation-before-click",
    revision: 4,
    policyVersion: 1,
    enabled: false,
    canEnable: true,
    reason: null,
    subject: null,
  };
  await qoderDailyApi.automaticContext("provider-a");
  expect(invoke).toHaveBeenCalledWith("qoder_daily_automatic_context", {
    providerId: "provider-a",
  });
  await qoderDailyApi.setAutomatic(context, true);
  expect(invoke).toHaveBeenCalledWith("set_qoder_daily_automatic", {
    request: {
      providerId: "provider-a",
      credentialGeneration: "generation-before-click",
      expectedRevision: 4,
      policyVersion: 1,
      enabled: true,
      source: "internationalQoderPat",
    },
  });
  await qoderDailyApi.setAutomatic(context, false);
  expect(invoke).toHaveBeenCalledWith("set_qoder_daily_automatic", {
    request: {
      providerId: "provider-a",
      credentialGeneration: "generation-before-click",
      expectedRevision: 4,
      policyVersion: 1,
      enabled: false,
      source: null,
    },
  });
});
