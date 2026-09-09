import { describe, expect, it } from "vitest";

import { parseStatsReport } from "./ping";

// 测试用 example 域名与全零 UUID（公开仓隐私纪律，不出现真实站点）。
function makeReport(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    installId: "01234567-89ab-cdef-0123-456789abcdef",
    appVersion: "6.20.0",
    os: "macos",
    siteHosts: ["example.com", "relay.example"],
    relayAccountCount: 3,
    ...overrides,
  };
}

describe("parseStatsReport", () => {
  it("接受合法上报并把站点列表排成规范序", () => {
    const r = parseStatsReport(
      makeReport({ siteHosts: ["relay.example", "example.com", "relay.example"] }),
    );
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    // 排序 + 去重是服务端自己的规范化：不信任客户端的顺序（排序本身防指纹）。
    expect(r.report.siteHosts).toEqual(["example.com", "relay.example"]);
    expect(r.report.installId).toBe("01234567-89ab-cdef-0123-456789abcdef");
  });

  it("空站点列表也是合法样本（装了没用起来的用户）", () => {
    const r = parseStatsReport(makeReport({ siteHosts: [], relayAccountCount: 0 }));
    expect(r).toEqual({
      ok: true,
      report: {
        installId: "01234567-89ab-cdef-0123-456789abcdef",
        appVersion: "6.20.0",
        os: "macos",
        siteHosts: [],
        relayAccountCount: 0,
      },
    });
  });

  it("installId 只认带横线的 UUID v4（客户端 generateUUID 的形状）", () => {
    // 32 位简单 hex（那是 crowd source id 的形状，不是 install id）
    expect(parseStatsReport(makeReport({ installId: "0123456789abcdef0123456789abcdef" })).ok).toBe(false);
    // 大写 / 乱串
    expect(parseStatsReport(makeReport({ installId: "01234567-89AB-CDEF-0123-456789ABCDEF" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ installId: "not-a-uuid" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ installId: 42 })).ok).toBe(false);
  });

  it("appVersion 拒绝任意文本与超长串", () => {
    expect(parseStatsReport(makeReport({ appVersion: "6.20.0-beta.1+build" })).ok).toBe(true);
    expect(parseStatsReport(makeReport({ appVersion: "" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ appVersion: "has space" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ appVersion: "6.20.0/../../../etc" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ appVersion: "a".repeat(40) })).ok).toBe(false);
  });

  it("os 只认客户端 current_os 的四个取值", () => {
    for (const os of ["macos", "windows", "linux", "other"]) {
      expect(parseStatsReport(makeReport({ os })).ok).toBe(true);
    }
    // 带版本号 / 任意串都不收（os 不带版本号是隐私评审定的）
    expect(parseStatsReport(makeReport({ os: "macos 15.3" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ os: "darwin" })).ok).toBe(false);
  });

  it("siteHosts 逐条过与 ingest 同一套站点校验", () => {
    // scheme / 端口 / 大写 / www 前缀 / IP 字面量都不收（与 isValidSite 同判据）
    expect(parseStatsReport(makeReport({ siteHosts: ["https://example.com"] })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ siteHosts: ["example.com:8443"] })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ siteHosts: ["Example.com"] })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ siteHosts: ["www.example.com"] })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ siteHosts: ["192.168.1.1"] })).ok).toBe(false);
    // 不是数组 / 条目不是串
    expect(parseStatsReport(makeReport({ siteHosts: "example.com" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ siteHosts: [42] })).ok).toBe(false);
    // 超过上限（防垃圾填充）
    expect(
      parseStatsReport(makeReport({ siteHosts: Array.from({ length: 65 }, () => "example.com") })).ok,
    ).toBe(false);
  });

  it("relayAccountCount 只收安全非负整数", () => {
    expect(parseStatsReport(makeReport({ relayAccountCount: -1 })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ relayAccountCount: 1.5 })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ relayAccountCount: "3" })).ok).toBe(false);
    expect(parseStatsReport(makeReport({ relayAccountCount: 10_001 })).ok).toBe(false);
  });

  it("非对象载荷整体拒绝", () => {
    expect(parseStatsReport(null).ok).toBe(false);
    expect(parseStatsReport("x").ok).toBe(false);
    expect(parseStatsReport([]).ok).toBe(false);
  });
});
