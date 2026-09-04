import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";

import { EasyBoard } from "@/components/easymode/EasyBoard";
import type { TierBoard } from "@/lib/api/autoMode";

// 本测试关注「看板怎么呈现事实 / 用户动作落到哪个 mutation」；
// 事实的聚合正确性由后端 tier_board 的单测钉住。
const setModelMock = vi.hoisted(() => vi.fn());
const setStrategyMock = vi.hoisted(() => vi.fn());
const setModeMock = vi.hoisted(() => vi.fn());
const setOrderMock = vi.hoisted(() => vi.fn());
const tierBoardMock = vi.hoisted(() => vi.fn());
const statusMock = vi.hoisted(() => vi.fn());
const resetBreakerMock = vi.hoisted(() => vi.fn());

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
    availableModels: ["claude-fable-5", "deepseek-v4-flash"],
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
      availableModels: board.availableModels,
      hasCandidates: true,
      cliInstalled: true,
    },
  });
  render(<EasyBoard appId="claude" />);
}

beforeEach(() => {
  vi.clearAllMocks();
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
    // 当前命中只在 isCurrent 档出现一次
    expect(screen.getAllByText("当前")).toHaveLength(1);
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
