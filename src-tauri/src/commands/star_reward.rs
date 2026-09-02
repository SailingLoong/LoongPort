//! 「点 Star 领注册礼」的机制层：弹窗邀请 payload 的组装与领取标志落库。
//!
//! 策略（什么时候弹、弹给谁）在 `commands::onboarding`（首个站点接入后邀请）
//! 与前端 `GitHubStarButton` / `StarRewardDialog`（红点入口与弹窗状态机）；
//! 本模块只提供两端共用的机制：
//! - 邀请 payload：纯本地组装（读远端配置缓存，不打任何网络）；
//! - `star_reward_claimed` 的窄命令 RMW 落库。
//!
//! 发放语义是**荣誉制**：点击「领取」= 打开浏览器仓库页 + 当场发码，**有意
//! 不做任何 Star 校验**。校验（gh CLI 代点 / 前后星数比对）都要打 GitHub API，
//! 国内网络下是长达 20s 的无反馈等待，用户感知就是「卡死」—— 低摩擦换转化，
//! 不为校验付出这个代价。

use serde::Serialize;

/// 弹窗邀请的 payload：`ONBOARDING_STAR_REWARD_OFFER` 事件与 `star_reward_offer`
/// 命令共用；前端 `src/lib/api/starReward.ts` 的 `StarRewardOffer` 与之对应。
///
/// 序列化 camelCase（本仓 TS 侧惯例），与 `commands::onboarding` 的
/// `RegisterCompletedPayload` 同一形状。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StarRewardOffer {
    pub promo_code: String,
    pub amount_usd: u64,
}

/// 远端配置里的 star_reward 当前可用吗。空码 = 维护者撤销 = 活动下线，
/// 与 `remote_config::resolve_code` 的「空值 = 撤销」同一语义。
pub(crate) fn effective_star_reward() -> Option<crate::relay::remote_config::StarRewardConfig> {
    crate::relay::remote_config::load_cached()
        .and_then(|config| config.star_reward)
        .filter(|reward| !reward.promo_code.trim().is_empty())
}

/// 邀请成立的唯一判定：远端配置里有可用的 `star_reward`。读不到就 `None` ——
/// 调用方（新人引导事件、红点点击）一律静默回落到现状行为，不给用户看一个
/// 随时兑现不了的 offer。
pub(crate) fn build_offer() -> Option<StarRewardOffer> {
    let reward = effective_star_reward()?;
    Some(StarRewardOffer {
        promo_code: reward.promo_code.trim().to_string(),
        amount_usd: reward.amount_usd,
    })
}

/// 红点入口的弹窗邀请。`None` = 活动不在（远端配置无 `star_reward` / 空码），
/// 前端回落「直接开仓库」。
#[tauri::command]
pub fn star_reward_offer() -> Result<Option<StarRewardOffer>, String> {
    Ok(build_offer())
}

/// Star 领取落点（2026-08-16 起）：后端 RMW 写 `star_reward_claimed`，幂等。
/// 不走前端全量 save —— 这条路在 `merge_settings_for_save` 对后端专有字段
/// 无条件取现有值（旧快照回写曾把它抹掉，红点复活、码可重领），这个字段的
/// 事实 owner 本来就在后端。
#[tauri::command]
pub fn star_reward_mark_claimed() -> Result<(), String> {
    crate::settings::mutate_settings(|settings| {
        settings.star_reward_claimed = Some(true);
    })
    .map_err(|e| e.to_string())
}
