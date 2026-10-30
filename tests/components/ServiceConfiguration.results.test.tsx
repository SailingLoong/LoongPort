import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";
import { createTestQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  switch: vi.fn(),
  state: vi.fn(),
  complete: vi.fn(),
  vendorSwitch: vi.fn(),
  apply: vi.fn(),
  catalogPreview: vi.fn(),
  records: new Map<string, any>(),
  outcomes: new Map<string, any>(),
  backendRevision: "written",
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/lib/api/applicationRouting", () => ({
  applicationRoutingApi: { apply: mocks.apply },
}));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { state: mocks.state },
}));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: {
    status: async () => ({ completed: false }),
    complete: mocks.complete,
  },
}));
// Preserve all old vendor result/consent regressions; relay no longer enters
// this transient switch loop and has its own native-operation consumer tests.
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    list: async (app: string) => ({
      accounts: [
        ...(["claude", "codex"].includes(app)
          ? [
              {
                id: 7,
                plans: [
                  {
                    planId: app,
                    providerId: app,
                    planName: app,
                    canSwitch: true,
                  },
                ],
              },
            ]
          : []),
        ...(app === "claude"
          ? [
              {
                id: 9,
                plans: [
                  {
                    planId: "standard",
                    providerId: "vendor-generated-provider",
                    planName: "Standard",
                    canSwitch: true,
                  },
                ],
              },
            ]
          : []),
      ],
    }),
    queryConnection: async (_app: string, id: string | null) =>
      id === null
        ? { status: "none" }
        : (mocks.outcomes.get(id) ?? { status: "unknown", operationId: id }),
    prepareConnection: async (input: any) => {
      const record = {
        ...input,
        providerId:
          input.rowId === 9 ? "vendor-generated-provider" : input.plan,
        receiptRevision: 1,
        status: "prepared",
        operationState: "prepared",
        completedConfigurationRevision: null,
      };
      mocks.records.set(input.operationId, record);
      return record;
    },
    switchPreparedConnection: async (input: any) => {
      const decision = mocks.records.get(input.operationId)?.catalogDecision;
      const result = decision
        ? { status: "switched", warnings: [] }
        : input.rowId === 7
          ? await mocks.switch(
              input.plan,
              input.app,
              input.quitChatgpt ?? undefined,
            )
          : await mocks.vendorSwitch(
              input.rowId,
              input.plan,
              input.app,
              input.quitChatgpt ?? undefined,
            );
      if (result?.status === "switched") {
        mocks.outcomes.set(input.operationId, {
          ...mocks.records.get(input.operationId),
          status: "completed",
          operationState: "completed",
          completedConfigurationRevision:
            input.rowId === 9 ? "vendor-written" : mocks.backendRevision,
        });
      }
      return result;
    },
    previewConnectionReview: async (input: any) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 1,
      observedRevision: "observed",
    }),
    reviewConnection: async (input: any) => ({
      ...mocks.records.get(input.operationId),
      status: "failedBeforeWrite",
      operationState: "failedBeforeWrite",
      receiptRevision: 2,
    }),
    previewConnectionCatalog: mocks.catalogPreview,
    previewNewConnectionIntent: async (input: any) => ({
      ...input,
      status: "newIntentReviewRequired",
      receiptRevision: 1,
      originalRowId: input.targetRowId,
      originalPlan: input.targetPlan,
      targetProviderId:
        input.targetRowId === 9
          ? "vendor-generated-provider"
          : input.targetPlan,
      reviewToken: "a".repeat(64),
    }),
    confirmNewConnectionIntent: async (input: any) => ({
      status: "newIntentPermitted",
      app: input.app,
    }),
    switch: (rowId: number, id: string, app: string, quit?: boolean) =>
      rowId === 7
        ? mocks.switch(id, app, quit)
        : mocks.vendorSwitch(rowId, id, app, quit),
  },
}));
function snapshot(app: string, revision = "written", current = true) {
  return {
    configurationRevision: revision,
    configurations: [
      {
        providerId: app,
        selection: { kind: "vendor", rowId: 7, planId: app },
        presentation: { isCurrent: current, isInConfig: current },
      },
    ],
    isAdditive: false,
  };
}
function mount() {
  const onDone = vi.fn();
  const view = render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 7, name: "Example" }}
        sourceAppId="claude"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  return { ...view, onDone };
}
async function selectBoth() {
  await screen.findByRole("combobox", { name: "Codex" });
  fireEvent.change(screen.getByRole("combobox", { name: "Codex" }), {
    target: { value: "codex" },
  });
}
function appRow(name: string) {
  const row = screen.getByRole("combobox", { name }).parentElement;
  if (!row) throw new Error(`Missing configuration row for ${name}`);
  return within(row);
}

beforeEach(() => {
  mocks.records.clear();
  mocks.outcomes.clear();
  mocks.catalogPreview.mockReset();
  mocks.apply.mockReset();
  mocks.backendRevision = "written";
  mocks.switch
    .mockReset()
    .mockResolvedValue({ status: "switched", warnings: [] });
  mocks.state
    .mockReset()
    .mockImplementation(async (app: string) => snapshot(app));
  mocks.complete.mockReset().mockResolvedValue({ completed: true });
  mocks.vendorSwitch
    .mockReset()
    .mockResolvedValue({ status: "switched", warnings: [] });
});

