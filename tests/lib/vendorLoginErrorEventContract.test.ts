import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { PURCHASE_CLOSED, VENDOR_LOGIN_ERROR } from "@/lib/api/events";

/**
 * 闸：**官网登录出错事件的名字，Rust 侧与 TS 侧必须逐字一致**。
 *
 * 与 `purchaseClosedEventContract.test.ts` 同一个模式、同一条理由 ——
 * 跨语言字符串契约，对不上的后果**完全静默**：Rust 照常 `emit`，TS 照常 `listen`，
 * 只是两个名字不同 ⇒ 用户走完登录流程、凭据解析失败，界面上**什么都不发生**
 * （而那正是最需要提示的一刻，不说他会反复重登）。`tsc` 管不到，单测碰不到。
 *
 * CLAUDE.md §三点六：新增任何「跨语言 / 跨文件的同一事实」时一并加闸 ——
 * 否则它迟早分叉，而分叉那天没人会收到通知。
 *
 * 2026-08-07 前 `VENDOR_LOGIN_ERROR_EVENT` 定义在 `commands/vendor.rs`，迁入
 * `events.rs` / `events.ts` 统一管理（名字也随迁为 `VENDOR_LOGIN_ERROR`）。
 */
describe("官网登录出错事件的跨语言契约", () => {
  it("Rust 侧的 VENDOR_LOGIN_ERROR 与 TS 侧逐字相同", () => {
    const rustSource = readFileSync(
      resolve(process.cwd(), "src-tauri/src/events.rs"),
      "utf8",
    );

    const match = rustSource.match(
      /pub const VENDOR_LOGIN_ERROR:\s*&str\s*=\s*"([^"]+)"/,
    );

    expect(
      match,
      "在 src-tauri/src/events.rs 里找不到 VENDOR_LOGIN_ERROR —— 它被改名或删了？",
    ).not.toBeNull();

    expect(match![1]).toBe(VENDOR_LOGIN_ERROR);
  });

  /**
   * 两条事件名**有意不同**（vendor 与 relay 的登录窗各自独立）。
   *
   * 钉住它是因为「统一成一个」看起来像是清理重复，实际会让一边的登录错误弹在
   * 另一边的界面上 —— 而 Rust 侧已有测试钉着 vendor 那条的取值。
   */
  it("与 relay 的事件名不同（两条链路各自独立）", () => {
    expect(VENDOR_LOGIN_ERROR).not.toBe(PURCHASE_CLOSED);
    expect(VENDOR_LOGIN_ERROR).not.toBe("relay-login-error");
  });
});
