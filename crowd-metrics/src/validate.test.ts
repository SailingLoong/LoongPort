import { describe, expect, it } from "vitest";

import { TTFT_BIN_COUNT } from "./bins";
import { hourFloorUtc, hourToEpochSec, isValidSite, parseIngestPayload } from "./validate";
import type { IngestPayload } from "./types";

// 固定「现在」：2026-08-26T12:00:00Z。测试用 example 域名（公开仓隐私纪律）。
const NOW = Math.floor(Date.UTC(2026, 7, 26, 12) / 1000);

function makeBucket(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  const bins = new Array<number>(TTFT_BIN_COUNT).fill(0);
  bins[1] = 8;
  bins[2] = 2;
  return {
    hour: hourFloorUtc(NOW - 3600),
    site: "example.com",
    app: "claude",
    samples: 10,
    errors: 1,
    ttftBins: bins,
    ttftCount: 10,
    inputTokens: 1000,
    outputTokens: 500,
    cacheReadTokens: 300,
    cacheCreationTokens: 100,
    costUsdMicros: 12_345,
    ...overrides,
  };
}

function makePayload(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    version: 1,
    sourceId: "0123456789abcdef0123456789abcdef",
    hours: [makeBucket()],
    ...overrides,
  };
}

describe("hour 工具", () => {
  it("epoch ↔ 小时串互为往返", () => {
    expect(hourFloorUtc(hourToEpochSec("2026-08-26T07Z"))).toBe("2026-08-26T07Z");
    expect(hourToEpochSec(hourFloorUtc(NOW))).toBeLessThanOrEqual(NOW);
  });
});

describe("isValidSite", () => {
  it.each([
    "example.com",
    "api.example.co.uk",
    "relay-1.example.io",
  ])("接受归一化 host：%s", (site) => {
    expect(isValidSite(site)).toBe(true);
  });

  it.each([
    "https://example.com", // 带 scheme
    "example.com:8443", // 带端口
    "Example.COM", // 大写
    "www.example.com", // 归一化应已去 www（防同站双身份）
    "192.168.1.5", // IP 字面量（内网地址无公开意义）
    "localhost",
    "example.com.", // 尾点
    "", // 空
  ])("拒绝非归一形状：%s", (site) => {
    expect(isValidSite(site)).toBe(false);
  });
});

describe("parseIngestPayload", () => {
  it("合法载荷整体通过，字段逐位保留", () => {
    const result = parseIngestPayload(makePayload(), NOW);
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.payload.hours[0].site).toBe("example.com");
      expect(result.payload.hours[0].ttftCount).toBe(10);
    }
  });

  it("版本不是 1 拒绝", () => {
    expect(parseIngestPayload(makePayload({ version: 2 }), NOW).ok).toBe(false);
  });

  it("sourceId 非 32 位小写 hex 拒绝", () => {
    expect(
      parseIngestPayload(makePayload({ sourceId: "XYZ" }), NOW).ok,
    ).toBe(false);
  });

  it("hours 为空或超过上限拒绝", () => {
    expect(parseIngestPayload(makePayload({ hours: [] }), NOW).ok).toBe(false);
    const many = Array.from({ length: 201 }, () => makeBucket());
    expect(parseIngestPayload(makePayload({ hours: many }), NOW).ok).toBe(false);
  });

  it.each([
    ["2026-02-31T00Z", "日历不合法（2 月 31 日）"],
    ["2026-13-01T00Z", "月越界"],
    ["2026-08-26T24Z", "小时越界"],
    ["2026-08-26 07", "格式不对"],
    [hourFloorUtc(NOW + 2 * 3600), "未来小时"],
    [hourFloorUtc(NOW - 40 * 86400), "太老（超出保留期）"],
  ])("小时串 %s 拒绝（%s）", (hour) => {
    const result = parseIngestPayload(
      makePayload({ hours: [makeBucket({ hour })] }),
      NOW,
    );
    expect(result.ok).toBe(false);
  });

  it.each([
    ["ttftBins 长度不符", { ttftBins: new Array<number>(TTFT_BIN_COUNT - 1).fill(0) }],
    ["bins 总和 ≠ ttftCount", { ttftBins: new Array<number>(TTFT_BIN_COUNT).fill(0) }],
    ["ttftCount > samples", { ttftCount: 11, ttftBins: (() => { const b = new Array<number>(TTFT_BIN_COUNT).fill(0); b[1] = 11; return b; })() }],
    ["errors > samples", { errors: 11 }],
    ["负数 token", { inputTokens: -1 }],
    ["小数 samples", { samples: 1.5 }],
  ])("%s 拒绝", (_label, overrides) => {
    const result = parseIngestPayload(
      makePayload({ hours: [makeBucket(overrides)] }),
      NOW,
    );
    expect(result.ok).toBe(false);
  });

  it("同一 (hour, site, app) 重复桶拒绝", () => {
    const result = parseIngestPayload(
      makePayload({ hours: [makeBucket(), makeBucket()] }),
      NOW,
    );
    expect(result.ok).toBe(false);
  });

  it("app 大写或带非法字符拒绝", () => {
    expect(
      parseIngestPayload(
        makePayload({ hours: [makeBucket({ app: "Claude" })] }),
        NOW,
      ).ok,
    ).toBe(false);
  });
});

describe("载荷隐私边界（对应客户端 Rust 侧的同类闸）", () => {
  it("序列化文本里不该出现任何身份/凭据形态", () => {
    const result = parseIngestPayload(makePayload(), NOW);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const text = JSON.stringify(result.payload as unknown as IngestPayload);
    for (const forbidden of ["token-", "sk-", "email", "@", "password", "apikey", "username"]) {
      expect(text.toLowerCase()).not.toContain(forbidden);
    }
  });
});