it("rechecks the successful app and requires explicit reapply for an unknown result", async () => {
  mocks.switch.mockImplementation(async (_id, app) => {
    if (
      app === "codex" &&
      mocks.switch.mock.calls.filter(([, name]) => name === "codex").length ===
        1
    )
      throw new Error("B unavailable");
    return { status: "switched", warnings: [] };
  });
  const { onDone } = mount();
  await selectBoth();
  const shareData = await screen.findByRole("checkbox");
  fireEvent.click(shareData);
  expect(shareData).not.toBeChecked();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("B unavailable");
  expect(
    appRow("Claude Code").getByText("loongport.onboarding.results.success"),
  ).toBeInTheDocument();
  expect(
    appRow("Codex").getByText("loongport.onboarding.results.unverified"),
  ).toBeInTheDocument();
  expect(screen.getByRole("combobox", { name: "Codex" })).toBeDisabled();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() => expect(mocks.state).toHaveBeenCalledTimes(2));
  expect(mocks.switch.mock.calls.map(([, app]) => app)).toEqual([
    "claude",
    "codex",
  ]);
  expect(mocks.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
  expect(
    appRow("Claude Code").getByText("loongport.onboarding.results.success"),
  ).toBeInTheDocument();
  expect(
    appRow("Codex").getByText("loongport.onboarding.results.unverified"),
  ).toBeInTheDocument();
  await waitFor(() =>
    expect(
      appRow("Codex").getByRole("button", {
        name: "loongport.onboarding.reapply",
      }),
    ).toBeEnabled(),
  );
  fireEvent.click(
    appRow("Codex").getByRole("button", {
      name: "loongport.onboarding.reapply",
    }),
  );
  expect(mocks.complete).not.toHaveBeenCalled();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "applications.vendorRecovery.confirm",
    }),
  );
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.switch.mock.calls.map(([, app]) => app)).toEqual([
    "claude",
    "codex",
    "codex",
  ]);
  expect(mocks.state.mock.calls.map(([app]) => app)).toEqual([
    "claude",
    "claude",
    "claude",
    "codex",
  ]);
  expect(mocks.complete).toHaveBeenCalledExactlyOnceWith(false);
});

it("requires separate explicit permission for a changed success and an unknown result", async () => {
  mocks.switch
    .mockResolvedValueOnce({ status: "switched", warnings: [] })
    .mockRejectedValueOnce(new Error("B unavailable"));
  const { onDone } = mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("B unavailable");
  mocks.state.mockImplementation(async (app: string) =>
    snapshot(app, "external"),
  );
  mocks.backendRevision = "external";
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await screen.findByText("loongport.onboarding.results.changed");
  expect(
    appRow("Claude Code").getByText("loongport.onboarding.results.changed"),
  ).toBeInTheDocument();
  expect(
    appRow("Codex").getByText("loongport.onboarding.results.unverified"),
  ).toBeInTheDocument();
  expect(mocks.switch).toHaveBeenCalledTimes(2);
  expect(mocks.complete).not.toHaveBeenCalled();
  fireEvent.click(
    appRow("Claude Code").getByRole("button", {
      name: "loongport.onboarding.reapply",
    }),
  );
  expect(mocks.complete).not.toHaveBeenCalled();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "applications.vendorRecovery.confirm",
    }),
  );
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  await waitFor(() =>
    expect(
      screen.getByRole("button", {
        name: "loongport.onboarding.continueSetup",
      }),
    ).toBeEnabled(),
  );
  expect(mocks.switch.mock.calls.map(([, app]) => app)).toEqual([
    "claude",
    "codex",
    "claude",
  ]);
  expect(mocks.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
  expect(
    appRow("Codex").getByText("loongport.onboarding.results.unverified"),
  ).toBeInTheDocument();
  fireEvent.click(
    appRow("Codex").getByRole("button", {
      name: "loongport.onboarding.reapply",
    }),
  );
  expect(mocks.complete).not.toHaveBeenCalled();
  fireEvent.click(
    await screen.findByRole("button", {
      name: "applications.vendorRecovery.confirm",
    }),
  );
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.switch.mock.calls.map(([, app]) => app)).toEqual([
    "claude",
    "codex",
    "claude",
    "codex",
  ]);
  expect(mocks.complete).toHaveBeenCalledExactlyOnceWith(true);
});

it("does not write or complete if a successful item's authoritative state cannot be read", async () => {
  mocks.switch
    .mockResolvedValueOnce({ status: "switched", warnings: [] })
    .mockRejectedValueOnce(new Error("B unavailable"));
  mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("B unavailable");
  mocks.state.mockRejectedValue(new Error("Cannot verify A"));
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await screen.findByText("Cannot verify A");
  expect(mocks.switch).toHaveBeenCalledTimes(2);
  expect(mocks.complete).not.toHaveBeenCalled();
});

