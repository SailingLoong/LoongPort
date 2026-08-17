/**
 * 新人引导（前端侧）。后端策略在 `src-tauri/src/relay/onboarding.rs` 与
 * `src-tauri/src/commands/onboarding.rs`，那边说了算。
 *
 * 「点 Star 领注册礼」的主动邀请已不在前端发起（2026-08-17 起）：后端挂在
 * 首个站点接入成功（`import_site` 成功路径）之后直接发
 * `ONBOARDING_STAR_REWARD_OFFER` 事件，App 层的监听照旧弹窗。这里只剩
 * 注册窗完成事件的 payload 类型。
 */

/** `ONBOARDING_REGISTER_COMPLETED` 事件的 payload（与 Rust 侧
 * `commands::onboarding::RegisterCompletedPayload` 对应）。 */
export interface OnboardingRegisterCompleted {
  relayId: number;
  siteName: string;
}
