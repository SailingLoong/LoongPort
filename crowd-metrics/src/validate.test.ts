import { describe, expect, it } from "vitest";

import { TPS_BIN_COUNT, TTFT_BIN_COUNT } from "./bins";
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

/** P4：合法模型子桶（v2）。 */
function makeModelBucket(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  const ttft = new Array<number>(TTFT_BIN_COUNT).fill(0);
  ttft[1] = 8;
  const tps = new Array<number>(TPS_BIN_COUNT).fill(0);
  tps[4] = 6;
  tps[5] = 2;
  return {
    model: "gpt-example",
    samples: 10,
    errors: 1,
    ttftBins: ttft,
    tpsBins: tps,
    inputTokens: 1000,
    outputTokens: 500,
    cacheReadTokens: 300,
    cacheCreationTokens: 100,
    costUsdMicros: 12_345,
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

  it("errSamples 必须穿透到解析产物（E2E 实测曾在校验后被丢）", () => {
    const ok = parseIngestPayload(
      makePayload({
        version: 2,
        hours: [makeBucket({ errSamples: 4, models: [makeModelBucket({ errSamples: 4 })] })],
      }),
      NOW,
    );
    expect(ok.ok).toBe(true);
    if (ok.ok) {
      expect(ok.payload.hours[0].errSamples).toBe(4);
      expect(ok.payload.hours[0].models?.[0]?.errSamples).toBe(4);
    }
    // 缺省合法（旧客户端）——解析产物同样缺省。
    const legacy = parseIngestPayload(makePayload(), NOW);
    expect(legacy.ok).toBe(true);
    if (legacy.ok) expect(legacy.payload.hours[0].errSamples).toBeUndefined();
  });

  it("版本不是 1/2 拒绝（v2 自 P4 起接受）", () => {
    expect(parseIngestPayload(makePayload({ version: 3 }), NOW).ok).toBe(false);
    expect(parseIngestPayload(makePayload({ version: 2 }), NOW).ok).toBe(false); // v2 必须 models
  });

  it("v2：模型子桶合法通过、逐位保留", () => {
    const result = parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket()] })] }), NOW);
    expect(result.ok).toBe(true);
    if (result.ok) {
      const m = result.payload.hours[0].models?.[0];
      expect(m?.model).toBe("gpt-example");
      expect(m?.tpsBins).toHaveLength(TPS_BIN_COUNT);
    }
  });

  it("P5：模型异常计数缺省 0、超模型样本数拒绝", () => {
    const ok = parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket({ anomalies: 2 })] })] }), NOW);
    expect(ok.ok).toBe(true);
    if (ok.ok) expect(ok.payload.hours[0].models?.[0]?.anomalies).toBe(2);
    // 缺省（旧客户端）→ 0
    const legacy = parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket()] })] }), NOW);
    if (legacy.ok) expect(legacy.payload.hours[0].models?.[0]?.anomalies).toBe(0);
    // 异常次数恒 ≤ 该模型样本数（异常响应必然是被计数的请求之一）
    expect(
      parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket({ anomalies: 99 })] })] }), NOW).ok,
    ).toBe(false);
  });

  it("v2：缺 models 数组 / 坏模型名 / 子桶超母桶样本 / tps 桶长错 一律拒绝", () => {
    expect(parseIngestPayload(makePayload({ version: 2 }), NOW).ok).toBe(false);
    expect(
      parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket({ model: "bad name!" })] })] }), NOW).ok,
    ).toBe(false);
    expect(
      parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket({ samples: 99 })] })] }), NOW).ok,
    ).toBe(false);
    expect(
      parseIngestPayload(makePayload({ version: 2, hours: [makeBucket({ models: [makeModelBucket({ tpsBins: [1] })] })] }), NOW).ok,
    ).toBe(false);
  });

  it("v1 载荷（无 models 键）照旧通过 —— 双版本兼容期", () => {
    const result = parseIngestPayload(makePayload(), NOW);
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.payload.hours[0].models).toBeUndefined();
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

describe("P4b 跳闸计数校验", () => {
  it("breakerTrips 可选、合法时逐位保留", () => {
    const r1 = parseIngestPayload(makePayload(), NOW);
    expect(r1.ok).toBe(true);
    if (r1.ok) expect(r1.payload.hours[0].breakerTrips).toBeUndefined();

    const r2 = parseIngestPayload(makePayload({ hours: [makeBucket({ breakerTrips: 2 })] }), NOW);
    expect(r2.ok).toBe(true);
    if (r2.ok) expect(r2.payload.hours[0].breakerTrips).toBe(2);
  });

  it("breakerTrips 非法值拒绝", () => {
    expect(
      parseIngestPayload(makePayload({ hours: [makeBucket({ breakerTrips: -1 })] }), NOW).ok,
    ).toBe(false);
    expect(
      parseIngestPayload(makePayload({ hours: [makeBucket({ breakerTrips: 1e9 })] }), NOW).ok,
    ).toBe(false);
  });
});
