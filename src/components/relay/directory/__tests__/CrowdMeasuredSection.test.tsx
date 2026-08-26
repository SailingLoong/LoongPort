import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { CrowdSiteStats } from "@/lib/api/crowd";

import { CrowdMeasuredSection } from "../CrowdMeasuredSection";
import { formatCostPerMTok, formatErrRate, ttftTone } from "../crowdDisplay";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) =>
      options ? `${key} ${JSON.stringify(options)}` : key,
    i18n: { resolvedLanguage: "zh" },
  }),
}));

function makeStats(overrides: Partial<CrowdSiteStats> = {}): CrowdSiteStats {
  return {
    w24: {
      samples: 120,
      sources: 3,
      ttftP50Ms: 812.5,
      ttftP95Ms: 2140,
      errRate: 0.008,
      cacheHitRate: 0.62,
      costUsdPerMTok: 1.25,
    },
    w7: null,
    hours: Array.from({ length: 24 }, (_, slot) =>
      slot === 3 ? { p50Ms: 780, samples: 42 } : { p50Ms: null, samples: 0 },
    ),
    ...overrides,
  };
}

describe("crowdDisplay 格式化", () => {
  it("错误率：<1% 保留两位，≥1% 一位", () => {
    expect(formatErrRate(0.004)).toBe("0.40%");
    expect(formatErrRate(0.012)).toBe("1.2%");
  });

  it("花费参考值带美元前缀与两位小数", () => {
    expect(formatCostPerMTok(1.25)).toBe("$1.25");
  });

  it("TTFT 档位阈值：800ms / 2000ms 两道坎", () => {
    expect(ttftTone(700)).toContain("emerald");
    expect(ttftTone(1500)).toBe("text-foreground");
    expect(ttftTone(3000)).toContain("amber");
  });
});

describe("CrowdMeasuredSection 三态", () => {
  it("未参与：锁定卡 + 加入入口（onJoin 可触发）", async () => {
    const onJoin = vi.fn();
    render(
      <CrowdMeasuredSection
        stats={makeStats()}
        enabled={false}
        onJoin={onJoin}
      />,
    );
    expect(screen.getByText("loongport.crowd.lockedTitle")).toBeTruthy();
    expect(screen.queryByText("loongport.crowd.ttftP50")).toBeNull();

    await userEvent.click(
      screen.getByRole("button", { name: "loongport.crowd.join" }),
    );
    expect(onJoin).toHaveBeenCalledTimes(1);
  });

  it("参与且无数据：占位说明，不渲染指标卡", () => {
    render(
      <CrowdMeasuredSection stats={null} enabled={true} onJoin={vi.fn()} />,
    );
    expect(screen.getByText("loongport.crowd.noData")).toBeTruthy();
    expect(screen.queryByText("loongport.crowd.ttftP50")).toBeNull();
  });

  it("参与且有数据：指标卡 + 来源注脚 + 时段条形", () => {
    render(
      <CrowdMeasuredSection
        stats={makeStats()}
        enabled={true}
        onJoin={vi.fn()}
      />,
    );
    // 812.5ms 落在 [800,1200) 的毫秒格式化（取整 813ms）。
    expect(screen.getByText("813ms")).toBeTruthy();
    expect(screen.getByText("2.1s")).toBeTruthy();
    expect(
      screen.getByText(/loongport.crowd.noteSources.*"count":3/u),
    ).toBeTruthy();
    // 24 根时段条都在（t 被 mock 成 key+JSON，按 mock 输出断言）。
    const bars = screen.getAllByTitle(/loongport.crowd.hour(Tooltip|Empty)/u);
    expect(bars).toHaveLength(24);
    expect(
      screen.getByTitle(/hourTooltip.*"slot":"03".*"value":"780ms"/u),
    ).toBeTruthy();
  });

  it("分布图：bins 与 edges 齐备时渲染 12 根分布条", () => {
    const stats = makeStats({
      w24: {
        ...makeStats().w24!,
        ttftBins: [3, 8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
      },
    });
    render(
      <CrowdMeasuredSection
        stats={stats}
        binEdges={[
          200, 400, 600, 800, 1200, 1600, 2400, 3200, 4800, 6400, 9600,
        ]}
        enabled={true}
        onJoin={vi.fn()}
      />,
    );
    const bars = screen.getAllByTitle(/loongport.crowd.distTooltip/u);
    expect(bars).toHaveLength(12);
    expect(screen.getByTitle(/"range":"200ms–400ms".*"count":8/u)).toBeTruthy();
  });

  it("分布图：缺 edges（旧快照）不渲染分布条", () => {
    const stats = makeStats({
      w24: {
        ...makeStats().w24!,
        ttftBins: [3, 8, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
      },
    });
    render(
      <CrowdMeasuredSection stats={stats} enabled={true} onJoin={vi.fn()} />,
    );
    expect(screen.queryAllByTitle(/loongport.crowd.distTooltip/u)).toHaveLength(
      0,
    );
    // 时段条仍在
    expect(
      screen.getAllByTitle(/loongport.crowd.hour(Tooltip|Empty)/u),
    ).toHaveLength(24);
  });

  it("w24 缺席时用 w7 并标注窗口", () => {
    render(
      <CrowdMeasuredSection
        stats={makeStats({ w24: null, w7: makeStats().w24 })}
        enabled={true}
        onJoin={vi.fn()}
      />,
    );
    expect(screen.getByText(/loongport.crowd.window7/u)).toBeTruthy();
  });
});
