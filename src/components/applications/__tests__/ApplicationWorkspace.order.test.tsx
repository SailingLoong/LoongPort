import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";

const state = vi.hoisted(() => ({
  data: {} as any,
  routing: {} as any,
  select: vi.fn(),
  apply: vi.fn(),
  setOrder: vi.fn(),
  setFailover: vi.fn(),
  blockTier: vi.fn(),
}));

// 表格子组件打桩：直接拿 onReorder 模拟拖拽产物（splice 语义在主测试文件
// 纯函数覆盖），这里专测工作台的「拖拽暂存 → 应用此顺序」状态机。
// visibleTierIds 是可见性唯源（工作台算应用目标也用它），mock 里保留真实现。
const tableProps = vi.hoisted(() => ({ current: null as any }));
vi.mock("../ApplicationTierTable", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../ApplicationTierTable")>();
  return {
    ...actual,
    ApplicationTierTable: (props: unknown) => {
      tableProps.current = props;
      return <table aria-label="tier table" />;
    },
  };
});
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
    apply: state.apply,
    setFailover: state.setFailover,
    blockTier: state.blockTier,
  }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const profilesApi = vi.hoisted(() => ({
  saved: [] as { name: string; providerIds: string[] }[],
  current: "default",
  saveCalls: [] as { appType: string; name: string; ids: string[] }[],
  setCurrent: vi.fn(),
  rename: vi.fn(),
}));
vi.mock("@/lib/api/orderProfiles", () => ({
  orderProfilesApi: {
    list: vi.fn(() =>
      Promise.resolve({
        profiles: profilesApi.saved,
        current: profilesApi.current,
      }),
    ),
    save: vi.fn(
      (appType: string, name: string, providerIds: string[]) =>
        new Promise<void>((resolve) => {
          profilesApi.saveCalls.push({ appType, name, ids: providerIds });
          profilesApi.saved.push({ name, providerIds });
          resolve();
        }),
    ),
    setCurrent: (appType: string, name: string): Promise<void> => {
      profilesApi.setCurrent(appType, name);
      profilesApi.current = name;
      return Promise.resolve();
    },
    rename: (appType: string, from: string, to: string): Promise<void> => {
      profilesApi.rename(appType, from, to);
      return Promise.resolve();
    },
    remove: vi.fn(
      (_appType: string, name: string) =>
        new Promise<void>((resolve) => {
          profilesApi.saved = profilesApi.saved.filter((p) => p.name !== name);
          resolve();
        }),
    ),
    import: vi.fn(() => Promise.resolve(null)),
    export: vi.fn(() => Promise.resolve(null)),
  },
}));
vi.mock("@/components/relay/SwitchTierConfirmDialog", () => ({
  SwitchTierConfirmDialog: ({ targetName, onCancel, onSwitch }: any) =>
    targetName ? (
      <div role="dialog">
        <button onClick={onCancel}>Cancel switch</button>
        <button onClick={() => onSwitch(false)}>Confirm switch</button>
      </div>
    ) : null,
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
// describe 级共享同一个 client：rerender 换新 client 会重挂全部查询。
let queryClient: QueryClient;
const workspaceElement = () => (
  <QueryClientProvider client={queryClient}>
    <ApplicationWorkspace {...props} />
  </QueryClientProvider>
);
const renderWorkspace = () => render(workspaceElement());
const rerenderWorkspace = (view: ReturnType<typeof renderWorkspace>) =>
  view.rerender(workspaceElement());

const drag = (ids: string[]) =>
  act(async () => {
    tableProps.current.onReorder(ids);
  });

describe("failover order staging", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    queryClient = new QueryClient();
    state.setOrder.mockResolvedValue(undefined);
    state.apply.mockResolvedValue({
      status: "switched",
      providerName: "Example",
      warnings: [],
      chatgptWasRunning: false,
      chatgptRelaunched: false,
    });
    state.setFailover.mockResolvedValue(undefined);
    state.blockTier.mockResolvedValue(undefined);
    profilesApi.saved = [];
    profilesApi.current = "default";
    profilesApi.saveCalls = [];
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
      autoFailoverEnabled: true,
      chainIds: ["a", "b", "c"],
      tiers: [
        {
          providerId: "a",
          position: 0,
          skipReason: null,
          rateMultiplier: 2,
          canFailover: true,
          canVerifyModels: true,
          models: [],
        },
        {
          providerId: "b",
          position: 1,
          skipReason: null,
          rateMultiplier: 1,
          canFailover: true,
          canVerifyModels: true,
          models: [],
        },
        {
          providerId: "c",
          position: 2,
          skipReason: null,
          rateMultiplier: 3,
          canFailover: true,
          canVerifyModels: true,
          models: [],
        },
      ],
    };
  });

  it("submits a reordered chain and its profile together, and converges after readback", async () => {
    const view = renderWorkspace();
    await waitFor(() =>
      expect(
        queryClient.getQueryData(["orderProfiles", "codex"]),
      ).toBeDefined(),
    );
    await drag(["c", "a", "b"]);
    expect(state.apply).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(state.apply).toHaveBeenCalledWith(
      { order: { profileName: "default", providerIds: ["c", "a", "b"] } },
      undefined,
    );
    expect(profilesApi.saveCalls).toEqual([]);
    state.routing = {
      ...state.routing,
      chainIds: ["c", "a", "b"],
      tiers: [
        state.routing.tiers[2],
        state.routing.tiers[0],
        state.routing.tiers[1],
      ],
    };
    rerenderWorkspace(view);
    expect(
      screen.queryByText("applications.pendingChanges"),
    ).not.toBeInTheDocument();
  });

  it("loads a profile locally and immediately applies to that exact profile", async () => {
    profilesApi.saved = [{ name: "Travel", providerIds: ["c", "b"] }];
    renderWorkspace();
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: /Travel/ }),
    );
    expect(profilesApi.setCurrent).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(state.apply).toHaveBeenCalledWith(
      { order: { profileName: "Travel", providerIds: ["c", "b"] } },
      undefined,
    );
    expect(profilesApi.saveCalls).toEqual([]);
  });

  it("keeps a partial applied list after refresh and exposes all tiers explicitly", async () => {
    state.routing.chainIds = ["b", "c"];
    const view = renderWorkspace();
    expect(tableProps.current.orderedIds).toEqual(["b", "c"]);
    expect(
      screen.queryByText("applications.pendingChanges"),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.showAllTiers" }),
    );
    expect(tableProps.current.orderedIds).toEqual(["a", "b", "c"]);
    await userEvent.click(
      screen.getByRole("button", { name: "applications.discardOrder" }),
    );
    state.routing = { ...state.routing };
    rerenderWorkspace(view);
    expect(tableProps.current.orderedIds).toEqual(["b", "c"]);
  });

  it("allows cancelling an empty profile draft without changing the current profile", async () => {
    profilesApi.saved = [{ name: "Other device", providerIds: ["unknown"] }];
    renderWorkspace();
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: /Other device/ }),
    );
    expect(tableProps.current.orderedIds).toEqual([]);
    expect(screen.getByText("applications.emptyChain")).toBeVisible();
    expect(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    ).toBeDisabled();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.discardOrder" }),
    );
    expect(tableProps.current.orderedIds).toEqual(["a", "b", "c"]);
    expect(profilesApi.setCurrent).not.toHaveBeenCalled();
  });

  it("applies a model-only change even when all chain members support the model", async () => {
    state.routing.model = "model-a";
    state.routing.tiers.forEach((tier: any) => {
      tier.models = ["model-a", "model-b"];
      tier.effectiveModel = "model-a";
    });
    renderWorkspace();
    await userEvent.click(
      screen.getByRole("combobox", { name: "applications.modelFilter" }),
    );
    await userEvent.click(screen.getByRole("option", { name: /model-b/ }));
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(state.apply).toHaveBeenCalledWith(
      {
        order: { profileName: "default", providerIds: ["a", "b", "c"] },
        selection: { providerId: "a", model: "model-b" },
      },
      undefined,
    );
    expect(state.select).not.toHaveBeenCalled();
  });

  it.each(["relay", "provider", "vendor"])(
    "passes the selected model for %s configurations",
    async (kind) => {
      state.data.configurations[1].selection = { kind };
      state.routing.tiers.forEach((tier: any) => {
        tier.models = ["model-a", "model-b"];
        tier.effectiveModel = "model-a";
      });
      renderWorkspace();
      await userEvent.click(
        screen.getByRole("combobox", { name: "applications.modelFilter" }),
      );
      await userEvent.click(screen.getByRole("option", { name: /model-b/ }));
      await act(async () => {
        tableProps.current.onSelect(state.data.configurations[1]);
      });
      expect(state.apply).toHaveBeenCalledWith(
        { selection: { providerId: "b", model: "model-b" } },
        undefined,
      );
    },
  );

  it("holds the complete change through confirmation and cancellation writes nothing else", async () => {
    state.apply.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Example",
    });
    renderWorkspace();
    await drag(["b", "a", "c"]);
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(screen.getByRole("dialog")).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "Cancel switch" }),
    );
    expect(state.apply).toHaveBeenCalledTimes(1);
    expect(profilesApi.saveCalls).toEqual([]);
    expect(tableProps.current.orderedIds).toEqual(["b", "a", "c"]);
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Confirm switch" }),
    );
    expect(state.apply).toHaveBeenLastCalledWith(
      { order: { profileName: "default", providerIds: ["b", "a", "c"] } },
      false,
    );
  });

  it("retains the draft on failure", async () => {
    state.apply.mockRejectedValue(new Error("Cannot update"));
    renderWorkspace();
    await drag(["b", "a", "c"]);
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(tableProps.current.orderedIds).toEqual(["b", "a", "c"]);
    expect(screen.getByText("applications.pendingChanges")).toBeVisible();
  });

  it("discards chain drafts when failover is disabled", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    state.routing = { ...state.routing, autoFailoverEnabled: false };
    rerenderWorkspace(view);
    expect(
      screen.queryByText("applications.pendingChanges"),
    ).not.toBeInTheDocument();
    state.routing = { ...state.routing, autoFailoverEnabled: true };
    rerenderWorkspace(view);
    expect(tableProps.current.orderedIds).toEqual(["a", "b", "c"]);
  });

  it("keeps a staged draft across routing refresh", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    state.routing = { ...state.routing };
    rerenderWorkspace(view);
    expect(tableProps.current.orderedIds).toEqual(["b", "a", "c"]);
  });
  it("saving as a new profile creates a draft without replacing the applied profile", async () => {
    renderWorkspace();
    await drag(["c", "b", "a"]);
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(
      screen.getByRole("menuitem", {
        name: "applications.orderProfileSaveCurrent",
      }),
    );
    await userEvent.type(
      screen.getByRole("textbox", {
        name: "applications.orderProfileNamePlaceholder",
      }),
      "Travel",
    );
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(screen.getByTitle("applications.orderProfiles")).toHaveTextContent(
      "Travel",
    );
    expect(state.apply).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(state.apply).toHaveBeenCalledWith(
      { order: { profileName: "Travel", providerIds: ["c", "b", "a"] } },
      undefined,
    );
  });
  it("explains why a model-filtered draft cannot be applied when no tier is available", async () => {
    state.routing.tiers[0].effectiveModel = "model-a";
    state.routing.tiers[1].effectiveModel = "model-b";
    state.routing.tiers[1].skipReason = "circuit_open";
    renderWorkspace();
    await userEvent.click(
      screen.getByRole("combobox", { name: "applications.modelFilter" }),
    );
    await userEvent.click(screen.getByRole("option", { name: /model-b/ }));
    expect(screen.getByText("applications.noSwitchableTier")).toBeVisible();
    expect(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    ).toBeDisabled();
    expect(state.apply).not.toHaveBeenCalled();
  });
  it("converges after applying a partial profile and reading the backend result", async () => {
    profilesApi.saved = [{ name: "Travel", providerIds: ["c", "b"] }];
    const view = renderWorkspace();
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(
      await screen.findByRole("menuitem", { name: /Travel/ }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    state.routing = {
      ...state.routing,
      chainIds: ["c", "b"],
      tiers: [
        state.routing.tiers[2],
        state.routing.tiers[1],
        state.routing.tiers[0],
      ],
    };
    profilesApi.current = "Travel";
    rerenderWorkspace(view);
    expect(tableProps.current.orderedIds).toEqual(["c", "b"]);
    expect(
      screen.queryByText("applications.pendingChanges"),
    ).not.toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.showAllTiers" }),
    );
    expect(tableProps.current.orderedIds).toEqual(["c", "b", "a"]);
  });
  it("does not report a change after dragging back to the applied order", async () => {
    renderWorkspace();
    await drag(["b", "a", "c"]);
    expect(screen.getByText("applications.pendingChanges")).toBeVisible();
    await drag(["a", "b", "c"]);
    expect(
      screen.queryByText("applications.pendingChanges"),
    ).not.toBeInTheDocument();
  });
  it("preserves the selected model when saving the current draft under a new name", async () => {
    state.routing.model = "model-a";
    state.routing.tiers.forEach((tier: any) => {
      tier.models = ["model-a", "model-b"];
      tier.effectiveModel = "model-a";
    });
    renderWorkspace();
    await userEvent.click(
      screen.getByRole("combobox", { name: "applications.modelFilter" }),
    );
    await userEvent.click(screen.getByRole("option", { name: /model-b/ }));
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(
      screen.getByRole("menuitem", {
        name: "applications.orderProfileSaveCurrent",
      }),
    );
    await userEvent.type(
      screen.getByRole("textbox", {
        name: "applications.orderProfileNamePlaceholder",
      }),
      "Travel",
    );
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    await userEvent.click(
      screen.getByRole("button", { name: /applications.applyOrder/ }),
    );
    expect(state.apply).toHaveBeenCalledWith(
      {
        order: { profileName: "Travel", providerIds: ["a", "b", "c"] },
        selection: { providerId: "a", model: "model-b" },
      },
      undefined,
    );
  });
});
