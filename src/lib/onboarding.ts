import { invoke } from "@tauri-apps/api/core";

/**
 * 新人引导（前端侧）。后端策略在 `src-tauri/src/relay/onboarding.rs` 与
 * `src-tauri/src/commands/onboarding.rs`，那边说了算；这里只保证一件事：
 * **一次进程内最多发起一次**引导询问 —— `App` 挂载与 `RelaySection` 的首启
 * 判定都可能碰它，靠模块级单例把多次调用收敛成一次 invoke。
 *
 * 完成后的 toast / 档位预配 / 列表刷新不在这里：那些是 `RelaySection` 的
 * 既有职责（监听 `ONBOARDING_REGISTER_COMPLETED`）。
 */
let promptPromise: Promise<boolean> | null = null;

/**
 * 问后端「要不要弹新人引导注册窗」。返回 `true` = 本次调用触发了弹窗
 *（本进程的首次且用户仍是新用户）；`false` = 已弹过 / 不是新用户 / 后端不可用。
 */
export function promptOnboardingRegister(): Promise<boolean> {
  promptPromise ??= invoke<boolean>("onboarding_prompt_register").catch(
    (error) => {
      // 引导是加分项：后端暂时不可用时静默降级（false = 走既有的首启目录提示），
      // 不为它弹错误打扰新用户。
      console.warn("[onboarding] 新人引导注册窗未能发起", error);
      return false;
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
