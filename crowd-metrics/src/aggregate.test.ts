import { describe, expect, it } from "vitest";

import {
  buildSnapshot,
  COHORT_FACTOR,
  MIN_ASN,
  MIN_SOURCES,
  type RawRow,
} from "./aggregate";
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
    asn: 4134,
    ua_trusted: 1,
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

/** n 个受信来源，ASN 在电信/联通间交替（满足多样性门槛的「正常」形态）。 */
function sources(n: number, overrides: Partial<RawRow> = {}): RawRow[] {
  return Array.from({ length: n }, (_, i) => ({
    ...makeRaw(overrides),
    source: `src-${i}`,
    asn: i % 2 === 0 ? 4134 : 4837,
  }));
}

describe("k-匿名与网络多样性门槛（L1）", () => {
  it("受信来源 < MIN_SOURCES 的站点整体不出现", () => {
    const snap = buildSnapshot(sources(MIN_SOURCES - 1), NOW);
    expect(snap.sites).toEqual({});
  });

  it("来源够但都在同一个 ASN：不发布（一个出口刷不动）", () => {
    const rows = Array.from({ length: 4 }, (_, i) => ({
      ...makeRaw(),
      source: `src-${i}`,
      asn: 4134,
    }));
    const snap = buildSnapshot(rows, NOW);
    expect(snap.sites).toEqual({});
  });

  it("横跨 ≥MIN_ASN 个 ASN 时发布，sources 如实计数", () => {
    const snap = buildSnapshot(sources(MIN_SOURCES), NOW);
    const site = snap.sites["example.com"];
    expect(site).toBeDefined();
    expect(site.w24?.sources).toBe(MIN_SOURCES);
  });

  it("未受信 UA 的来源不计入门禁（3 个脚本来源 ≠ 3 个用户）", () => {
    const snap = buildSnapshot(sources(MIN_SOURCES, { ua_trusted: 0 }), NOW);
    expect(snap.sites).toEqual({});
  });

  it("两个窗口都没过门槛时站点不出现；仅 w7 过门槛时 w24 为 null", () => {
    const rows = [
      ...sources(2),
      makeRaw({
        source: "old",
        asn: 4837,
        hour: hourFloorUtc(NOW - 30 * 3600),
      }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const site = snap.sites["example.com"];
    expect(site).toBeDefined();
    expect(site.w24).toBeNull();
    expect(site.w7?.sources).toBe(3);
  });
});

describe("cohort 异常剔除（L3）", () => {
  const fastBins = () => {
    const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
    bins[1] = 10; // [200,400) —— 比正常源快 3 倍以上
    return JSON.stringify(bins);
  };
  const slowRealBins = () => {
    const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
    bins[5] = 10; // [1200,1600)：桶 i 的区间是 [EDGES[i-1], EDGES[i])
    return JSON.stringify(bins);
  };

  function realSources(n: number): RawRow[] {
    return Array.from({ length: n }, (_, i) => ({
      ...makeRaw({ ttft_bins: slowRealBins(), ttft_count: 10 }),
      source: `real-${i}`,
      asn: i % 2 === 0 ? 4134 : 4837,
    }));
  }
  function fakeCohort(n: number, asn: number): RawRow[] {
    return Array.from({ length: n }, (_, i) => ({
      ...makeRaw({ ttft_bins: fastBins(), ttft_count: 10 }),
      source: `fake-${i}`,
      asn,
    }));
  }

  it("同 ASN + 只此一家 + 联合快于同行 3 倍 → 整组剔除", () => {
    const rows = [...realSources(3), ...fakeCohort(3, 56040)];
    const snap = buildSnapshot(rows, NOW);
    const w24 = snap.sites["example.com"]?.w24;
    expect(w24).toBeDefined();
    // 假组被剔除后统计只来自真实源：p50 落在 [1200,1600)。
    expect(w24?.ttftP50Ms).toBeGreaterThanOrEqual(1200);
    expect(w24?.ttftP50Ms).toBeLessThan(1600);
    // 快桶里不该再有假源样本
    expect(w24?.ttftBins[1]).toBe(0);
    expect(w24?.sources).toBe(3);
  });

  it("跨站出现的来源组不是「只此一家」，不剔除", () => {
    const fakesAtA = fakeCohort(2, 56040);
    const rows = [
      ...realSources(3),
      ...fakesAtA,
      // 同一批 source 也给另一家站报数 → 不再 site-exclusive → 不构成指纹。
      ...fakesAtA.map((r) => ({ ...r, site: "other.example.org" })),
    ];
    const snap = buildSnapshot(rows, NOW);
    const w24 = snap.sites["example.com"]?.w24;
    expect(w24?.sources).toBe(5);
  });

  it("联合快慢不到因子阈值 → 不剔除（模型差异是合法的）", () => {
    // 真实源 [1200,1600) 均值 1400，cohort [600,800) 均值 700：比值 2，不够 3。
    const mildBins = () => {
      const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
      bins[3] = 10; // [600,800)
      return JSON.stringify(bins);
    };
    const rows = [
      ...realSources(3),
      ...Array.from({ length: 2 }, (_, i) => ({
        ...makeRaw({ ttft_bins: mildBins(), ttft_count: 10 }),
        source: `mild-${i}`,
        asn: 56040,
      })),
    ];
    const snap = buildSnapshot(rows, NOW);
    const w24 = snap.sites["example.com"]?.w24;
    expect(w24?.sources).toBe(5);
    expect(COHORT_FACTOR).toBe(3);
  });
});

describe("极值裁剪（单点离群防线）", () => {
  const overflowBins = () => {
    const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
    bins[TTFT_BIN_COUNT - 1] = 10; // 全部在溢出桶 [9600,∞)
    return JSON.stringify(bins);
  };

  it("来源 ≥5 时丢掉极值源：分位数不被单个病态源拉偏", () => {
    const rows = [
      ...sources(4),
      makeRaw({
        source: "pathological",
        ttft_bins: overflowBins(),
        asn: 56040,
      }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const w24 = snap.sites["example.com"]?.w24;
    expect(w24?.ttftP95Ms).toBeLessThan(9600);
  });
});

describe("分布直方图与口径", () => {
  it("快照窗口带合并后的 ttftBins，求和覆盖全部样本", () => {
    const snap = buildSnapshot(sources(3), NOW);
    const w24 = snap.sites["example.com"]?.w24!;
    expect(w24.ttftBins).toHaveLength(TTFT_BIN_COUNT);
    expect(w24.ttftBins.reduce((a, b) => a + b, 0)).toBe(30);
  });

  it("错误率 / 缓存命中 / 花费口径不变", () => {
    const snap = buildSnapshot(sources(3), NOW);
    const w24 = snap.sites["example.com"]?.w24!;
    expect(w24.errRate).toBeCloseTo(0.1, 6);
    expect(w24.cacheHitRate).toBeCloseTo(300 / 1400, 6);
    expect(w24.costUsdPerMTok).toBeCloseTo(300_000 / 5700, 6);
  });

  it("时段槽按 UTC 小时落位，槽内来源不过门槛则置零", () => {
    const rows = [
      ...sources(3, { hour: "2026-08-26T03Z" }),
      ...sources(2, { hour: "2026-08-26T05Z" }),
    ];
    const snap = buildSnapshot(rows, NOW);
    const hours = snap.sites["example.com"]?.hours;
    expect(hours).toHaveLength(24);
    expect(hours[3].p50Ms).not.toBeNull();
    expect(hours[5]).toEqual({ p50Ms: null, samples: 0 });
  });

  it("多 app 合并到站点级；站点按键序产出，输出确定；脏行跳过", () => {
    const rows = [
      ...sources(3, { site: "b.example.org" }),
      ...sources(3, { site: "a.example.org", app: "codex" }),
      makeRaw({ source: "dirty", ttft_bins: "not json" }),
    ];
    const snap1 = buildSnapshot(rows, NOW);
    const snap2 = buildSnapshot([...rows].reverse(), NOW);
    expect(Object.keys(snap1.sites)).toEqual([
      "a.example.org",
      "b.example.org",
    ]);
    expect(JSON.stringify(snap1)).toBe(JSON.stringify(snap2));
  });

  it("快照可携带桶边界（recompute 注入，唯源 bins.ts）", async () => {
    const { TTFT_BIN_EDGES_MS } = await import("./bins");
    expect(TTFT_BIN_EDGES_MS).toHaveLength(11);
    expect(TTFT_BIN_EDGES_MS[0]).toBe(200);
  });

  it("门槛常量钉住：MIN_SOURCES=3、MIN_ASN=2（调阈值要连着测试改）", () => {
    expect(MIN_SOURCES).toBe(3);
    expect(MIN_ASN).toBe(2);
  });
});
