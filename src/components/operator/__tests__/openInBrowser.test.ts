import { afterEach, describe, expect, it, vi } from "vitest";

import { openInBrowser } from "../openInBrowser";

/**
 * `openInBrowser` 合成一次真实的 `<a target="_blank">` 点击 —— 那是本仓唯一
 * 验证过的开外链路子（Tauri 的 opener 插件接管 DOM 点击，而它的 JS 包没装）。
 *
 * 这三条钉的都是**静默失效**：属性写错 / 没进文档树 / 忘了清理，
 * 表现都是「点了按钮什么都不发生」或者页面里堆垃圾节点，没有任何报错。
 */
describe("openInBrowser", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("用 target=_blank 与 rel=noopener 点击一个 <a>", () => {
    let clicked: HTMLAnchorElement | null = null;
    // 在原型上拦 click：jsdom 不实现导航，所以只能验「点了什么」。
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked = this;
    });

    openInBrowser("https://platform.deepseek.com/api_keys");

    expect(clicked).not.toBeNull();
    expect(clicked!.href).toBe("https://platform.deepseek.com/api_keys");
    expect(clicked!.target).toBe("_blank");
    // `noopener` 不能省：少了它被打开的页面能拿到 `window.opener`。
    expect(clicked!.rel).toBe("noopener noreferrer");
  });

  it("点击时那个 <a> 在文档树里（游离节点在部分 WebView 里不触发导航）", () => {
    let wasConnected: boolean | null = null;
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      wasConnected = this.isConnected;
    });

    openInBrowser("https://example.com");

    expect(wasConnected).toBe(true);
  });

  it("点完把节点清掉（否则每次点都往 body 里堆一个）", () => {
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const before = document.body.querySelectorAll("a").length;

    openInBrowser("https://example.com");

    expect(document.body.querySelectorAll("a").length).toBe(before);
  });
});
