import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 低余额提醒**只对中转站（relay）行生效，不对官网直连（vendor）行生效**。
 *
 * ## 为什么这是个正确性问题，不是产品偏好
 *
 * 两侧余额的**类型和币种都不同**：
 *
 * | | 类型 | 值的样子 | 币种 |
 * |---|---|---|---|
 * | relay | `number \| null` | `547.08` | **美元**（sub2api 的 balance 就是 USD 计价） |
 * | vendor | `string \| null` | `"¥547.08"` | **人民币**（DeepSeek 官网） |
 *
 * 拿人民币余额跟一个 5 **美元**的阈值比是错的（差着汇率）。而且 vendor 那侧是
 * 后端已格式化好的字符串，前端要比较就得反解析它 —— 那会把「格式化在 Rust 侧做完」
 * 这条设计打破。
 *
 * ⚠️ 2026-08-04 维护者明确要求：「只对中转站生效，对 deepseek 之类的不生效」。
 *
 * ## 为什么读源码断言，而不是渲染组件
 *
 * 要测的是「`VendorRow` 压根没引入这个判据」这件**结构性事实**。
 * 渲染测试只能验「某个具体余额值不出叹号」—— 那种断言在有人给 vendor
 * 加上判据、只是阈值凑巧没命中时照样通过。同型于
 * `vendorSwitchGuardContract.test.ts`（仓库已有这个惯例）。
 */
describe("低余额提醒的作用域", () => {
  const read = (rel: string) =>
    readFileSync(resolve(__dirname, "../../src", rel), "utf8");

  it("VendorRow 不引入低余额判据 —— 那边余额是人民币字符串，比不了美元阈值", () => {
    const vendorRow = read("components/relay/VendorRow.tsx");
    expect(vendorRow).not.toContain("isLowBalance");
    expect(vendorRow).not.toContain("LOW_BALANCE_THRESHOLD");
    expect(vendorRow).not.toContain("lowBalanceHint");
  });

  it("RelayRow 引入了它 —— 提醒要真的出现在中转站行上", () => {
    const relayRow = read("components/relay/RelayRow.tsx");
    expect(relayRow).toContain("isLowBalance");
    expect(relayRow).toContain("lowBalanceHint");
  });

  /**
   * ⭐ **余额为 0 时必须仍然渲染** —— 那正是最该提醒的一刻。
   *
   * 这条守的是 `{balance && ...}` / `if (!balance) return null` 那种写法：
   * `0` 是 falsy ⇒ 余额刚好花完时整块消失，用户看到的是「没有余额这一项」
   * 而不是「余额 $0.00 + 叹号」。判据必须是显式的 `=== null`（`null` 才是「不知道」）。
   *
   * 2026-08-04：中转站余额原本有两处显示（这里 + 已删的 LoongPort 独立页顶部），
   * 那道「两处判据必须一致」的断言随那个页面一起去掉了。现在只有这一处。
   */
  it("余额为 0 时不被 falsy 判据吞掉", () => {
    const relayRow = read("components/relay/RelayRow.tsx");
    expect(relayRow).toContain("balance === null) return null");
    expect(relayRow).not.toContain("{balance && (");
  });

  /**
   * ⭐ **vendor 的余额必须仍是字符串类型** —— 这条守的是「别为了复用提醒
   * 而把 vendor 余额改成数字」那种改法。
   *
   * 那样改会连带打破两件事：后端那套纯字符串的两位小数进位（`1.005` 要显示
   * 成 `¥1.01`，走 `f64` 会变 `1.00`），以及「币种符号已经在字符串里」
   * 这个有意的设计（见 Rust 侧 `VendorBalance` 的文档：只有一个字段）。
   */
  it("vendor 的余额契约仍是「已格式化的字符串」", () => {
    const vendorRow = read("components/relay/VendorRow.tsx");
    expect(vendorRow).toContain("balance: string | null");
  });
});
