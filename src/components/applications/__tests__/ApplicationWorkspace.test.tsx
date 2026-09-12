import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, it, expect, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";

const state = vi.hoisted(() => ({
  data: {} as any,
  routing: {} as any,
  select: vi.fn(),
  setOrder: vi.fn(),
  setFailover: vi.fn(),
}));
vi.mock("../useApplicationOverview", () => ({
  useApplicationOverview: () => ({
    data: state.data,
    isPending: false,
    error: null,
    refetch: vi.fn(),
    select: state.select,
    busy: false,
    confirmation: null,
    cancel: vi.fn(),
    confirm: vi.fn(),
  }),
}));
vi.mock("../useApplicationRouting", () => ({
  useApplicationRouting: () => ({
    data: state.routing,
    isPending: false,
    error: null,
    refetch: vi.fn(),
    busy: false,
    setOrder: state.setOrder,
    setFailover: state.setFailover,
  }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/components/relay/SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: () => null,
}));
const config = (id: string, name: string, current = false) => ({
  providerId: id,
  name,
  source: "relay",
  account: { kind: "relay", id: 7 },
  serviceName: "Example service",
  accountLabel: "Personal",
  configurationName: name,
  model: null,
  presentation: { isCurrent: current, isInConfig: true, isDefaultModel: false },
  canSelect: true,
  selection: { kind: "relay" },
});
const props = {
  appId: "codex" as const,
  providers: {},
  onSwitchProvider: vi.fn(),
  onOpenAccount: vi.fn(),
  onAdd: vi.fn(),
  children: <div>Advanced configuration actions</div>,
};
const names = () =>
  screen
    .getAllByRole("row")
    .slice(1)
    .map((row) =>
      within(row)
        .getByRole("button", { name: /applications.use/ })
        .getAttribute("aria-label"),
    );
beforeEach(() => {
  vi.clearAllMocks();
  state.setFailover.mockResolvedValue(undefined);
  state.setOrder.mockResolvedValue(undefined);
  state.data = {
    configurations: [
      config("a", "Standard", true),
      config("b", "Premium"),
      config("c", "Unknown"),
    ],
    recentProviderIds: [],
    isAdditive: false,
  };
  state.routing = {
    autoFailoverEnabled: false,
    tiers: [
      {
        providerId: "a",
        position: 0,
        rateMultiplier: 2,
        errorRate: 0.02,
        balanceUsd: 10,
        skipReason: null,
      },
      {
        providerId: "b",
        position: 1,
        rateMultiplier: 0.5,
        errorRate: 0.01,
        balanceUsd: 50,
        skipReason: null,
      },
      {
        providerId: "c",
        position: 2,
        rateMultiplier: null,
        errorRate: null,
        balanceUsd: null,
        skipReason: null,
      },
    ],
  };
});
describe("application workspace", () => {
  it.each([false, true])(
    "keeps every tier directly selectable when failover is %s",
    async (enabled) => {
      state.routing.autoFailoverEnabled = enabled;
      render(<ApplicationWorkspace {...props} />);
      expect(
        screen.getByRole("columnheader", { name: "applications.priority" }),
      ).toBeVisible();
      expect(screen.getByText("Standard")).toBeVisible();
      expect(screen.getByText("Premium")).toBeVisible();
      expect(
        screen.queryByText("applications.switchService"),
      ).not.toBeInTheDocument();
      await userEvent.click(
        screen.getByRole("button", { name: "applications.use Premium" }),
      );
      expect(state.select).toHaveBeenCalledWith(
        expect.objectContaining({ providerId: "b" }),
      );
    },
  );
  it("searches immediately without changing priority or current selection", async () => {
    render(<ApplicationWorkspace {...props} />);
    await userEvent.type(screen.getByRole("searchbox"), "Premium");
    expect(screen.queryByText("Standard")).not.toBeInTheDocument();
    const row = screen.getByText("Premium").closest("tr")!;
    expect(within(row).getByText("2")).toBeVisible();
    expect(state.setOrder).not.toHaveBeenCalled();
    expect(state.select).not.toHaveBeenCalled();
  });
  it("persists metric sort with unknown values last, and reverses on second click", async () => {
    render(<ApplicationWorkspace {...props} />);
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.metrics.rateMultiplier",
      }),
    );
    expect(state.setOrder).toHaveBeenLastCalledWith(["b", "a", "c"]);
    await waitFor(() => expect(names()[0]).toBe("applications.use Premium"));
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.metrics.rateMultiplier",
      }),
    );
    expect(state.setOrder).toHaveBeenLastCalledWith(["a", "b", "c"]);
    expect(state.select).not.toHaveBeenCalled();
  });
  it("restores the previous order when saving the priority fails", async () => {
    state.setOrder.mockRejectedValueOnce(new Error("save failed"));
    render(<ApplicationWorkspace {...props} />);
    await userEvent.click(
      screen.getByRole("button", {
        name: "applications.metrics.rateMultiplier",
      }),
    );
    await waitFor(() => expect(names()[0]).toBe("applications.use Standard"));
    expect(state.select).not.toHaveBeenCalled();
  });
  it("sorts balance high first and retains all rows", async () => {
    render(<ApplicationWorkspace {...props} />);
    await userEvent.click(
      screen.getByRole("button", { name: "applications.metrics.balanceUsd" }),
    );
    expect(state.setOrder).toHaveBeenCalledWith(["b", "a", "c"]);
    expect(names()).toHaveLength(3);
  });
  it("retains skipped tiers with a visible reason and manual action", async () => {
    state.routing.tiers[0].skipReason = "circuit_open";
    render(<ApplicationWorkspace {...props} />);
    expect(
      screen.getByText("applications.skipReasons.circuit_open"),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "applications.use Standard" }),
    ).toBeEnabled();
  });
  it("shows a paused notice when failover is configured but routing is inactive", async () => {
    state.routing.autoFailoverEnabled = true;
    state.routing.routingActive = false;
    render(<ApplicationWorkspace {...props} />);
    expect(screen.getByText("applications.routingPaused")).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.resumeRouting" }),
    );
    expect(state.setFailover).toHaveBeenCalledWith(true);
    expect(state.select).not.toHaveBeenCalled();
  });
  it("puts failover beside the tier heading and changes only fallback permission", async () => {
    render(<ApplicationWorkspace {...props} />);
    await userEvent.click(
      screen.getByRole("switch", { name: "applications.autoFailover" }),
    );
    expect(state.setFailover).toHaveBeenCalledWith(true);
    expect(state.select).not.toHaveBeenCalled();
  });
  it("keeps account maintenance and additive configuration actions accessible", async () => {
    state.data.isAdditive = true;
    render(<ApplicationWorkspace {...props} />);
    expect(
      screen.getByRole("button", { name: "applications.enable Premium" }),
    ).toBeEnabled();
    await userEvent.click(
      screen.getAllByRole("button", { name: "applications.manageAccount" })[0],
    );
    expect(props.onOpenAccount).toHaveBeenCalledWith({ kind: "relay", id: 7 });
    await userEvent.click(
      screen.getByRole("button", { name: "applications.manageConfigurations" }),
    );
    expect(screen.getByText("Advanced configuration actions")).toBeVisible();
  });
});
