import { describe, expect, it } from "vitest";

import { cleanupDue, isFresh, STALE_AFTER_SECS } from "./freshness";

const NOW = 1_787_740_000;

describe("isFresh（GET 自愈的判定）", () => {
  it("刚生成的快照是新鲜的", () => {
    expect(isFresh(JSON.stringify({ generatedAt: NOW - 60 }), NOW)).toBe(true);
  });

  it(`超过 ${STALE_AFTER_SECS / 60} 分钟判定陈旧（触发请求路径现算）`, () => {
    expect(
      isFresh(JSON.stringify({ generatedAt: NOW - STALE_AFTER_SECS - 1 }), NOW),
    ).toBe(false);
    expect(
      isFresh(JSON.stringify({ generatedAt: NOW - STALE_AFTER_SECS }), NOW),
    ).toBe(true);
  });

  it("null / 坏 JSON / 缺字段都按陈旧处理（损坏自愈）", () => {
    expect(isFresh(null, NOW)).toBe(false);
    expect(isFresh("not json", NOW)).toBe(false);
    expect(isFresh(JSON.stringify({}), NOW)).toBe(false);
    expect(isFresh(JSON.stringify({ generatedAt: "soon" }), NOW)).toBe(false);
  });
});

describe("cleanupDue（清理时间闸）", () => {
  it("从未跑过 → 该跑", () => {
    expect(cleanupDue(null, NOW)).toBe(true);
  });

  it("一小时内跑过 → 不跑；满一小时 → 该跑", () => {
    expect(cleanupDue(NOW - 3599, NOW)).toBe(false);
    expect(cleanupDue(NOW - 3600, NOW)).toBe(true);
  });
});
