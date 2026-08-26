import { describe, expect, it } from "vitest";

import {
  binBounds,
  binMidpoint,
  quantileFromBins,
  TTFT_BIN_COUNT,
  TTFT_BIN_EDGES_MS,
} from "./bins";

function binsWith(index: number, count: number): number[] {
  const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
  bins[index] = count;
  return bins;
}

describe("TTFT 直方图常量", () => {
  it("桶数 = 边界数 + 1（含溢出桶），边界严格递增", () => {
    expect(TTFT_BIN_COUNT).toBe(TTFT_BIN_EDGES_MS.length + 1);
    for (let i = 1; i < TTFT_BIN_EDGES_MS.length; i++) {
      expect(TTFT_BIN_EDGES_MS[i]).toBeGreaterThan(TTFT_BIN_EDGES_MS[i - 1]);
    }
  });

  it("桶边界首桶从 0 起、末桶是溢出外推", () => {
    expect(binBounds(0)).toEqual({ lo: 0, hi: TTFT_BIN_EDGES_MS[0] });
    const last = TTFT_BIN_COUNT - 1;
    expect(binBounds(last).lo).toBe(TTFT_BIN_EDGES_MS[TTFT_BIN_EDGES_MS.length - 1]);
    expect(binBounds(last).hi).toBeGreaterThan(binBounds(last).lo);
  });

  it("中点落在桶界内", () => {
    for (let i = 0; i < TTFT_BIN_COUNT; i++) {
      const { lo, hi } = binBounds(i);
      const mid = binMidpoint(i);
      expect(mid).toBeGreaterThanOrEqual(lo);
      expect(mid).toBeLessThanOrEqual(hi);
    }
  });
});

describe("quantileFromBins", () => {
  it("空直方图返回 null", () => {
    expect(quantileFromBins(new Array<number>(TTFT_BIN_COUNT).fill(0), 0.5)).toBeNull();
  });

  it("全部质量在单桶内 → 桶内线性插值", () => {
    // 10 个样本全落在 [200,400)：median rank = 4.5 → 200 + 0.45 * 200 = 290。
    const q = quantileFromBins(binsWith(1, 10), 0.5);
    expect(q).toBeCloseTo(290, 5);
  });

  it("跨桶分布 → 分位落在正确的桶里", () => {
    // 5 个在 [0,200)、5 个在 [200,400)。
    const bins = [5, 5, ...new Array<number>(TTFT_BIN_COUNT - 2).fill(0)];
    expect(quantileFromBins(bins, 0.5)).toBeCloseTo(180, 5); // rank 4.5 落首桶
    expect(quantileFromBins(bins, 0.6)).toBeCloseTo(216, 5); // rank 5.4 落次桶
  });

  it("溢出桶按外推上界插值", () => {
    // 10 个样本全在 [9600, 14400)：rank 4.5 → 9600 + 0.45 * 4800 = 11760。
    const q = quantileFromBins(binsWith(TTFT_BIN_COUNT - 1, 10), 0.5);
    expect(q).toBeCloseTo(11760, 5);
  });

  it("q=0 / q=1 是分布的两端（rank = q×(total−1) 的插值端点）", () => {
    const bins = [3, 0, 4, ...new Array<number>(TTFT_BIN_COUNT - 3).fill(0)];
    expect(quantileFromBins(bins, 0)).toBeCloseTo(0, 5);
    // rank 6 落在 [400,600) 桶内偏 3/4 处 → 550。
    expect(quantileFromBins(bins, 1)).toBeCloseTo(550, 5);
  });
});
