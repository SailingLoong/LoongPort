import { describe, expect, it } from "vitest";

import { resolvePresetReferralUrl } from "@/lib/presetReferrals";

describe("resolvePresetReferralUrl", () => {
  const referrals = {
    "packyapi.ai": "https://www.packyapi.ai/register?aff=loongport",
    "aicoding.inc": "https://aicoding.inc/i/LOONGPORT",
  };

  it("精确 host 命中时返回覆盖 URL", () => {
    expect(
      resolvePresetReferralUrl("https://aicoding.inc/register", referrals),
    ).toBe("https://aicoding.inc/i/LOONGPORT");
  });

  it("预设链接带 www. 而配置键是裸域时仍命中", () => {
    expect(
      resolvePresetReferralUrl("https://www.packyapi.ai/register", referrals),
    ).toBe("https://www.packyapi.ai/register?aff=loongport");
  });

  it("大小写不同的 host 归一后命中", () => {
    expect(
      resolvePresetReferralUrl("https://WWW.PackyAPI.ai/register", referrals),
    ).toBe("https://www.packyapi.ai/register?aff=loongport");
  });

  it("未命中返回 null（调用方回落中性链接）", () => {
    expect(
      resolvePresetReferralUrl("https://unknown.example/register", referrals),
    ).toBeNull();
  });

  it("候选 URL 解析失败或缺失时返回 null，不抛异常", () => {
    expect(resolvePresetReferralUrl("not a url", referrals)).toBeNull();
    expect(resolvePresetReferralUrl("", referrals)).toBeNull();
    expect(resolvePresetReferralUrl(undefined, referrals)).toBeNull();
    expect(resolvePresetReferralUrl("https://packyapi.ai", null)).toBeNull();
    expect(resolvePresetReferralUrl("https://packyapi.ai", {})).toBeNull();
  });
});
