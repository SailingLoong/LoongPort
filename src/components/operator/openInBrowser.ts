/**
 * 在系统浏览器里打开一个外链，**从非 DOM 的上下文**（如 toast 的 action 回调）。
 *
 * ## 为什么不是 `window.open`
 *
 * Tauri 的 opener 插件在 Rust 侧接管的是 **DOM 里链接的点击**
 * （`tauri_plugin_opener::init()`，capability 里的 `opener:default`）。
 * 而它的 **JS 包 `@tauri-apps/plugin-opener` 本仓没装** ⇒ 没有 `openUrl()` 可调，
 * `window.open` 在 WebView 里也不保证被送到系统浏览器（最坏是被吞掉 ——
 * 用户点了按钮什么都不发生，且不报错）。
 *
 * 仓里既有的四处外链（`ApiKeySection` / `CodexOAuthSection` / `XaiOAuthSection` /
 * `CopilotAuthSection`）全部是 `<a target="_blank" rel="noopener noreferrer">`，
 * 那是本仓**唯一验证过**的路子。
 *
 * ## 所以这里合成一次真实的 `<a>` 点击
 *
 * 不是绕过那条路，而是**程序化触发同一条路**：走的仍然是 DOM 点击那条链，
 * 只是点击源是代码而不是用户的鼠标 —— toast 的 action 只吃 `onClick`，
 * 没法在那里塞一个 `<a>` 让用户点。
 *
 * `rel` 与那四处逐字一致（`noopener` 防被打开的页面拿到 `window.opener`）。
 */
export function openInBrowser(url: string): void {
  const a = document.createElement("a");
  a.href = url;
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  // 必须进文档树：游离节点上的 `click()` 在部分 WebView 里不触发导航。
  // 同步移除 —— 点击事件是同步派发的，走完这一行链接已经处理完了。
  document.body.appendChild(a);
  a.click();
  a.remove();
}
