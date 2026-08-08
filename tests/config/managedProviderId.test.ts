import { describe, expect, it } from "vitest";
import { isManagedProviderId } from "@/config/managedProviderId";

/**
 * 闸：**前端的托管判据必须与 Rust 侧同形。**
 *
 * ## 为什么前端也要收紧，而不是「后端拦住就行」
 *
 * 两侧判据不一致的后果不是少拦一次，而是**换一种死局**：
 *
 * - 只收紧 Rust：`loongport-mine` 在前端被前缀过滤掉（列表里看不见），
 *   而后端已经放行 ⇒ 用户那条 provider 从界面上消失了，却又没有任何守卫解释它去哪了。
 * - 只收紧前端：后端仍拦 ⇒ 列表里看得见、点编辑保存报错，指向一个没有它的页面。
 *
 * 所以这条判据必须两侧同时收紧。同一事实散在 Rust 与 TS 两处的通用解法见
 * `src-tauri/src/relay/managed.rs` 的 `prefix_matches_the_frontend_copy`
 * （`include_str!` 字面比对）—— 那道闸守常量，这道守行为。
 */
describe("前端托管 provider 判据", () => {
  /**
   * 正面：两个生成端真正产出的形状。**这里只能写字面量**（TS 侧没有生成器可调），
   * 所以形状的权威源在 Rust —— 与那边同步靠 `managed.rs` 里
   * `both_generators_produce_ids_the_guard_recognizes` 那条测试钉住生成器，
   * 加上本文件与 `prefix_matches_the_frontend_copy` 钉住前缀。
   */
  it("认出两个生成端的 id", () => {
    // relay：前缀 + 16 位小写 hex
    expect(isManagedProviderId("loongport-0123456789abcdef")).toBe(true);
    // vendor：前缀 + "vendor-" + 16 位小写 hex
    expect(isManagedProviderId("loongport-vendor-fedcba9876543210")).toBe(true);
  });

  /**
   * 反面：用户手填 / deeplink 能造出来的。这些命中判据就等于把他的 provider 锁死
   * （编辑被拦、删除被拦、列表里消失），而 UI 上没有逃生路径。
   */
  it("不把用户能造出来的 id 当托管", () => {
    for (const id of [
      // opencode / openclaw / hermes 的 provider id 就是用户手填的 key，
      // 前端正则 /^[a-z0-9]+(-[a-z0-9]+)*$/ 放行它
      "loongport-mine",
      "loongport-my-provider",
      // deeplink 的 {name}-{timestamp}：名字填 LoongPort 正好命中前缀
      "loongport-1785818820765",
      // 长度不对
      "loongport-abc",
      "loongport-0123456789abcdef0",
      // 字符集不对：hex 里没有 g
      "loongport-0123456789abcdefg",
      // 大写：`{:x}` 恒产出小写，放行大写会把判据重新放宽
      "loongport-0123456789ABCDEF",
      "loongport-",
      // vendor 那支前缀对、hex 段不对
      "loongport-vendor-nothex0123456",
      // 完全无关的
      "custom-1",
      "codex-official",
      "",
      "loongport",
      "LoongPort-0123456789abcdef",
    ]) {
      expect(isManagedProviderId(id), `id: ${id}`).toBe(false);
    }
  });
});
