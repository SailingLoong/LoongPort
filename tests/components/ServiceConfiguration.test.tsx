import { PreservedView } from "@/components/ui/PreservedView";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createTestQueryClient } from "../utils/testQueryClient";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";
const switchTier = vi.fn();
const complete = vi.fn();
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: {
    status: async () => ({ completed: false }),
    complete: (...args: unknown[]) => complete(...args),
  },
}));
vi.mock("@/lib/api/relay", () => ({
  relayApi: {
    listRelays: async (app: string) =>
      app === "codex"
        ? [
            {
              id: 7,
              tiers: [
                { providerId: "p1", displayName: "Standard", appId: "codex" },
              ],
            },
          ]
        : [],
    switchTier: (...args: unknown[]) => switchTier(...args),
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
beforeEach(() => vi.clearAllMocks());
function renderConfiguration() {
  const onDone = vi.fn();
  const onBack = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ServiceConfiguration
        account={{ kind: "relay", rowId: 7, name: "Example" }}
        sourceAppId="codex"
        onBack={onBack}
        onDone={onDone}
      />
    </QueryClientProvider>,
  );
  return { onDone, onBack };
}
describe("ServiceConfiguration", () => {
  it("does not save consent when the required application exit confirmation is cancelled", async () => {
    const user = userEvent.setup();
    switchTier.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Standard",
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
      screen.getByRole("button", { name: "loongport.onboarding.finish" }),
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
        screen.getByRole("button", { name: "loongport.onboarding.finish" }),
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
    render(
      <QueryClientProvider client={createTestQueryClient()}>
        <ServiceConfiguration
          account={{ kind: "relay", rowId: 7, name: "Example" }}
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
          account={{ kind: "relay", rowId: 7, name: "Example" }}
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
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  ).toBeEnabled();
});
