import { PreservedView } from "@/components/ui/PreservedView";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createTestQueryClient } from "../utils/testQueryClient";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";
const switchTier = vi.fn();
const complete = vi.fn();
const records = new Map<string, any>();
const outcomes = new Map<string, any>();
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: {
    state: async () => ({
      configurationRevision: "fixture-revision",
      configurations: [
        {
          providerId: "p1",
          selection: { kind: "vendor", rowId: 7, planId: "p1" },
          presentation: { isCurrent: true },
        },
      ],
      isAdditive: false,
    }),
  },
}));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: {
    status: async () => ({ completed: false }),
    complete: (...args: unknown[]) => complete(...args),
  },
}));
// The original transient configuration owner is now vendor-only. Relay's
// durable preparation/application coverage lives in RelaySetupConfiguration.
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    list: async (app: string) => ({
      accounts:
        app === "codex"
          ? [
              {
                id: 7,
                plans: [
                  {
                    planId: "p1",
                    providerId: "p1",
                    planName: "Standard",
                    canSwitch: true,
                  },
                ],
              },
            ]
          : [],
    }),
    queryConnection: async (_app: string, id: string | null) =>
      id === null
        ? { status: "none" }
        : (outcomes.get(id) ?? { status: "unknown", operationId: id }),
    prepareConnection: async (input: any) => {
      const record = {
        ...input,
        providerId: "p1",
        receiptRevision: 1,
        status: "prepared",
        operationState: "prepared",
        completedConfigurationRevision: null,
      };
      records.set(input.operationId, record);
      return record;
    },
    switchPreparedConnection: async (input: any) => {
      const result = await switchTier(
        input.plan,
        input.app,
        input.quitChatgpt ?? undefined,
      );
      if (result?.status === "switched")
        outcomes.set(input.operationId, {
          ...records.get(input.operationId),
          status: "completed",
          operationState: "completed",
          completedConfigurationRevision: "fixture-revision",
        });
      return result;
    },
    switch: (_rowId: number, id: string, app: string, quit?: boolean) =>
      switchTier(id, app, quit),
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
beforeEach(() => {
  vi.clearAllMocks();
  records.clear();
  outcomes.clear();
});
function renderConfiguration() {
  const onDone = vi.fn();
  const onBack = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ServiceConfiguration
        account={{ kind: "vendor", rowId: 7, name: "Example" }}
        sourceAppId="codex"
        onBack={onBack}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  return { onDone, onBack };
}
describe("ServiceConfiguration", () => {
  it("owns Escape before shell navigation and blocks it while finishing", async () => {
    const user = userEvent.setup();
    let resolveSwitch!: (value: unknown) => void;
    switchTier.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSwitch = resolve;
        }),
    );
    const shellBack = vi.fn();
    const shellKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) shellBack();
    };
    window.addEventListener("keydown", shellKeyDown);
    try {
      const { onBack } = renderConfiguration();
      await screen.findByText("Standard");
      fireEvent.keyDown(document.body, { key: "Escape" });
      expect(onBack).toHaveBeenCalledTimes(1);
      expect(shellBack).not.toHaveBeenCalled();
      await user.click(
        screen.getByRole("button", { name: "loongport.onboarding.finish" }),
      );
      expect(
        screen.getByRole("button", { name: "common.back" }),
      ).toBeDisabled();
      fireEvent.keyDown(document.body, { key: "Escape" });
      expect(onBack).toHaveBeenCalledTimes(1);
      expect(shellBack).not.toHaveBeenCalled();
      await act(async () => resolveSwitch({ status: "cancelled" }));
    } finally {
      window.removeEventListener("keydown", shellKeyDown);
      switchTier.mockReset();
    }
  });

  it("does not save consent when the required application exit confirmation is cancelled", async () => {
    const user = userEvent.setup();
    switchTier.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Standard",
      quitRequired: true,
    });
    const { onDone } = renderConfiguration();
    await screen.findByText("Standard");
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.finish" }),
    );
    await user.click(
      await screen.findByRole("button", { name: "common.cancel" }),
    );
    expect(complete).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", {
        name: "loongport.onboarding.continueSetup",
      }),
    ).toBeEnabled();
  });
  it("keeps the consent draft and configuration choices after a failed switch", async () => {
    const user = userEvent.setup();
    switchTier.mockRejectedValue(new Error("Configuration unavailable"));
    const { onDone } = renderConfiguration();
    await screen.findByText("Standard");
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.finish" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", {
          name: "loongport.onboarding.continueSetup",
        }),
      ).toBeEnabled(),
    );
    expect(complete).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
    expect(screen.getByRole("combobox", { name: "Codex" })).toHaveValue("p1");
  });
  it("does not change configuration or consent when returning", async () => {
    const user = userEvent.setup();
    const { onBack } = renderConfiguration();
    await screen.findByText("Standard");
    await user.click(screen.getByRole("button", { name: "common.back" }));
    expect(onBack).toHaveBeenCalled();
    expect(switchTier).not.toHaveBeenCalled();
    expect(complete).not.toHaveBeenCalled();
  });
  it("keeps consent a draft until selected configuration has successfully switched", async () => {
    const user = userEvent.setup();
    const onDone = vi.fn();
    switchTier.mockResolvedValue({ status: "switched", warnings: [] });
    complete.mockResolvedValue({ completed: true });
    const client = createTestQueryClient();
    client.setQueryData(["credentialProtection"], { shouldPrompt: false });
    render(
      <QueryClientProvider client={client}>
        <ServiceConfiguration
          account={{ kind: "vendor", rowId: 7, name: "Example" }}
          sourceAppId="codex"
          onBack={vi.fn()}
          onDone={onDone}
        />
      </QueryClientProvider>,
    );
    await screen.findByText("Standard");
    expect(complete).not.toHaveBeenCalled();
    await user.click(
      screen.getByRole("checkbox", { name: "loongport.onboarding.shareLabel" }),
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.finish" }),
    );
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(switchTier).toHaveBeenCalledWith("p1", "codex", undefined);
    expect(complete).toHaveBeenCalledWith(false);
    expect(client.getQueryState(["credentialProtection"])?.isInvalidated).toBe(
      true,
    );
  });
});

it("does not resume an abandoned finish operation after returning to the page", async () => {
  const user = userEvent.setup();
  const onDone = vi.fn();
  let resolveSwitch!: (value: unknown) => void;
  switchTier.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        resolveSwitch = resolve;
      }),
  );
  const client = createTestQueryClient();
  const view = (active: boolean) => (
    <QueryClientProvider client={client}>
      <PreservedView active={active}>
        <ServiceConfiguration
          account={{ kind: "vendor", rowId: 7, name: "Example" }}
          sourceAppId="codex"
          onBack={vi.fn()}
          onDone={onDone}
        />
      </PreservedView>
    </QueryClientProvider>
  );
  const { rerender } = render(view(true));
  await screen.findByText("Standard");
  await user.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  rerender(view(false));
  rerender(view(true));
  await act(async () => {
    resolveSwitch({ status: "switched", warnings: [] });
  });
  expect(complete).not.toHaveBeenCalled();
  expect(onDone).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  ).toBeEnabled();
});