it("keeps successful items after consent persistence fails and only retries completion", async () => {
  mocks.complete.mockRejectedValueOnce(new Error("Cannot save choice"));
  const { onDone } = mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("Cannot save choice");
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.switch).toHaveBeenCalledTimes(2);
  expect(mocks.complete).toHaveBeenCalledTimes(2);
});

it("keeps a cancelled item separate from earlier success", async () => {
  mocks.switch
    .mockResolvedValueOnce({ status: "switched", warnings: [] })
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Codex target",
      quitRequired: true,
    });
  mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  fireEvent.click(await screen.findByRole("button", { name: "common.cancel" }));
  await screen.findByText("loongport.onboarding.results.cancelled");
  expect(
    screen.getByText("loongport.onboarding.results.success"),
  ).toBeInTheDocument();
  expect(mocks.complete).not.toHaveBeenCalled();
});

it("prevents same-tick duplicate application and stops before B when navigation abandons A", async () => {
  let resolve!: (value: unknown) => void;
  mocks.switch.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const { unmount, onDone } = mount();
  await selectBoth();
  const button = screen.getByRole("button", {
    name: "loongport.onboarding.finish",
  });
  act(() => {
    fireEvent.click(button);
    fireEvent.click(button);
  });
  await waitFor(() => expect(mocks.switch).toHaveBeenCalledTimes(1));
  unmount();
  await act(async () => resolve({ status: "switched", warnings: [] }));
  expect(mocks.switch).toHaveBeenCalledTimes(1);
  expect(mocks.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
});

it("recovers a completed receipt after a failed current-state read without replaying the write", async () => {
  mocks.state.mockRejectedValueOnce(new Error("Post-switch read failed"));
  const { onDone } = mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("Post-switch read failed");
  await waitFor(() => expect(mocks.switch).toHaveBeenCalledTimes(2));
  expect(mocks.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
  // A durable completion receipt plus a fresh authoritative read recovers A.
  // It must not require repeating a known-completed write, nor stall independent B.
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.switch.mock.calls.map(([, app]) => app)).toEqual([
    "claude",
    "codex",
  ]);
  expect(mocks.complete).toHaveBeenCalledExactlyOnceWith(true);
});

it("verifies a vendor plan using its account and selection identity rather than its generated provider ID", async () => {
  mocks.state.mockResolvedValue({
    configurationRevision: "vendor-written",
    isAdditive: false,
    configurations: [
      {
        providerId: "vendor-generated-provider",
        selection: { kind: "vendor", rowId: 9, planId: "standard" },
        presentation: { isCurrent: true },
      },
    ],
  });
  const onDone = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 9, name: "Example vendor" }}
        sourceAppId="claude"
        onBack={vi.fn()}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  await screen.findByRole("combobox", { name: "Claude Code" });
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.vendorSwitch).toHaveBeenCalledWith(
    9,
    "standard",
    "claude",
    undefined,
  );
});

it("previews catalog management before final onboarding apply and never marks preview as success", async () => {
  const catalog = {
    ownership: "unclaimedExternal",
    choice: "keep",
    canManage: true,
    revision: "catalog-v1",
    beforePointer: "/external/models.json",
    afterPointer: "/external/models.json",
  };
  mocks.switch.mockImplementation(async (_id, app) =>
    app === "codex"
      ? {
          status: "confirmationRequired",
          targetName: "Codex",
          catalog,
          quitRequired: false,
        }
      : { status: "switched", warnings: [] },
  );
  mocks.catalogPreview
    .mockReset()
    .mockResolvedValueOnce({
      status: "confirmationRequired",
      targetName: "Codex",
      catalog: {
        ...catalog,
        choice: "manage",
        afterPointer: "/managed/models.json",
      },
      quitRequired: false,
    })
    .mockResolvedValueOnce({ status: "switched", warnings: [] });
  const { onDone } = mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByRole("dialog");
  fireEvent.click(
    screen.getByRole("radio", { name: "provider.preview.catalog.manage" }),
  );
  expect(
    screen.getByRole("button", { name: "applications.apply" }),
  ).toBeDisabled();
  fireEvent.click(
    screen.getByRole("button", {
      name: "provider.preview.catalog.confirmChoice",
    }),
  );
  await screen.findByText("/managed/models.json");
  const catalogDecision = { choice: "manage", revision: "catalog-v1" };
  expect(mocks.catalogPreview).toHaveBeenCalledExactlyOnceWith({
    app: "codex",
    operationId: expect.any(String),
    catalogDecision,
    quitChatgpt: null,
  });
  expect(mocks.apply).not.toHaveBeenCalled();
  expect(mocks.complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "applications.apply" }));
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  const prepared = [...mocks.records.values()].filter(
    (record) => record.app === "codex",
  );
  expect(prepared).toHaveLength(2);
  expect(prepared[0].catalogDecision).toBeNull();
  expect(prepared[1].catalogDecision).toEqual(catalogDecision);
  expect(prepared[1].operationId).not.toBe(prepared[0].operationId);
  expect(mocks.apply).not.toHaveBeenCalled();
});
