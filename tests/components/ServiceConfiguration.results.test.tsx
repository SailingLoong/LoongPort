import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import { ServiceConfiguration } from "@/components/relay/onboarding/ServiceConfiguration";
import { createTestQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  switch: vi.fn(),
  state: vi.fn(),
  complete: vi.fn(),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
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
vi.mock("@/lib/api/relay", () => ({
  relayApi: {
    listRelays: async (app: string) =>
      ["claude", "codex"].includes(app)
        ? [{ id: 7, tiers: [{ providerId: app, displayName: app }] }]
        : [],
    switchTier: mocks.switch,
  },
}));
function snapshot(app: string, revision = "written", current = true) {
  return {
    configurationRevision: revision,
    configurations: [
      {
        providerId: app,
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
        account={{ kind: "relay", rowId: 7, name: "Example" }}
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
beforeEach(() => {
  mocks.switch
    .mockReset()
    .mockResolvedValue({ status: "switched", warnings: [] });
  mocks.state
    .mockReset()
    .mockImplementation(async (app: string) => snapshot(app));
  mocks.complete.mockReset().mockResolvedValue({ completed: true });
});

it("shows partial success and retries B without applying still-matching A again", async () => {
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
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("B unavailable");
  expect(
    screen.getByText("loongport.onboarding.results.success"),
  ).toBeInTheDocument();
  expect(screen.getByRole("combobox", { name: "Codex" })).toBeDisabled();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.switch.mock.calls.map(([, app]) => app)).toEqual([
    "claude",
    "codex",
    "codex",
  ]);
  expect(mocks.state.mock.calls.map(([app]) => app)).toContain("claude");
});

it("requires explicit permission before reapplying a successful item changed externally", async () => {
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
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await screen.findByText("loongport.onboarding.results.changed");
  expect(mocks.switch).toHaveBeenCalledTimes(2);
  expect(mocks.complete).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.reapply" }),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
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

it("requires explicit reapplication when a completed switch cannot be verified", async () => {
  mocks.state.mockRejectedValueOnce(new Error("Post-switch read failed"));
  const { onDone } = mount();
  await selectBoth();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.finish" }),
  );
  await screen.findByText("Post-switch read failed");
  expect(mocks.switch).toHaveBeenCalledTimes(1);
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  expect(mocks.switch).toHaveBeenCalledTimes(1);
  expect(mocks.complete).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.reapply" }),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.onboarding.continueSetup" }),
  );
  await waitFor(() => expect(onDone).toHaveBeenCalledOnce());
  expect(mocks.switch).toHaveBeenCalledTimes(3);
});
