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
const drag = (ids: string[]) =>
  act(async () => {
    tableProps.current.onReorder(ids);
  });

describe("failover order staging", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    state.setOrder.mockResolvedValue(undefined);
    state.setFailover.mockResolvedValue(undefined);
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
      autoFailoverEnabled: true,
      tiers: [
        { providerId: "a", position: 0, skipReason: null, rateMultiplier: 2 },
        { providerId: "b", position: 1, skipReason: null, rateMultiplier: 1 },
        { providerId: "c", position: 2, skipReason: null, rateMultiplier: 3 },
      ],
    };
  });

  it("stages drags without persisting until Apply order is clicked", async () => {
    const view = render(<ApplicationWorkspace {...props} />);
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
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument(),
    );
    view.unmount();
  });

  it("persists immediately when failover is off (no apply button)", async () => {
    state.routing.autoFailoverEnabled = false;
    render(<ApplicationWorkspace {...props} />);
    await drag(["b", "a", "c"]);
    await waitFor(() =>
      expect(state.setOrder).toHaveBeenCalledWith(["b", "a", "c"]),
    );
    expect(
      screen.queryByRole("button", { name: /applications\.applyOrder/ }),
    ).not.toBeInTheDocument();
  });

  it("offers Apply for a metric-sorted view and applies the displayed order", async () => {
    const view = render(<ApplicationWorkspace {...props} />);
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
    // 应用后临时排序与暂存一起清空，按钮消失。
    await waitFor(() => {
      expect(tableProps.current.sort).toBeNull();
      expect(tableProps.current.orderedIds).toEqual(["a", "b", "c"]);
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument();
    });
    view.unmount();
  });

  it("discarding drops staged drags and sorting back to the stored order", async () => {
    const view = render(<ApplicationWorkspace {...props} />);
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

  it("discards staged order when failover turns off (no ghost pending state)", async () => {
    const view = render(<ApplicationWorkspace {...props} />);
    await drag(["b", "a", "c"]);
    expect(
      screen.getByRole("button", { name: /applications\.applyOrder/ }),
    ).toBeInTheDocument();
    state.routing.autoFailoverEnabled = false;
    view.rerender(<ApplicationWorkspace {...props} />);
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: /applications\.applyOrder/ }),
      ).not.toBeInTheDocument(),
    );
    // 重新开启后暂存不复活：丢弃是一次性的。
    state.routing.autoFailoverEnabled = true;
    view.rerender(<ApplicationWorkspace {...props} />);
    expect(
      screen.queryByRole("button", { name: /applications\.applyOrder/ }),
    ).not.toBeInTheDocument();
    expect(state.setOrder).not.toHaveBeenCalled();
  });
});
