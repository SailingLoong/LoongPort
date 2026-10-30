import { beforeEach, expect, it, vi } from "vitest";
import { vendorApi } from "@/lib/api/vendor";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => invoke.mockReset());
const identity = {
  app: "codex" as const,
  operationId: "f8d02319-67c1-426b-a931-c836de11320e",
};
const review = {
  ...identity,
  status: "reviewRequired",
  receiptRevision: 2,
  observedRevision: "observed",
};
const target = { ...identity, targetRowId: 7, targetPlan: "standard" };
const intent = {
  ...target,
  status: "newIntentReviewRequired",
  receiptRevision: 2,
  originalRowId: 6,
  originalPlan: "old",
  targetProviderId: "provider-7",
  reviewToken: "a".repeat(64),
};
it("preserves exact review proof and command input", async () => {
  invoke.mockResolvedValue(review);
  expect(await vendorApi.previewConnectionReview(identity)).toEqual(review);
  expect(invoke).toHaveBeenCalledWith(
    "vendor_preview_connection_review",
    identity,
  );
});
it.each([
  { app: "claude" },
  { operationId: "00000000-0000-0000-0000-000000000000" },
  { receiptRevision: 0 },
  { observedRevision: "" },
  { newGate: true },
])("rejects mismatched or malformed review %j", async (change) => {
  invoke.mockResolvedValue({ ...review, ...change });
  expect(await vendorApi.previewConnectionReview(identity)).toEqual({
    status: "unknown",
  });
});
it("returns only reviewed terminal receipt, not completed or adoption", async () => {
  const receipt = {
    ...identity,
    rowId: 7,
    plan: "standard",
    providerId: "provider-7",
    receiptRevision: 3,
    completedConfigurationRevision: null,
    status: "reviewed",
    operationState: "reviewed",
  };
  invoke.mockResolvedValue(receipt);
  expect(
    await vendorApi.reviewConnection({
      ...identity,
      expectedRevision: 2,
      observedRevision: "observed",
    }),
  ).toEqual(receipt);
});
it("preserves bound new-intent token without applying", async () => {
  invoke.mockResolvedValue(intent);
  expect(await vendorApi.previewNewConnectionIntent(target)).toEqual(intent);
  expect(invoke.mock.calls).toEqual([
    ["vendor_preview_new_connection_intent", target],
  ]);
});
it.each([
  { targetRowId: 8 },
  { targetPlan: "other" },
  { app: "claude" },
  { reviewToken: "bad" },
  { extraGate: true },
])("rejects unbound new intent %j", async (change) => {
  invoke.mockResolvedValue({ ...intent, ...change });
  expect(await vendorApi.previewNewConnectionIntent(target)).toEqual({
    status: "unknown",
  });
});
it("confirms only exact app permission and never switches automatically", async () => {
  invoke.mockResolvedValue({ status: "newIntentPermitted", app: "codex" });
  const input = { ...target, reviewToken: intent.reviewToken };
  expect(await vendorApi.confirmNewConnectionIntent(input)).toEqual({
    status: "newIntentPermitted",
    app: "codex",
  });
  expect(invoke.mock.calls).toEqual([
    ["vendor_confirm_new_connection_intent", input],
  ]);
});
it("rejects confirmation extra gate or foreign app", async () => {
  for (const result of [
    { status: "newIntentPermitted", app: "claude" },
    { status: "newIntentPermitted", app: "codex", gate: true },
  ]) {
    invoke.mockResolvedValue(result);
    expect(
      await vendorApi.confirmNewConnectionIntent({
        ...target,
        reviewToken: intent.reviewToken,
      }),
    ).toEqual({ status: "unknown" });
  }
});
it("catalog preview keeps native confirmation shape without switching", async () => {
  const result = {
    status: "confirmationRequired",
    targetName: "Codex",
    quitRequired: true,
  };
  invoke.mockResolvedValue(result);
  const input = {
    ...identity,
    catalogDecision: { choice: "keep" as const, revision: "a".repeat(64) },
    quitChatgpt: false,
  };
  expect(await vendorApi.previewConnectionCatalog(input)).toEqual(result);
  expect(invoke.mock.calls).toEqual([
    ["vendor_preview_connection_catalog", input],
  ]);
});
it.each(["completed", "prepared", "applying", "unknown"])(
  "review refuses non-retired %s receipts",
  async (state) => {
    invoke.mockResolvedValue({
      ...identity,
      rowId: 7,
      plan: "standard",
      providerId: "provider-7",
      receiptRevision: 3,
      completedConfigurationRevision: null,
      status: state,
      operationState: state,
    });
    expect(
      await vendorApi.reviewConnection({
        ...identity,
        expectedRevision: 2,
        observedRevision: "observed",
      }),
    ).toEqual({ status: "unknown" });
  },
);
it("review refuses unchanged receipt revision", async () => {
  invoke.mockResolvedValue({
    ...identity,
    rowId: 7,
    plan: "standard",
    providerId: "provider-7",
    receiptRevision: 2,
    completedConfigurationRevision: null,
    status: "failedBeforeWrite",
    operationState: "failedBeforeWrite",
  });
  expect(
    await vendorApi.reviewConnection({
      ...identity,
      expectedRevision: 2,
      observedRevision: "observed",
    }),
  ).toEqual({ status: "unknown" });
});
it.each([
  {
    status: "switched",
    providerName: "x",
    chatgptWasRunning: false,
    chatgptRelaunched: false,
    warnings: [],
  },
  {
    status: "confirmationRequired",
    targetName: "x",
    quitRequired: true,
    newGate: true,
  },
  {
    status: "confirmationRequired",
    targetName: "x",
    quitRequired: true,
    catalog: { choice: "manage" },
  },
])("catalog refuses non-preview or incomplete response %j", async (result) => {
  invoke.mockResolvedValue(result);
  expect(
    await vendorApi.previewConnectionCatalog({
      ...identity,
      catalogDecision: { choice: "keep", revision: "a".repeat(64) },
      quitChatgpt: null,
    }),
  ).toEqual({ status: "unknown" });
});
