import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";

const state = vi.hoisted(() => ({ data: {} as any, select: vi.fn() }));
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
describe("application workspace", () => {
  it("shows current configuration before collapsed choices and searches within accounts", async () => {
    state.data = {
      configurations: [config("a", "Standard", true), config("b", "Premium")],
      recentProviderIds: ["b"],
      isAdditive: false,
    };
    render(<ApplicationWorkspace {...props} />);
    expect(screen.getByText("Standard")).toBeVisible();
    expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.switchService" }),
    );
    expect(screen.getByRole("searchbox")).toHaveFocus();
    await userEvent.type(screen.getByRole("searchbox"), "Premium");
    expect(
      screen.getByRole("button", { name: "applications.use Premium" }),
    ).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.use Premium" }),
    );
    expect(state.select).toHaveBeenCalledWith(
      expect.objectContaining({ providerId: "b" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "common.back" }));
    expect(screen.getByText("Standard")).toBeVisible();
  });
  it("keeps additive configured entries and exact account management accessible", async () => {
    state.data = {
      configurations: [config("a", "Standard"), config("b", "Premium")],
      recentProviderIds: [],
      isAdditive: true,
    };
    render(<ApplicationWorkspace {...props} />);
    expect(screen.getByText("Standard")).toBeVisible();
    expect(screen.getByText("Premium")).toBeVisible();
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
