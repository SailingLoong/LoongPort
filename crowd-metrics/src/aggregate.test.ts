import { describe, expect, it } from "vitest";

import { buildSnapshot, MIN_SOURCES, type RawRow } from "./aggregate";
import { hourFloorUtc } from "./validate";
import { TTFT_BIN_COUNT } from "./bins";

// 固定「现在」：2026-08-26T12:00:00Z。测试用 example 域名（公开仓隐私纪律）。
const NOW = Math.floor(Date.UTC(2026, 7, 26, 12) / 1000);

let seq = 0;
function makeRaw(overrides: Partial<RawRow> = {}): RawRow {
  const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
  bins[2] = 10; // 全部落在 [400,600) 桶
  seq += 1;
  return {
    hour: hourFloorUtc(NOW - 3600),
    site: "example.com",
    app: "claude",
    source: `s${seq}`,
    samples: 10,
    errors: 1,
    ttft_bins: JSON.stringify(bins),
    ttft_count: 10,
    input_tokens: 1000,
    output_tokens: 500,
    cache_read_tokens: 300,
    cache_creation_tokens: 100,
    cost_usd_micros: 100_000,
    ...overrides,
  };
}

function sources(n: number, overrides: Partial<RawRow> = {}): RawRow[] {
  return Array.from({ length: n }, (_, i) => ({
    ...makeRaw(overrides),
    source: `src-${i}`,
  }));
}

describe("k-匿名门槛", () => {
  it("来源 < MIN_SOURCES 的站点整体不出现", () => {
    const snap = buildSnapshot(sources(MIN_SOURCES - 1), NOW);
    expect(snap.sites).toEqual({});
  });

  it("来源 = MIN_SOURCES 时发布且 sources 如实计数", () => {
    const snap = buildSnapshot(sources(MIN_SOURCES), NOW);
    const site = snap.sites["example.com"];
    expect(site).toBeDefined();
    expect(site.w24?.sources).toBe(MIN_SOURCES);
    expect(site.w7?.sources).toBe(MIN_SOURCES);
  });

  it("两个窗口都没过门槛时站点不出现；仅 w7 过门槛时 w24 为 null", () => {
    // 3 个来源，其中 1 个只有 30 小时前的旧数据 → w24 只有 2 个来源。
    const rows = [
      ...sources(2),
      makeRaw({ source: "old", hour: hourFloorUtc(NOW - 30 * 3600) }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const site = snap.sites["example.com"];
    expect(site).toBeDefined();
    expect(site.w24).toBeNull();
    expect(site.w7?.sources).toBe(3);
  });
});

describe("极端源裁剪", () => {
  const slowBins = () => {
    const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
    bins[TTFT_BIN_COUNT - 1] = 10; // 全部在溢出桶 [9600,∞)
    return JSON.stringify(bins);
  };

  it("来源 ≥ 5 时丢掉极值源：分位数不被单个病态源拉偏", () => {
    const rows = [
      ...sources(4), // 正常源，均值 ~500ms
      makeRaw({ source: "pathological", ttft_bins: slowBins() }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const w24 = snap.sites["example.com"]?.w24;
    expect(w24).toBeDefined();
    // 裁掉最快(≈500)与最慢(≈12000)后剩 3 个正常源 → p50/p95 都该在 [400,600)。
    expect(w24?.ttftP50Ms).toBeGreaterThanOrEqual(400);
    expect(w24?.ttftP50Ms).toBeLessThan(600);
    expect(w24?.ttftP95Ms).toBeGreaterThanOrEqual(400);
    expect(w24?.ttftP95Ms).toBeLessThan(600);
  });

  it("来源 < 5 时不裁：极值照常计入（k-匿名已是底线）", () => {
    const rows = [
      ...sources(3),
      makeRaw({ source: "pathological", ttft_bins: slowBins() }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const w24 = snap.sites["example.com"]?.w24;
    expect(w24?.ttftP95Ms).toBeGreaterThanOrEqual(9600);
  });
});

describe("窗口与时段画像", () => {
  it("30 小时前的桶只进 w7 不进 w24", () => {
    const rows = sources(3, { hour: hourFloorUtc(NOW - 30 * 3600) });
    const snap = buildSnapshot(rows, NOW);
    const site = snap.sites["example.com"];
    expect(site.w24).toBeNull();
    expect(site.w7?.samples).toBe(30);
  });

  it("时段槽按 UTC 小时落位，槽内来源不过门槛则置零", () => {
    // 槽 3（UTC 03:00）3 个来源 → 有值；槽 5 只有 2 个来源 → 置零。
    const rows = [
      ...sources(3, { hour: `2026-08-26T03Z` }),
      ...sources(2, { hour: `2026-08-26T05Z`, source: undefined }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const hours = snap.sites["example.com"]?.hours;
    expect(hours).toHaveLength(24);
    expect(hours[3].p50Ms).not.toBeNull();
    expect(hours[3].samples).toBe(30);
    expect(hours[5]).toEqual({ p50Ms: null, samples: 0 });
  });
});

describe("统计口径", () => {
  it("错误率 = errors / samples", () => {
    const snap = buildSnapshot(sources(3), NOW);
    expect(snap.sites["example.com"]?.w24?.errRate).toBeCloseTo(0.1, 6);
  });

  it("缓存命中率 = cache_read / (cache_read + cache_creation + input)", () => {
    const snap = buildSnapshot(sources(3), NOW);
    // 300 / (300 + 100 + 1000) = 0.2142…
    expect(snap.sites["example.com"]?.w24?.cacheHitRate).toBeCloseTo(
      300 / 1400,
      6,
    );
  });

  it("花费参考值 = 微美元 / 总 token（即 $/Mtok）", () => {
    // 3 源 × (100_000 micros) = 300_000 micros；每源 1900 token × 3 = 5700。
    const snap = buildSnapshot(sources(3), NOW);
    expect(snap.sites["example.com"]?.w24?.costUsdPerMTok).toBeCloseTo(
      300_000 / 5700,
      6,
    );
  });

  it("多 app 合并到站点级；站点按键序产出，输出确定", () => {
    const rows = [
      ...sources(3, { site: "b.example.org" }),
      ...sources(3, { site: "a.example.org", app: "codex" }),
    ];
    const snap1 = buildSnapshot(rows, NOW);
    const snap2 = buildSnapshot([...rows].reverse(), NOW);
    expect(Object.keys(snap1.sites)).toEqual(["a.example.org", "b.example.org"]);
    expect(JSON.stringify(snap1)).toBe(JSON.stringify(snap2));
  });

  it("脏行（bins JSON 损坏）跳过，不毒死整份快照", () => {
    const rows = [...sources(3), makeRaw({ source: "dirty", ttft_bins: "not json" })];
    const snap = buildSnapshot(rows, NOW);
    expect(snap.sites["example.com"]).toBeDefined();
  });
});
