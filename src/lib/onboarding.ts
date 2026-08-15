import { invoke } from "@tauri-apps/api/core";

/**
 * 新人引导（前端侧）。后端策略在 `src-tauri/src/relay/onboarding.rs` 与
 * `src-tauri/src/commands/onboarding.rs`，那边说了算；这里只保证一件事：
 * **一次进程内最多发起一次**引导询问 —— 多处首启判定都可能碰它，靠模块级
 * 单例把多次调用收敛成一次 invoke。
 *
 * 询问的内容是「点 Star 领注册礼」弹窗（弹不弹由后端三道闸收口：资格 +
 * 远端配置 + 基线星数），返回值没有业务分支 —— 广场页总是要落的。
 */
let promptPromise: Promise<void> | null = null;

/** 问后端要不要给这个新用户弹「点 Star 领注册礼」邀请（后端判完直接发事件）。 */
export function promptOnboardingStarReward(): Promise<void> {
  promptPromise ??= invoke<void>("onboarding_prompt_star_reward").catch(
    (error) => {
      // 引导是加分项：后端暂时不可用时静默降级，不为它弹错误打扰新用户。
      console.warn("[onboarding] star 领礼邀请未能发起", error);
    },
  );
  return promptPromise;
}

/** `ONBOARDING_REGISTER_COMPLETED` 事件的 payload（与 Rust 侧
 * `commands::onboarding::RegisterCompletedPayload` 对应）。 */
export interface OnboardingRegisterCompleted {
  relayId: number;
  siteName: string;
}
