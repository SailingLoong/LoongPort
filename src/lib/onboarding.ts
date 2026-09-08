/**
 * 新人引导（前端侧）。后端策略在 `src-tauri/src/relay/onboarding.rs` 与
 * `src-tauri/src/commands/onboarding.rs`，那边说了算。
 *
 * 全程不弹任何邀约（2026-09-06 起，连首导入成功后的 Star 弹窗事件也删了）：
 * Star 礼的入口只剩顶栏 GitHub 红点。这里只剩注册窗完成事件的 payload 类型。
 */

/** `ONBOARDING_REGISTER_COMPLETED` 事件的 payload（与 Rust 侧
 * `events::RegisterCompletedPayload` 对应；生产者是注册窗与浏览器接力登录）。 */
export interface OnboardingRegisterCompleted {
  relayId: number;
  siteName: string;
}
