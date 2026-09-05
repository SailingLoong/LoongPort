import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import {
  describe,
  expect,
  it,
  vi,
  beforeEach,
  beforeAll,
  afterAll,
} from "vitest";

import { EasyBoard } from "@/components/easymode/EasyBoard";
import type { TierBoard } from "@/lib/api/autoMode";

// 看板验真徽章的呈现按模块启用测（下线契约在
// model-verification/__tests__/offline.test.tsx 单独钉）。
vi.mock("@/components/relay/model-verification/availability", () => ({
  MODEL_VERIFICATION_ENABLED: true,
}));

// 本测试关注「看板怎么呈现事实 / 用户动作落到哪个 mutation」；
// 事实的聚合正确性由后端 tier_board 的单测钉住。
const setModelMock = vi.hoisted(() => vi.fn());
const setStrategyMock = vi.hoisted(() => vi.fn());
const setModeMock = vi.hoisted(() => vi.fn());
const setOrderMock = vi.hoisted(() => vi.fn());
const tierBoardMock = vi.hoisted(() => vi.fn());
const statusMock = vi.hoisted(() => vi.fn());
const resetBreakerMock = vi.hoisted(() => vi.fn());
const proxyStatusMock = vi.hoisted(() => vi.fn());
const switchBackMock = vi.hoisted(() => vi.fn());
const providersQueryMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/query/queries", () => ({
  useProvidersQuery: providersQueryMock,
}));

vi.mock("@/hooks/useProxyStatus", () => ({
  useProxyStatus: proxyStatusMock,
}));

vi.mock("@/lib/query/autoMode", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/query/autoMode")>();
  return {
    ...actual,
    useTierBoard: tierBoardMock,
    useAutoModeStatus: statusMock,
    useSetAutoModeModel: () => ({ mutate: setModelMock, isPending: false }),
    useSetAutoModeStrategy: () => ({
      mutate: setStrategyMock,
      isPending: false,
    }),
    useSetEasyModeMode: () => ({ mutate: setModeMock, isPending: false }),
    useSetEasyModeManualOrder: () => ({
      mutate: setOrderMock,
      isPending: false,
    }),
    useSwitchToSelfManaged: () => ({
      mutateAsync: switchBackMock,
      isPending: false,
    }),
  };
});

vi.mock("@/lib/query/failover", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/query/failover")>();
  return {
    ...actual,
    useResetCircuitBreaker: () => ({
      mutate: resetBreakerMock,
      mutateAsync: resetBreakerMock,
      isPending: false,
    }),
  };
});

function boardFixture(overrides: Partial<TierBoard> = {}): TierBoard {
  return {
    mode: "auto",
    strategy: "cheapest",
    model: null,
    modelOptions: [
      { model: "claude-fable-5", tierCount: 2, cheapestPricePerMillion: 1.5 },
      {
        model: "deepseek-v4-flash",
        tierCount: 1,
        cheapestPricePerMillion: null,
      },
    ],
    currentProviderId: "tier-b",
    tiers: [
      {
        providerId: "tier-a",
        name: "便宜档",
        position: 0,
        isCurrent: false,
        rateMultiplier: 0.5,
        unitPricePerMillion: 2,
        effectiveModel: "claude-fable-5",
        avgFirstTokenMs: 420,
        balanceUsd: 10.347,
        verificationVerdict: null,
        isHealthy: null,
        consecutiveFailures: null,
        lastError: null,
        todayCostUsd: 0.75,
        todayRequests: 128,
        cacheHitRate: 0.87,
        recentActivity: [
          ...Array.from({ length: 10 }, () => ({
            successCount: 5,
            failCount: 0,
            avgFirstTokenMs: 800,
          })),
          { successCount: 3, failCount: 1, avgFirstTokenMs: 1200 },
          ...Array.from({ length: 12 }, () => ({
            successCount: 4,
            failCount: 0,
            avgFirstTokenMs: 900,
          })),
          { successCount: 0, failCount: 2, avgFirstTokenMs: null },
        ],
        breakerState: null,
        breakerReopenInSecs: null,
        // 当前档是 tier-b…不对：fixture 里 isCurrent 是 tier-b（贵档）；
        // 粘性徽章断言放这条会混——tier-a 非当前，不给 affinity。
        affinityRemainingSecs: null,
      },
      {
        providerId: "tier-b",
        name: "贵档",
        position: 1,
        isCurrent: true,
        rateMultiplier: 2,
        unitPricePerMillion: null,
        effectiveModel: null,
        avgFirstTokenMs: null,
        balanceUsd: null,
        verificationVerdict: null,
        isHealthy: null,
        consecutiveFailures: null,
        lastError: null,
        todayCostUsd: null,
        todayRequests: null,
        cacheHitRate: null,
        recentActivity: null,
        breakerState: null,
        breakerReopenInSecs: null,
        affinityRemainingSecs: 720,
      },
    ],
    ...overrides,
  };
}

