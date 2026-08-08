import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * vendor 的「使用」按钮必须走 `relay_switch_tier`，不能走上游的 `switch_provider`。
 *
 * ## 为什么需要这条闸
 *
 * vendor 的 provider id 是 `loongport-vendor-<hash>`，命中 `MANAGED_ID_PREFIX`
 * （那是有意的设计 —— 为了继承托管守卫、让 provider 页不给编辑入口）。
 * 而上游的 `switch_provider`（`src-tauri/src/commands/provider.rs`）第一件事就是
 * `reject_if_managed(id)` ⇒ **走那条路必被拦下**，用户看到
 * 「请在 LoongPort 页面里操作」，而那个页面没有官网行 ⇒ 无路可走。
 *
 * 初版就是这么写的（论证成「零改动复用上游」，漏掉了守卫这一层），
 * final review 实测抓出：`reject_if_managed("loongport-vendor-0c0a…")` → `Err`。
 *
 * ⚠️ **回归时的症状是静默的** —— toast 里那句「请在 LoongPort 页面里操作」
 * 看着像正常业务提示，不像 bug。所以必须有闸。
 */
describe("vendor 切换入口", () => {
  const src = readFileSync(
    resolve(__dirname, "../../src/components/relay/RelaySection.tsx"),
    "utf8",
  );

  /** 只取 `handleVendorUse` 那个函数体（避免中转站那条路的调用干扰断言）。 */
  const vendorUseBody = (() => {
    const start = src.indexOf("const handleVendorUse");
    expect(start).toBeGreaterThan(-1);
    // 到下一个 `const doRemoveVendor` / `const ` 顶层声明为止够用
    const end = src.indexOf("const doRemoveVendor", start);
    expect(end).toBeGreaterThan(start);
    return src.slice(start, end);
  })();

  it("走 relayApi.switchTier（在守卫之内）", () => {
    expect(vendorUseBody).toContain("relayApi.switchTier");
  });

  it("不走 providersApi.switch（会被 reject_if_managed 拦下）", () => {
    expect(vendorUseBody).not.toContain("providersApi.switch");
  });
});
