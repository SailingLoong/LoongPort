import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, it, expect, vi } from "vitest";
import { ApplicationWorkspace } from "../ApplicationWorkspace";
import { reorderWithinVisible } from "../ApplicationTierTable";

const state = vi.hoisted(() => ({
  data: {} as any,
  routing: {} as any,
  select: vi.fn(),
  setOrder: vi.fn(),
  setFailover: vi.fn(),
  blockTier: vi.fn(),
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
  state.blockTier.mockResolvedValue(undefined);
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
      // 优先级列与屏蔽按钮只在故障切换开启时出现（2026-09-16 三隐定调）。
      expect(
        Boolean(
          screen.queryByRole("columnheader", {
            name: "applications.priority",
          }),
        ),
      ).toBe(enabled);
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
  it("keeps row actions hover-revealed instead of always visible", async () => {
    render(<ApplicationWorkspace {...props} />);
    // 信息常驻、动作按需出现（2026-09-13 定调）：未悬停的行不摆一排
    // 「设为当前」按钮；组仍可聚焦（键盘可达）与点按（opacity 不摘出 DOM）。
    const use = await screen.findByRole("button", {
      name: "applications.use Premium",
    });
    const group = use.closest("div");
    expect(group?.className).toContain("opacity-0");
    expect(group?.className).toContain("group-hover:opacity-100");
    expect(group?.className).toContain("group-focus-within:opacity-100");
    expect(group?.className).toContain("[@media(hover:none)]:opacity-100");
  });

  it("searches immediately without changing priority or current selection", async () => {
    state.routing.autoFailoverEnabled = true;
    render(<ApplicationWorkspace {...props} />);
    await userEvent.type(screen.getByRole("searchbox"), "Premium");
    expect(screen.queryByText("Standard")).not.toBeInTheDocument();
    const row = screen.getByText("Premium").closest("tr")!;
    // 筛选视图优先级 = 可见行局部序，从 1 连续编号（2026-09-16 用户定调，无断层）。
    expect(within(row).getByText("1")).toBeVisible();
    expect(state.setOrder).not.toHaveBeenCalled();
    expect(state.select).not.toHaveBeenCalled();
  });
  it("sorts by the metric's default direction without persisting, reverses on second click, restores on third", async () => {
    state.routing.autoFailoverEnabled = true;
    render(<ApplicationWorkspace {...props} />);
    const rateHeader = screen.getByRole("button", {
      name: "applications.metrics.rateMultiplier",
    });
    // 第一击：默认向（倍率升序），视图排序、不落库。
    await userEvent.click(rateHeader);
    expect(state.setOrder).not.toHaveBeenCalled();
    await waitFor(() => expect(names()[0]).toBe("applications.use Premium"));
    // 优先级列永远按列表顺序连续编号：排序后 Premium 排第一就是 1。
    const premiumRow = screen.getByText("Premium").closest("tr")!;
    expect(within(premiumRow).getByText("1")).toBeVisible();
    // 第二击：反向（降序），箭头翻转。
    await userEvent.click(rateHeader);
    expect(state.setOrder).not.toHaveBeenCalled();
    await waitFor(() => expect(names()[0]).toBe("applications.use Standard"));
    // 第三击：取消排序，回到数据库档位序。
    await userEvent.click(rateHeader);
    await waitFor(() => expect(names()[0]).toBe("applications.use Standard"));
    expect(names().join()).toContain("Standard");
    expect(names()).toEqual([
      "applications.use Standard",
      "applications.use Premium",
      "applications.use Unknown",
    ]);
    expect(state.select).not.toHaveBeenCalled();
  });
  it("switching to another metric drops the previous sort and applies that metric's default direction", async () => {
    render(<ApplicationWorkspace {...props} />);
    // 余额默认降序：Premium(50) 在前。
    await userEvent.click(
      screen.getByRole("button", { name: "applications.metrics.balanceUsd" }),
    );
    await waitFor(() => expect(names()[0]).toBe("applications.use Premium"));
    // 换错误率（默认升序）：旧排序就地取消，按错误率排，Standard(0.02) 仍在前？
    // a=0.02、b=0.01 → 升序 b 在前。
    await userEvent.click(
      screen.getByRole("button", { name: "applications.metrics.errorRate" }),
    );
    await waitFor(() => expect(names()[0]).toBe("applications.use Premium"));
    expect(state.setOrder).not.toHaveBeenCalled();
    expect(names()).toHaveLength(3);
  });
  it("shows the sort arrow only on the active metric column", async () => {
    render(<ApplicationWorkspace {...props} />);
    const header = (name: string) =>
      screen.getByRole("button", { name }).querySelector("svg");
    // 默认无任何方向箭头。
    for (const key of [
      "rateMultiplier",
      "errorRate",
      "avgFirstTokenMs",
      "balanceUsd",
    ]) {
      expect(header(`applications.metrics.${key}`)).toBeNull();
    }
    await userEvent.click(
      screen.getByRole("button", { name: "applications.metrics.errorRate" }),
    );
    expect(header("applications.metrics.errorRate")).not.toBeNull();
    expect(header("applications.metrics.balanceUsd")).toBeNull();
  });
  it("counts only detected errors (and blocks) as unavailable, never model incompatibility", async () => {
    state.routing.model = "gpt-5";
    state.routing.routingActive = true;
    state.routing.tiers[0].effectiveModel = "gpt-5";
    state.routing.tiers[1].effectiveModel = "grok-4.6";
    state.routing.tiers[2].effectiveModel = "gpt-5";
    // gpt-5: a 可用、c 限流中（circuit_open=已嗅探错误）→ 1/2 部分可用，分子橙；
    // grok-4.6: b 标 model_incompatible —— 对当前模型 gpt-5 恒真、不是错误，
    // 不得扣分（beta.4 回归：曾把其他模型分子全清零）→ 1/1 全可用，分子绿。
    state.routing.tiers[1].skipReason = "model_incompatible";
    state.routing.tiers[2].skipReason = "circuit_open";
    render(<ApplicationWorkspace {...props} />);
    const filter = screen.getByRole("combobox", {
      name: "applications.modelFilter",
    });
    await userEvent.click(filter);
    const optionElements = screen.getAllByRole("option");
    const options = optionElements.map((option) => option.textContent);
    expect(options[0]).toBe("applications.allModels");
    expect(options[1]).toContain("⚡ gpt-5");
    expect(options[1]).toContain("1/2");
    expect(optionElements[1].querySelector(".text-orange-500")).not.toBeNull();
    const grok = optionElements.find((option) =>
      option.textContent?.includes("grok-4.6"),
    )!;
    expect(grok.textContent).toContain("1/1");
    expect(grok.querySelector(".text-green-600")).not.toBeNull();
    expect(options.some((text) => text?.includes("0/"))).toBe(false);
    // 过滤行为：选 gpt-5 只留 effectiveModel=gpt-5 的行（含限流中的，看原因）。
    await userEvent.click(screen.getByRole("option", { name: /⚡ gpt-5/ }));
    expect(screen.getByText("Standard")).toBeVisible();
    expect(screen.getByText("Unknown")).toBeVisible();
    expect(screen.queryByText("Premium")).not.toBeInTheDocument();
    expect(state.setOrder).not.toHaveBeenCalled();
  });
  it("blocked tiers gray out, free their priority number, and unblock from the row action", async () => {
    state.routing.autoFailoverEnabled = true;
    state.routing.tiers[1].skipReason = "blocked";
    render(<ApplicationWorkspace {...props} />);
    const premiumRow = screen.getByText("Premium").closest("tr")!;
    // 整行置灰；被屏蔽的行不占优先级号，下一行顶上。
    expect(premiumRow.className).toContain("opacity-55");
    // 优先级格 = 行首单元格（故障切换开启时）；指标列的「—」不参与此断言。
    expect(within(premiumRow.cells[0]!).getByText("—")).toBeVisible();
    const standardRow = screen.getByText("Standard").closest("tr")!;
    const unknownRow = screen.getByText("Unknown").closest("tr")!;
    expect(within(standardRow).getByText("1")).toBeVisible();
    expect(within(unknownRow).getByText("2")).toBeVisible();
    // 屏蔽只挡自动切换：手动「设为当前」仍在。
    expect(
      within(premiumRow).getByRole("button", {
        name: "applications.use Premium",
      }),
    ).toBeVisible();
    // 取消屏蔽走行动作（即时生效，不经「应用此顺序」）。
    await userEvent.click(
      within(premiumRow).getByRole("button", {
        name: "applications.unblockTier",
      }),
    );
    expect(state.blockTier).toHaveBeenCalledWith({
      providerId: "b",
      blocked: false,
    });
    // 屏蔽一个正常档位同理。
    await userEvent.click(
      within(standardRow).getByRole("button", {
        name: "applications.blockTier",
      }),
    );
    expect(state.blockTier).toHaveBeenCalledWith({
      providerId: "a",
      blocked: true,
    });
  });
  it("filters tiers by account and shows all again from the dropdown", async () => {
    state.data.configurations = [
      config("a", "Standard", true),
      {
        ...config("b", "Premium"),
        account: { kind: "relay", id: 9 },
        accountLabel: "Team",
      },
      config("c", "Unknown"),
    ];
    render(<ApplicationWorkspace {...props} />);
    const filter = screen.getByRole("combobox", {
      name: "applications.accountFilter",
    });
    await userEvent.click(filter);
    await userEvent.click(
      screen.getByRole("option", { name: "Example service · Team" }),
    );
    expect(screen.queryByText("Standard")).not.toBeInTheDocument();
    expect(screen.getByText("Premium")).toBeVisible();
    await userEvent.click(filter);
    await userEvent.click(
      screen.getByRole("option", { name: "applications.allAccounts" }),
    );
    expect(screen.getByText("Standard")).toBeVisible();
    expect(screen.getByText("Unknown")).toBeVisible();
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

describe("reorderWithinVisible (splice semantics)", () => {
  it("swaps only the visible rows and leaves hidden rows in place", () => {
    const fn = reorderWithinVisible;
    // 全序 [A,B,C,D,E]，筛选只显示 B,D；把 D 拖到 B 前 → [A,D,C,B,E]。
    expect(fn(["A", "B", "C", "D", "E"], ["B", "D"], 1, 0)).toEqual([
      "A",
      "D",
      "C",
      "B",
      "E",
    ]);
    // 未筛选时等价整体换位。
    expect(fn(["A", "B", "C"], ["A", "B", "C"], 2, 0)).toEqual(["C", "A", "B"]);
    // 非法下标原样返回。
    expect(fn(["A", "B"], ["A", "B"], -1, 0)).toEqual(["A", "B"]);
  });
});
