import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 守「只有会动 codex 配置的那一屏才提 ChatGPT」这件事。
 *
 * ## 修的是什么
 *
 * 2026-08-05 维护者实测：**在 claude 页面切档位，也会弹「要不要退出 ChatGPT」的确认框，
 * 选完还提示「请手动重启 ChatGPT」** —— 而 claude 档位写的是 `~/.claude/settings.json`，
 * 跟 ChatGPT 桌面版毫无关系。用户被要求为一件不存在的因果做决定，而且那个决定
 * （强制杀掉 ChatGPT）在 Windows 上是不可逆的。
 *
 * 根因：判据只用了 `chatgptNeedsAttention`，而它的语义是「**这台机器上**要不要提示
 * 处理 ChatGPT」—— 只关平台与安装状态（非 macOS 恒为 true，见
 * `chatgpt_app::needs_user_attention`），**完全不含「切的是哪个 app」**。
 * 那一处此前的注释声称它「已经包含这个事实」，那是不属实的。
 *
 * 后端一直是对的（`relay.rs` 那句「不需要碰 ChatGPT（非 codex，或用户选了只切换）」），
 * 只是前端没把 app 这一维传进判据。
 *
 * ## 为什么用读源码断言而不是渲染组件
 *
 * 与 `SwitchTierConfirmDialogPlatformCopy.test.ts` 同一套路：要守的是**判据里含 app
 * 这一维**这件事本身。渲染要搭一整套 relay 数据（站点、档位、余额、mock 六个命令），
 * 而那些与这条判据无关 —— 搭出来的复杂度会掩盖它真正在守什么。
 *
 * ⚠️ 这类断言的已知弱点是**改写法就失效**（把 `&&` 换成提前 return 也是对的实现，
 * 但这条会误报红）。接受它：误报会让人来读这段说明，而漏报会让 bug 悄悄回来 ——
 * 两种失败里前者可修，后者是这条测试存在的理由。
 */
const source = readFileSync(
  resolve(__dirname, "../../src/components/relay/RelaySection.tsx"),
  "utf-8",
);

describe("ChatGPT 确认框只在 codex 那一屏出现", () => {
  it("判据里含 app 这一维，不只看 chatgptNeedsAttention", () => {
    expect(
      source,
      "RelaySection 里应有一个按 appId 判断的派生值（当前叫 touchesCodexConfig）—— " +
        "少了它就会在 claude / gemini 页面问一件与 ChatGPT 无关的事",
    ).toContain("touchesCodexConfig");
  });

  it("那个派生值确实按 appId 算，且只认 codex", () => {
    expect(source).toMatch(/touchesCodexConfig\s*=\s*appId === "codex"/);
    // `codex-image` 有意不算：生图档位落的是 MCP 条目、不改 codex 主模型与 base_url，
    // ChatGPT 桌面版不消费它；且首次注册本来就要用户新开终端。
    // 若将来实测发现桌面版确实受影响再加，但要有依据。
    expect(
      source,
      "codex-image 不该被算进去（除非有实测依据），见那个派生值的说明",
    ).not.toMatch(/touchesCodexConfig[^;]*codex-image/);
  });

  it("三条切换路径都用上了这个判据", () => {
    // 中转站档位（handleSwitchTier）、官网直连（handleVendorUse）和指定模型
    // （handleSelectTierModel）各一处，不能只修其中一条。
    const guarded = source.match(
      /if \(touchesCodexConfig && chatgptNeedsAttention\)/g,
    );
    expect(
      guarded?.length,
      "应有三处（中转站档位 + 官网直连账号 + 指定模型）都带上 touchesCodexConfig",
    ).toBe(3);
  });

  it("没有残留的裸 chatgptNeedsAttention 判断", () => {
    // 这条是防「只改了一处」：任何 `if (chatgptNeedsAttention)` 形式的裸判断都说明
    // 有一条路径又会在无关的页面上问 ChatGPT。
    expect(
      source.match(/if \(chatgptNeedsAttention\)/g),
      "还有裸的 `if (chatgptNeedsAttention)` —— 那条路径会在 claude 页面问无关的事",
    ).toBeNull();
  });
});
