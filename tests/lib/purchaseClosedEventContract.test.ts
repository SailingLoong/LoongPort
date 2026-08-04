import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { PURCHASE_CLOSED_EVENT } from "@/lib/api/operator";

/**
 * 闸：**充值窗关闭事件的名字，Rust 侧与 TS 侧必须逐字一致**。
 *
 * ## 为什么这道闸值得读 `src-tauri/`
 *
 * 这是一个跨语言字符串契约，而它对不上的后果**完全静默**：
 *
 * - Rust 侧照常 `emit`，TS 侧照常 `listen`，只是两个名字不同 ⇒ 谁也收不到谁
 * - 用户看到的现象是「充完钱关掉窗口，余额没变」—— 而余额本来就是「拿不到就不显示」
 *   的附加信息，所以既不报错也不弹 toast
 * - `tsc` 管不到（两边是各自合法的字符串字面量）、单测也不会碰到（没有人断言它）
 *
 * 那个 Rust 常量的文档注释自己就预言过这件事。既然预言了就该配一道闸，
 * 而不是只写一句警告 —— 警告拦不住重命名。
 *
 * ⚠️ **本仓此前没有「测试读 `src-tauri/`」的先例**（这是第一处）。选它而不是
 * 「靠人记得同步改两处」，判据是：这条契约的失效是静默的，而静默失效必须有闸。
 * Rust 侧同样有一份镜像闸（`config.rs` 的 `brand_constant_consistency`
 * 就是反方向读 `constants.ts` 的同一个模式），所以这不是新发明的模式。
 */
describe("充值窗关闭事件的跨语言契约", () => {
  it("Rust 侧的 PURCHASE_CLOSED_EVENT 与 TS 侧逐字相同", () => {
    // 从仓库根解析（vitest 的 cwd 就是根）。**不用 `import.meta.url`** ——
    // 本仓的 vitest 配置下它不是 `file:` scheme，`fileURLToPath` 会抛
    // `ERR_INVALID_URL_SCHEME`（实测踩过）。
    const rustSource = readFileSync(
      resolve(process.cwd(), "src-tauri/src/commands/operator.rs"),
      "utf8",
    );

    // 匹配 `pub const PURCHASE_CLOSED_EVENT: &str = "...";`
    const match = rustSource.match(
      /pub const PURCHASE_CLOSED_EVENT:\s*&str\s*=\s*"([^"]+)"/,
    );

    // 找不到本身就是失败：常量被改名或删掉时，这条要红而不是静默跳过。
    expect(
      match,
      "在 commands/operator.rs 里找不到 PURCHASE_CLOSED_EVENT —— 它被改名或删了？",
    ).not.toBeNull();

    expect(match![1]).toBe(PURCHASE_CLOSED_EVENT);
  });

  it("事件名不是空串（空串会让 listen 匹配到意外的东西）", () => {
    expect(PURCHASE_CLOSED_EVENT.length).toBeGreaterThan(0);
  });
});
