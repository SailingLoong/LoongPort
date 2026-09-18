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
          profilesApi.current = name;
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
    // 计数 = 应用目标的大小（链里将有 3 个）。
    expect(apply).toHaveTextContent("(3)");
    await userEvent.click(apply);
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["c", "a", "b"]),
    );
    // 应用即保存进当前配置档（2026-09-17 定调：default 是落点）。
    await waitFor(() =>
      expect(profilesApi.saveCalls).toContainEqual({
        appType: "codex",
        name: "default",
        ids: ["c", "a", "b"],
      }),
    );
    // 没选模型筛选的应用不碰当前档（调序 ≠ 换用途）。
    expect(state.select).not.toHaveBeenCalled();
    // 模拟真实链路的应用后刷新：routing 查询换新对象、链与 tiers 序=已应用序，
    // 乐观快照随之失效、待应用归零、按钮消失。
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
    await waitFor(() => {
      expect(tableProps.current.orderedIds).toEqual(["c", "a", "b"]);
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
    });
    view.unmount();
  });

  it("hides chain editing entirely when failover is off (order has no runtime effect)", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    expect(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    ).toBeInTheDocument();
    // 关掉故障切换：顺序与配置档没有任何运行时作用（2026-09-17 定调）——
    // 整套链编辑面消失，未应用草稿随之丢弃。
    state.routing.autoFailoverEnabled = false;
    rerenderWorkspace(view);
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: /applications\.discardOrder/ }),
      ).not.toBeInTheDocument();
      expect(screen.queryByTitle("applications.orderProfiles")).toBeNull();
    });
    expect(state.setOrder).not.toHaveBeenCalled();
    // 重新打开：无幽灵待应用（草稿已在关闭时丢弃），配置档回来。
    state.routing.autoFailoverEnabled = true;
    rerenderWorkspace(view);
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
      expect(screen.getByTitle("applications.orderProfiles")).toBeVisible();
    });
    expect(state.setOrder).not.toHaveBeenCalled();
    view.unmount();
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
    // 应用后临时排序与暂存一起清空；模拟刷新后链与显示序=已应用的排序序。
    state.routing = {
      ...state.routing,
      chainIds: ["b", "a", "c"],
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

  it("discarding drops staged drags, sorting and filters back to the stored chain", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    await act(async () => {
      tableProps.current.onSort("rateMultiplier");
    });
    await userEvent.type(
      screen.getByRole("searchbox", { name: "applications.search" }),
      "Premium",
    );
    expect(
      screen.getByRole("button", { name: /applications\.discardOrder/ }),
    ).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.discardOrder/ }),
    );
    await waitFor(() => {
      expect(tableProps.current.sort).toBeNull();
      expect(tableProps.current.search).toBe("");
      expect(tableProps.current.orderedIds).toEqual(["a", "b", "c"]);
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
    });
    expect(state.setOrder).not.toHaveBeenCalled();
    view.unmount();
  });

  it("lights Apply from filtering alone and narrows the chain to the visible tiers", async () => {
    const view = renderWorkspace();
    // 筛选（搜索）一变目标就变：可见只剩 b，不需要先拖一下。
    await userEvent.type(
      screen.getByRole("searchbox", { name: "applications.search" }),
      "Premium",
    );
    const apply = screen.getByRole("button", {
      name: /applications\.applyOrder/,
    });
    expect(apply).toHaveTextContent("(1)");
    await userEvent.click(apply);
    // 应用写入 = 可见 ∧ 未屏蔽的显示序——链收窄为 [b]，其余档位出链（不是后备）。
    await waitFor(() => expect(state.setOrder).toHaveBeenCalledWith(["b"]));
    view.unmount();
  });

  it("blocked tiers are excluded from the applied chain and blocking alone does not nag", async () => {
    state.routing.tiers[2].skipReason = "blocked";
    const view = renderWorkspace();
    // 屏蔽即时生效且不制造待应用：目标与参照都剔除 c，两者一致 → 无按钮。
    expect(
      screen.queryByRole("button", { name: /applications\.applyOrder/ }),
    ).not.toBeInTheDocument();
    // 拖拽后应用：写入的目标不含被屏蔽的 c。
    await drag(["b", "a", "c"]);
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["b", "a"]),
    );
    view.unmount();
  });

  it("chain ghosts from upstream deletions light Apply until the user re-applies", async () => {
    state.routing.chainIds = ["a", "b", "c", "ghost"];
    const view = renderWorkspace();
    // 幽灵不在视图里（configurations 没有它）→ 目标 [a,b,c] ≠ 参照 [a,b,c,ghost]。
    const apply = screen.getByRole("button", {
      name: /applications\.applyOrder/,
    });
    expect(apply).toHaveTextContent("(3)");
    await userEvent.click(apply);
    // 应用即清理幽灵。
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["a", "b", "c"]),
    );
    view.unmount();
  });

  it("loads an order profile into the draft (filtered to known tiers, no padding), then applies", async () => {
    profilesApi.saved = [
      { name: "便宜优先", providerIds: ["c", "ghost", "b"] },
    ];
    const view = renderWorkspace();
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(screen.getByRole("menuitem", { name: /便宜优先/ }));
    // 载入 = 进草稿 + 切换当前配置档：认不出的 id 滤掉、不垫底——链外档位（a）从视图消失。
    expect(tableProps.current.orderedIds).toEqual(["c", "b"]);
    expect(profilesApi.setCurrent).toHaveBeenCalledWith("codex", "便宜优先");
    expect(state.setOrder).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    // 应用写入就是档内这批——链 = [c,b]，a 出链；落进切换后的当前档。
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["c", "b"]),
    );
    await waitFor(() =>
      expect(profilesApi.saveCalls).toContainEqual({
        appType: "codex",
        name: "便宜优先",
        ids: ["c", "b"],
      }),
    );
    view.unmount();
  });

  it("renames a profile from the row action", async () => {
    profilesApi.saved = [{ name: "便宜优先", providerIds: ["c", "b"] }];
    const view = renderWorkspace();
    await userEvent.click(screen.getByTitle("applications.orderProfiles"));
    await userEvent.click(
      screen.getByRole("button", { name: "applications.orderProfileRename" }),
    );
    const input = screen.getByRole("textbox", {
      name: "applications.orderProfileNamePlaceholder",
    });
    await userEvent.clear(input);
    await userEvent.type(input, "快的优先");
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(profilesApi.rename).toHaveBeenCalledWith(
        "codex",
        "便宜优先",
        "快的优先",
      ),
    );
    view.unmount();
  });

  it("applying with a model filter switches current to the first available tier of that model", async () => {
    // a(当前)=gpt-4、b=gpt-5、c=gpt-4：选 gpt-5 后应用 = 切到 b，
    // 走标准切换编排（select → 确认框 → 退 ChatGPT → 切 → 重开）。
    state.routing.tiers[0].effectiveModel = "gpt-4";
    state.routing.tiers[1].effectiveModel = "gpt-5";
    state.routing.tiers[2].effectiveModel = "gpt-4";
    const view = renderWorkspace();
    await userEvent.click(
      screen.getByRole("combobox", { name: "applications.modelFilter" }),
    );
    await userEvent.click(screen.getByRole("option", { name: /gpt-5/ }));
    const apply = screen.getByRole("button", {
      name: /applications\.applyOrder/,
    });
    expect(apply).toHaveTextContent("(1)");
    await userEvent.click(apply);
    await waitFor(() => expect(state.setOrder).toHaveBeenCalledWith(["b"]));
    await waitFor(() =>
      expect(state.select).toHaveBeenCalledWith(
        expect.objectContaining({ providerId: "b" }),
        undefined,
        // 模型筛选下切换把模型一起带过去（switchTierModel 链）。
        "gpt-5",
      ),
    );
    view.unmount();
  });

  it("applying with a model filter keeps current when it already serves that model", async () => {
    state.routing.tiers[0].effectiveModel = "gpt-4";
    state.routing.tiers[1].effectiveModel = "gpt-5";
    state.routing.tiers[2].effectiveModel = "gpt-4";
    const view = renderWorkspace();
    await userEvent.click(
      screen.getByRole("combobox", { name: "applications.modelFilter" }),
    );
    await userEvent.click(screen.getByRole("option", { name: /gpt-4/ }));
    // 当前档 a 就在应用目标里（它服务 gpt-4）→ 不折腾、零打扰。
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["a", "c"]),
    );
    expect(state.select).not.toHaveBeenCalled();
    view.unmount();
  });

  it("applying with a model filter skips circuit-open tiers and does not switch when none is available", async () => {
    state.routing.tiers[0].effectiveModel = "gpt-4";
    state.routing.tiers[1].effectiveModel = "gpt-5";
    state.routing.tiers[2].effectiveModel = "gpt-4";
    // gpt-5 只有一个档位且正熔断 → 链照常应用，但不切换（熔断档位不算可用）。
    state.routing.tiers[1].skipReason = "circuit_open";
    const view = renderWorkspace();
    await userEvent.click(
      screen.getByRole("combobox", { name: "applications.modelFilter" }),
    );
    await userEvent.click(screen.getByRole("option", { name: /gpt-5/ }));
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() => expect(state.setOrder).toHaveBeenCalledWith(["b"]));
    expect(state.select).not.toHaveBeenCalled();
    view.unmount();
  });

  it("saves the apply target (visible and unblocked) as a named profile and switches to it", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
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
      "快的优先",
    );
    await userEvent.click(screen.getByRole("button", { name: "common.save" }));
    await waitFor(() =>
      expect(profilesApi.saved).toEqual([
        { name: "快的优先", providerIds: ["b", "a", "c"] },
      ]),
    );
    // 另存为新档 = 切换过去（后端 save 设当前，mock 同步 current）。
    expect(profilesApi.current).toBe("快的优先");
    view.unmount();
  });

  it("staged draft survives provider refresh until applied", async () => {
    const view = renderWorkspace();
    await drag(["b", "a", "c"]);
    // 5s 轮询换新 routing 对象（引用变化）——草稿是独立 state，不随之丢失。
    state.routing = { ...state.routing };
    rerenderWorkspace(view);
    await userEvent.click(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    );
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["b", "a", "c"]),
    );
    view.unmount();
  });
});
