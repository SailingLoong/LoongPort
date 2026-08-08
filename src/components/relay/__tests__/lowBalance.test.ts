import { describe, expect, it } from "vitest";

import {
  isLowBalance,
  LOW_BALANCE_THRESHOLD_USD,
} from "@/components/relay/lowBalance";

describe("isLowBalance", () => {
  it("在余额低于阈值时提醒", () => {
    expect(isLowBalance(0)).toBe(true);
    expect(isLowBalance(4.99)).toBe(true);
    // 负余额（欠费）当然也算低 —— 别让它因为「不在 0..5 区间」漏掉。
    expect(isLowBalance(-1)).toBe(true);
  });

  it("余额充足时不提醒", () => {
    expect(isLowBalance(5.01)).toBe(false);
    expect(isLowBalance(547.08)).toBe(false);
  });

  /**
   * ⭐ **正好等于阈值不算低** —— 需求原文是「少于 5 刀」，严格小于。
   *
   * 这条守的是把 `<` 写成 `<=` 那种一字之差：5.00 整会被提醒，
   * 而用户看着一个「余额不足 $5」旁边写着 `$5.00`，那是自相矛盾的界面。
   */
  it("正好等于阈值不算低（需求是「少于」，严格小于）", () => {
    expect(isLowBalance(LOW_BALANCE_THRESHOLD_USD)).toBe(false);
    expect(isLowBalance(5)).toBe(false);
  });

  /**
   * ⭐ **`null` 是「不知道」，不是「没钱」。**
   *
   * 它的含义是还没拉到、或中转站关了用户面板（`RelayRow` 的 `balance`
   * 文档写明了这一点）。把不知道当成低余额会给每一行都挂上叹号 ——
   * 那个提醒立刻变成噪音，而噪音等于没有提醒。
   */
  it("余额未知（null）时不提醒", () => {
    expect(isLowBalance(null)).toBe(false);
  });

  /**
   * 阈值是维护者拍板的 5 美元。
   *
   * 断言这个具体数字而不是只测函数行为：改动它是一个**产品决定**，
   * 该有一条测试在改的那一刻被看见（而不是静默生效）。
   */
  it("阈值是 5 美元", () => {
    expect(LOW_BALANCE_THRESHOLD_USD).toBe(5);
  });
});
