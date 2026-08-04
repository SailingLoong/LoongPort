import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { VENDOR_APPS, vendorSupportsApp } from "@/lib/api/vendor";

/**
 * 闸：**「官网账号在哪些平台可用」这个事实，Rust 侧与 TS 侧必须一致**。
 *
 * Rust 侧的权威定义是 `vendor::provision::DEEPSEEK_APPS`（决定 provision 真的写出
 * 哪几条 provider 记录）；TS 侧的 `VENDOR_APPS` 决定官网行在哪几个 tab 出现。
 *
 * ## 两处分叉的后果都是静默的
 *
 * - TS 多一个（如把 `gemini` 也算进去）⇒ 那个 tab 显示官网行，但它名下**永远没有
 *   provider 记录**（Rust 不生成）⇒ 用户点「使用」拿到一个找不到的 provider id
 * - TS 少一个 ⇒ 那个平台的配置**已经写好了却看不到入口**，用户以为没支持
 *
 * 编译器管不到（两边是各自合法的字面量），所以按 CLAUDE.md §三点六 加这道闸。
 *
 * ⚠️ 两边的写法不同名：Rust 是 `AppType::ClaudeDesktop` 这样的变体名，TS 是
 * `"claude-desktop"` 这样的 kebab-case `app_type`。所以比的是**换算后的集合**，
 * 换算表就在下面 —— 加第七个平台时这里会红，那正是要的（提醒你两边都改）。
 */

/** `AppType` 变体名 → kebab-case `app_type`（与 Rust 侧 `as_str()` 一致）。 */
const VARIANT_TO_APP_ID: Record<string, string> = {
  Codex: "codex",
  Claude: "claude",
  ClaudeDesktop: "claude-desktop",
  Hermes: "hermes",
  OpenClaw: "openclaw",
  OpenCode: "opencode",
  Gemini: "gemini",
  GrokBuild: "grokbuild",
};

describe("官网账号支持哪些平台的跨语言契约", () => {
  it("Rust 侧的 DEEPSEEK_APPS 与 TS 侧的 VENDOR_APPS 是同一个集合", () => {
    const rustSource = readFileSync(
      resolve(process.cwd(), "src-tauri/src/vendor/provision.rs"),
      "utf8",
    );

    // 匹配 `pub const DEEPSEEK_APPS: [AppType; 6] = [ ... ];` 的方括号内容。
    const block = rustSource.match(
      /pub const DEEPSEEK_APPS:\s*\[AppType;\s*\d+\]\s*=\s*\[([^\]]+)\]/,
    );
    expect(
      block,
      "在 vendor/provision.rs 里找不到 DEEPSEEK_APPS —— 它被改名或改形状了？",
    ).not.toBeNull();

    const variants = Array.from(
      block![1].matchAll(/AppType::(\w+)/g),
      ([, name]) => name,
    );
    // 空数组会让下面的断言在两边都空时假通过 —— 先钉住解析真的抓到了东西。
    expect(variants.length).toBeGreaterThan(0);

    const fromRust = variants.map((v) => {
      const appId = VARIANT_TO_APP_ID[v];
      expect(
        appId,
        `AppType::${v} 不在换算表里 —— 加平台时请一并补 VARIANT_TO_APP_ID`,
      ).toBeDefined();
      return appId;
    });

    // 比集合而不是顺序：Rust 那边的顺序决定 `sort_index`，TS 这边只用于「在不在」判断。
    expect([...fromRust].sort()).toEqual([...VENDOR_APPS].sort());
  });

  it("gemini 与 grokbuild 不在里面（上游无 DeepSeek preset，协议不兼容）", () => {
    expect(vendorSupportsApp("gemini")).toBe(false);
    expect(vendorSupportsApp("grokbuild")).toBe(false);
    expect(vendorSupportsApp("codex")).toBe(true);
  });
});