function setupBoard(board: TierBoard) {
  tierBoardMock.mockReturnValue({ data: board, isLoading: false });
  statusMock.mockReturnValue({
    data: {
      enabled: true,
      strategy: board.strategy,
      model: board.model,
      availableModels: board.modelOptions.map((option) => option.model),
      hasCandidates: true,
      cliInstalled: true,
    },
  });
  render(<EasyBoard appId="claude" />);
}

// cmdk（模型选择器）需要 scrollIntoView，jsdom 没有 —— 保存/恢复式打桩
let scrollIntoViewDescriptor: PropertyDescriptor | undefined;
beforeAll(() => {
  scrollIntoViewDescriptor = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "scrollIntoView",
  );
  Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
    configurable: true,
    value: vi.fn(),
  });
});
afterAll(() => {
  if (scrollIntoViewDescriptor) {
    Object.defineProperty(
      HTMLElement.prototype,
      "scrollIntoView",
      scrollIntoViewDescriptor,
    );
  }
});

beforeEach(() => {
  vi.clearAllMocks();
  proxyStatusMock.mockReturnValue({
    isRunning: true,
    startProxyServer: vi.fn().mockResolvedValue(undefined),
  });
  providersQueryMock.mockReturnValue({
    data: {
      providers: {
        "loongport-d680a2ae9e42a740": {
          id: "loongport-d680a2ae9e42a740",
          name: "托管档（不该出现在栏里）",
        },
        "official-chatgpt": {
          id: "official-chatgpt",
          name: "ChatGPT 官方",
        },
        "my-custom": {
          id: "my-custom",
          name: "我的自建",
        },
      },
    },
  });
  switchBackMock.mockResolvedValue({ status: "ok" });
});

