import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";

const state = vi.hoisted(() => ({
  data: {} as any,
  routing: {} as any,
  select: vi.fn(),
  setOrder: vi.fn(),
  setFailover: vi.fn(),
  blockTier: vi.fn(),
}));

// 表格子组件打桩：直接拿 onReorder 模拟拖拽产物（splice 语义在主测试文件
// 纯函数覆盖），这里专测工作台的「拖拽暂存 → 应用此顺序」状态机。
const tableProps = vi.hoisted(() => ({ current: null as any }));
vi.mock("../ApplicationTierTable", () => ({
  ApplicationTierTable: (props: unknown) => {
    tableProps.current = props;
    return <table aria-label="tier table" />;
  },
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
    blockTier: state.blockTier,
  }),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const profilesApi = vi.hoisted(() => ({
  saved: [] as { name: string; providerIds: string[] }[],
}));
vi.mock("@/lib/api/orderProfiles", () => ({
  orderProfilesApi: {
    list: vi.fn(() => Promise.resolve(profilesApi.saved)),
    save: vi.fn(
      (_appType: string, name: string, providerIds: string[]) =>
        new Promise<void>((resolve) => {
          profilesApi.saved.push({ name, providerIds });
          resolve();
        }),
    ),
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
    state.setFailover.mockResolvedValue(undefined);
    state.blockTier.mockResolvedValue(undefined);
    profilesApi.saved = [];
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
      tiers: [
        { providerId: "a", position: 0, skipReason: null, rateMultiplier: 2 },
        { providerId: "b", position: 1, skipReason: null, rateMultiplier: 1 },
        { providerId: "c", position: 2, skipReason: null, rateMultiplier: 3 },
      ],
    };
  });

  it("stages drags without persisting until Apply order is clicked", async () => {
    const view = renderWorkspace();
    expect(
      screen.queryByRole("button", { name: /applications\.applyOrder/ }),
    ).not.toBeInTheDocument();
    // 拖拽（表格回调）：c 提到最前 → 全序 [c,a,b]；只暂存、不落库。
    await drag(["c", "a", "b"]);
    expect(state.setOrder).not.toHaveBeenCalled();
    const apply = screen.getByRole("button", {
      name: /applications\.applyOrder/,
    });
    // 三行位置全变 → 待应用计数 3。
    expect(apply).toHaveTextContent("(3)");
    await userEvent.click(apply);
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["c", "a", "b"]),
    );
    // 模拟真实链路的应用后刷新：routing 查询换新对象、tiers 序=已应用序，
    // 乐观快照随之失效、待应用归零、按钮消失。
    state.routing = {
      ...state.routing,
      tiers: [
        state.routing.tiers[2],
        state.routing.tiers[0],
        state.routing.tiers[1],
      ],
    };
    rerenderWorkspace(view);
    await waitFor(() => {
      expect(tableProps.current.orderedIds).toEqual(["c", "a", "b"]);
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
    });
    view.unmount();
  });

  it("stages even when failover is off: only Apply persists (unified draft)", async () => {
    state.routing.autoFailoverEnabled = false;
    renderWorkspace();
    await drag(["b", "a", "c"]);
    // 草稿语义统一（2026-09-16 定调）：故障切换关着也一样——只暂存，不落库。
    expect(state.setOrder).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["b", "a", "c"]),
    );
  });

  it("offers Apply for a metric-sorted view and applies the displayed order", async () => {
    const view = renderWorkspace();
    // 倍率升序：b(1), a(2), c(3)——显示序不同于存储序 [a,b,c]。
    await act(async () => {
      tableProps.current.onSort("rateMultiplier");
    });
    const apply = await screen.findByRole("button", {
      name: /applications\.applyOrder/,
    });
    expect(tableProps.current.orderedIds).toEqual(["b", "a", "c"]);
    await userEvent.click(apply);
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["b", "a", "c"]),
    );
    // 应用后临时排序与暂存一起清空；模拟刷新后显示序=已应用的排序序。
    state.routing = {
      ...state.routing,
      tiers: [
        state.routing.tiers[1],
        state.routing.tiers[0],
        state.routing.tiers[2],
      ],
    };
    rerenderWorkspace(view);
    await waitFor(() => {
      expect(tableProps.current.sort).toBeNull();
      expect(tableProps.current.orderedIds).toEqual(["b", "a", "c"]);
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
    });
    view.unmount();
  });

  it("discarding drops staged drags and sorting back to the stored order", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    await act(async () => {
      tableProps.current.onSort("rateMultiplier");
    });
    expect(
      screen.getByRole("button", { name: /applications\.discardOrder/ }),
    ).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.discardOrder/ }),
    );
    await waitFor(() => {
      expect(tableProps.current.sort).toBeNull();
      expect(tableProps.current.orderedIds).toEqual(["a", "b", "c"]);
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
    });
    expect(state.setOrder).not.toHaveBeenCalled();
    view.unmount();
  });

  it("loads an order profile into the draft (filtered to known tiers, padded), then applies", async () => {
    profilesApi.saved = [
      { name: "便宜优先", providerIds: ["c", "ghost", "b"] },
    ];
    const view = renderWorkspace();
    await userEvent.click(
      screen.getByRole("button", { name: "applications.orderProfiles" }),
    );
    await userEvent.click(screen.getByRole("menuitem", { name: /便宜优先/ }));
    // 载入 = 进草稿：认不出的 id 滤掉、剩余档位按存储序垫底 → [c,b,a]。
    expect(tableProps.current.orderedIds).toEqual(["c", "b", "a"]);
    expect(state.setOrder).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["c", "b", "a"]),
    );
    view.unmount();
  });

  it("saves the displayed order as a named profile", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    await userEvent.click(
      screen.getByRole("button", { name: "applications.orderProfiles" }),
    );
    await userEvent.click(
      screen.getByRole("menuitem", {
        name: "applications.orderProfileSaveCurrent",
      }),
    );
    await userEvent.type(
      screen.getByRole("textbox", {
        name: "applications.orderProfileNamePlaceholder",
      }),
      "快的优先",
    );
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(profilesApi.saved).toEqual([
        { name: "快的优先", providerIds: ["b", "a", "c"] },
      ]),
    );
    view.unmount();
  });

  it("staged draft survives failover toggling until applied", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    state.routing.autoFailoverEnabled = false;
    rerenderWorkspace(view);
    // 开关切换不吞草稿：仍可应用。
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["b", "a", "c"]),
    );
    view.unmount();
  });
});
