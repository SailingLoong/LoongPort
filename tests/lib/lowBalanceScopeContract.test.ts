import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 低余额提醒由后端决定，前端只展示 `shouldPromptTopUp`。
 *
 * ## 为什么这是个正确性问题，不是产品偏好
 *
 * 阈值是 **5 美元**（sub2api 的钱包就是 USD 计价），而官网行是 DeepSeek 的
 * **人民币**钱包（`unit: "CNY"`）。拿人民币余额跟一个美元阈值比是错的（差着汇率）。
 *
 * ⚠️ 2026-08-04 维护者明确要求：「只对中转站生效，对 deepseek 之类的不生效」。
 *
 * ## ⚠️ 2026-08-13 之后这条闸更重要了，不是更没用了
 *
 * 那一轮把两类行的余额契约统一成了上游的 `UsageResult`（原来是 relay 的 `number`
 * vs vendor 的已格式化字符串），两类行也从此共用同一个 `RowBalance` 组件。
 * **类型上那道天然隔离没有了** —— 现在只剩组件里一句显式判据顶着：`low` 只在
 * 有充值入口（`onPurchase`，只有中转站行传）时才算。
 *
 * 所以这道闸从「守两个类型不许合并」改成「守那句判据不许被简化掉」。
 *
 * ## 为什么读源码断言，而不是渲染组件
 *
 * 要测的是「vendor 那条路压根到不了这个判据」这件**结构性事实**。渲染测试只能验
 * 「某个具体余额值不出叹号」—— 那种断言在有人把判据放开、只是阈值凑巧没命中时
 * 照样通过。同型于 `vendorSwitchGuardContract.test.ts`（仓库已有这个惯例）。
 */
describe("低余额提醒的作用域", () => {
  const read = (rel: string) =>
    readFileSync(resolve(__dirname, "../../src", rel), "utf8");

  it("VendorRow 不引入低余额判据", () => {
    const vendorRow = read("components/relay/VendorRow.tsx");
    expect(vendorRow).not.toContain("isLowBalance");
    expect(vendorRow).not.toContain("LOW_BALANCE_THRESHOLD");
    expect(vendorRow).not.toContain("lowBalanceHint");
    expect(vendorRow).not.toMatch(/onPurchase\s*[={]/);
  });

  /**
   * ⭐ **`low` 必须以「有没有充值入口」为前置条件。**
   *
   * 这条守的是把它简化成 `isLowBalance(remaining ?? null)` 那种改法 —— 那样一改，
   * 官网行的人民币余额就会拿去跟 5 美元比，低于 ¥5 之外的区间全部误判，
   * 而且不会有任何报错。
   */
  it("RowBalance 不读取余额数字或维护阈值", () => {
    const rowBalance = read("components/relay/RowBalance.tsx");
    expect(rowBalance).not.toContain("isLowBalance");
    expect(rowBalance).not.toContain("LOW_BALANCE_THRESHOLD");
    expect(rowBalance).not.toContain("usage.data?.[0]?.remaining");
    expect(rowBalance).toContain("shouldPromptTopUp");
  });

  it("中转站行真的用上了它 —— 提醒要出现在中转站行上", () => {
    const relayRow = read("components/relay/RelayRow.tsx");
    const rowBalance = read("components/relay/RowBalance.tsx");
    // 中转站行传了充值入口 ⇒ 判据对它成立。
    expect(relayRow).toContain("onPurchase={onPurchase}");
    expect(rowBalance).toContain("shouldPromptTopUp");
    expect(rowBalance).toContain("lowBalanceHint");
  });

  /**
   * ⭐ **余额为 0 时必须仍然渲染** —— 那正是最该提醒的一刻。
   *
   * 这条守的是 `{remaining && ...}` 那种写法：`0` 是 falsy ⇒ 余额刚好花完时
   * 整块消失，用户看到的是「没有余额这一项」而不是「余额 0.00 + 叹号」。
   *
   * 现在这件事由呈现件 `InlineUsage` 保证：它判的是 `!== undefined`
   * （`undefined` 才是「没有这个字段」）。
   */
  it("余额为 0 时不被 falsy 判据吞掉", () => {
    const usageFooter = read("components/UsageFooter.tsx");
    expect(usageFooter).toContain("firstUsage.remaining !== undefined");
    expect(usageFooter).not.toContain("{firstUsage.remaining && (");
  });
});
