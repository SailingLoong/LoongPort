import { invoke } from "@tauri-apps/api/core";

/**
 * 「点 Star 领注册礼」（后端机制层在 `src-tauri/src/commands/star_reward.rs`）。
 *
 * 配置（码 + 额度）的唯一数据源是远端配置的 `star_reward` 块 —— 整块缺席
 * = 活动下线，`offer` 返回 null，一切回落现状行为。
 * 界面上每个「$N」都从 `offer.amountUsd` 来，前端不另存数值。
 */

/** Star 对话框的 payload（`star_reward_offer` 命令返回；与 Rust 侧
 * `commands::star_reward::StarRewardOffer` 对应。曾经的主动弹窗事件已删，
 * 顶栏红点是唯一入口）。 */
export interface StarRewardOffer {
  promoCode: string;
  amountUsd: number;
}

export const starRewardApi = {
  /** 红点入口的邀请：null = 活动不在，回落「直接开仓库」。纯本地读缓存，无等待。 */
  async offer(): Promise<StarRewardOffer | null> {
    return await invoke("star_reward_offer");
  },

  /** 标记已领取（发码时刻调用）。后端专有事实走窄命令 RMW，不走全量 save。 */
  async markClaimed(): Promise<void> {
    await invoke("star_reward_mark_claimed");
  },

  /** 打开官方站注册窗并预填奖励码（发码后的终点）。 */
  async openRegisterWindow(promoCode: string): Promise<void> {
    await invoke("onboarding_open_register_window", { promoCode });
  },
};