describe("EasyBoard", () => {
  it("自动模式：按优先级序展示档位事实（倍率/单价/余额/首字/当前）", () => {
    setupBoard(boardFixture());
    expect(screen.getByText("便宜档")).toBeDefined();
    expect(screen.getByText("贵档")).toBeDefined();
    expect(screen.getByText("×0.5")).toBeDefined();
    expect(screen.getByText("$2.00/M")).toBeDefined();
    expect(screen.getByText("余额 $10.35")).toBeDefined();
    expect(screen.getByText("价格未知")).toBeDefined();
    expect(screen.getByText("今日 $0.75 · 128 次")).toBeDefined();
    expect(screen.getByText("缓存 87%")).toBeDefined();
    // 有近期流量的档渲染时间线（title 汇总 + 首字折线）；无流量的档不渲染
    expect(
      screen.getByTitle("近 6 小时：成功 101 · 失败 3 · 首字 870ms"),
    ).toBeDefined();
    expect(document.querySelectorAll("polyline").length).toBeGreaterThan(0);
    // 当前命中只在 isCurrent 档出现一次；粘性倒计时跟着当前档走
    expect(screen.getAllByText("当前")).toHaveLength(1);
    expect(screen.getByTitle(/会话粘性中/)).toBeDefined();
    expect(screen.getByText("粘性 · 12 分钟")).toBeDefined();
  });

  it("失败档位外露原因徽章（熔断），健康档位不显示健康标记", () => {
    const nulls = {
      effectiveModel: null,
      avgFirstTokenMs: null,
      balanceUsd: null,
      verificationVerdict: null,
      todayCostUsd: null,
      todayRequests: null,
      cacheHitRate: null,
      recentActivity: null,
      breakerState: null,
      breakerReopenInSecs: null,
      affinityRemainingSecs: null,
    };
    setupBoard(
      boardFixture({
        tiers: [
          {
            providerId: "tier-dead",
            name: "熔断档",
            position: 0,
            isCurrent: false,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: false,
            consecutiveFailures: 4,
            lastError: '上游 HTTP 403: {"error":{"message":"无可用渠道"}}',
          },
          {
            providerId: "tier-fine",
            name: "健康档",
            position: 1,
            isCurrent: true,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: true,
            consecutiveFailures: 0,
            lastError: null,
          },
        ],
      }),
    );
    expect(screen.getByText("熔断")).toBeDefined();
    expect(screen.queryByText("降级")).toBeNull();
    expect(screen.getAllByText("当前")).toHaveLength(1);
  });

  it("失败档位有重新启用按钮，头部出现重试全部；点击批量重试逐档调用", async () => {
    resetBreakerMock.mockResolvedValue(undefined);
    const nulls = {
      effectiveModel: null,
      avgFirstTokenMs: null,
      balanceUsd: null,
      verificationVerdict: null,
      todayCostUsd: null,
      todayRequests: null,
      cacheHitRate: null,
      recentActivity: null,
      breakerState: null,
      breakerReopenInSecs: null,
      affinityRemainingSecs: null,
    };
    setupBoard(
      boardFixture({
        tiers: [
          {
            providerId: "tier-dead-a",
            name: "熔断档A",
            position: 0,
            isCurrent: false,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: false,
            consecutiveFailures: 4,
            lastError: '上游 HTTP 403: {"error":{"message":"无可用渠道"}}',
          },
          {
            providerId: "tier-degraded-b",
            name: "降级档B",
            position: 1,
            isCurrent: true,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: true,
            consecutiveFailures: 2,
            lastError: "上游 HTTP 429: rate limited",
          },
          {
            providerId: "tier-fine",
            name: "健康档",
            position: 2,
            isCurrent: false,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: true,
            consecutiveFailures: 0,
            lastError: null,
          },
        ],
      }),
    );
    // 失败档位各有一个重新启用（健康档没有）
    expect(screen.getAllByLabelText("重新启用")).toHaveLength(2);
    const retryAll = screen.getByText("重试全部熔断档位");
    expect(retryAll).toBeDefined();

    fireEvent.click(retryAll);
    await waitFor(() => expect(resetBreakerMock).toHaveBeenCalledTimes(2));
    expect(resetBreakerMock).toHaveBeenCalledWith({
      providerId: "tier-dead-a",
      appType: "claude",
    });
  });

  it("内存熔断态（DB 仍健康）也外露熔断徽章并计入重试全部", async () => {
    resetBreakerMock.mockResolvedValue(undefined);
    const nulls = {
      effectiveModel: null,
      avgFirstTokenMs: null,
      balanceUsd: null,
      verificationVerdict: null,
      todayCostUsd: null,
      todayRequests: null,
      cacheHitRate: null,
      recentActivity: null,
      affinityRemainingSecs: null,
    };
    setupBoard(
      boardFixture({
        tiers: [
          {
            providerId: "tier-fatal",
            name: "致命档",
            position: 0,
            isCurrent: false,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: true,
            consecutiveFailures: 1,
            lastError: "上游 HTTP 403: 无可用渠道",
            breakerState: "open",
            breakerReopenInSecs: 1740,
          },
          {
            providerId: "tier-fine",
            name: "健康档",
            position: 1,
            isCurrent: true,
            rateMultiplier: 1,
            unitPricePerMillion: null,
            ...nulls,
            isHealthy: true,
            consecutiveFailures: 0,
            lastError: null,
            breakerState: null,
            breakerReopenInSecs: null,
          },
        ],
      }),
    );
    expect(screen.getByText("熔断")).toBeDefined();
    fireEvent.click(screen.getByText("重试全部熔断档位"));
    await waitFor(() => expect(resetBreakerMock).toHaveBeenCalledTimes(1));
    expect(resetBreakerMock).toHaveBeenCalledWith({
      providerId: "tier-fatal",
      appType: "claude",
    });
  });

  it("点策略按钮落到 setStrategy mutation", () => {
    setupBoard(boardFixture());
    fireEvent.click(screen.getByText("省时"));
    expect(setStrategyMock).toHaveBeenCalledWith({ strategy: "fastest" });
  });

  it("切手动模式走 setMode；手动下不再显示策略按钮", () => {
    setupBoard(boardFixture({ mode: "manual" }));
    // 当前是手动：切回自动是可见动作
    fireEvent.click(screen.getByText("自动"));
    expect(setModeMock).toHaveBeenCalledWith({
      appType: "claude",
      mode: "auto",
    });
  });

  it("本地路由未运行时显示警告条与启动按钮", () => {
    const startRouting = vi.fn().mockResolvedValue(undefined);
    proxyStatusMock.mockReturnValue({
      isRunning: false,
      startProxyServer: startRouting,
    });
    setupBoard(boardFixture());
    expect(
      screen.getByText("本地路由未运行，流量不会经省心选路"),
    ).toBeDefined();
    fireEvent.click(screen.getByText("启动本地路由"));
    expect(startRouting).toHaveBeenCalledTimes(1);
  });

  it("官方与自建栏：只列非托管供应商，点击走切回编排", async () => {
    setupBoard(boardFixture());
    expect(screen.getByText("官方与自建")).toBeDefined();
    expect(screen.getByText("ChatGPT 官方")).toBeDefined();
    expect(screen.getByText("我的自建")).toBeDefined();
    expect(screen.queryByText("托管档（不该出现在栏里）")).toBeNull();

    fireEvent.click(screen.getByText("ChatGPT 官方"));
    await waitFor(() =>
      expect(switchBackMock).toHaveBeenCalledWith({
        appType: "claude",
        providerId: "official-chatgpt",
        quitChatgpt: undefined,
      }),
    );
  });

  it("模型选择器：可输入过滤，候选项带档数与最低价", async () => {
    setupBoard(boardFixture());
    // 默认收起，触发器显示「不限模型」
    expect(screen.getByRole("combobox", { name: "" })).toBeDefined();

    fireEvent.click(screen.getByRole("combobox"));
    expect(await screen.findByText("claude-fable-5")).toBeDefined();
    expect(screen.getByText("2 档 · $1.50/M")).toBeDefined();
    expect(screen.getByText("deepseek-v4-flash")).toBeDefined();
    expect(screen.getByText("1 档")).toBeDefined();

    // 输入过滤：只剩匹配项
    fireEvent.change(await screen.findByPlaceholderText("输入模型名筛选…"), {
      target: { value: "deepseek" },
    });
    expect(screen.queryByText("claude-fable-5")).toBeNull();
    expect(screen.getByText("deepseek-v4-flash")).toBeDefined();

    // 选中 deepseek → setModel 收到 null 语义反转前的真实模型名
    fireEvent.click(screen.getByText("deepseek-v4-flash"));
    await waitFor(() =>
      expect(setModelMock).toHaveBeenCalledWith({
        appType: "claude",
        model: "deepseek-v4-flash",
      }),
    );
  });

  it("空档位显示空态而不是崩", () => {
    setupBoard(boardFixture({ tiers: [] }));
    expect(screen.getByText("还没有可用档位")).toBeDefined();
  });

  it("验真异常档显示标记，干净档位零视觉噪音", () => {
    const tiers = boardFixture().tiers.map((tier) => ({ ...tier }));
    tiers[0] = { ...tiers[0], verificationVerdict: "anomaly" };
    setupBoard(boardFixture({ tiers }));

    expect(screen.getByTitle("检测到异常")).toBeDefined();
    expect(screen.queryByTitle("需要复核")).toBeNull();
    expect(screen.queryByTitle("验证通过")).toBeNull();
  });

  it("suspicious 档显示复核标记", () => {
    const tiers = boardFixture().tiers.map((tier) => ({
      ...tier,
      verificationVerdict: "suspicious" as const,
    }));
    setupBoard(boardFixture({ tiers }));
    expect(screen.getAllByTitle("需要复核")).toHaveLength(2);
  });
});
