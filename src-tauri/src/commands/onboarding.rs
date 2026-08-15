//! 新人引导命令层：薄调度，策略事实都在 [`crate::relay::onboarding`]。
//!
//! 见那个模块的文档 for 模块边界（策略收拢、机制复用、后续调整只动那边）。
//!
//! 引导形状（2026-08-15 起）：新人首启**不再自动弹官方站注册窗** —— 唯一的
//! 主动触点是「点 Star 领注册礼」弹窗（本模块判资格后发
//! [`ONBOARDING_STAR_REWARD_OFFER`]，前端 `StarRewardDialog` 弹），注册窗退化成
//! star 走通之后的终点（[`onboarding_open_register_window`]）。

use serde::Serialize;
use tauri::{Emitter, State};

use crate::events::{ONBOARDING_REGISTER_COMPLETED, ONBOARDING_STAR_REWARD_OFFER};
use crate::relay::onboarding;
use crate::store::AppState;

use super::relay::{import_site, user_has_no_accounts, BrowserEntrySource, ImportResult};
use super::star_reward;

/// 新人引导注册窗完成事件的 payload（前端 `src/lib/onboarding.ts` 消费）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterCompletedPayload {
    relay_id: i64,
    site_name: String,
}

/// 弹不弹新人引导的判据。**纯判据，无副作用**：
/// 还没有任何账号（新用户）&& 这个安装还没引导过（一次性标志未置位）。
fn register_prompt_eligible(state: &AppState) -> Result<bool, crate::error::AppError> {
    Ok(crate::settings::get_settings()
        .onboarding_register_prompted
        .is_none()
        && user_has_no_accounts(state)?)
}

/// 标志只置位一次：置位后无论弹窗结局如何（领了 / 取消 / 压根没弹成），
/// 后续启动都不再主动弹。这是有意的不重试 —— 引导过的用户回落到广场页 +
/// 顶栏红点（未领取时红点常亮，那是他们的「稍后再说」入口）。
fn mark_register_prompted() {
    let mut settings = crate::settings::get_settings();
    if settings.onboarding_register_prompted.is_none() {
        settings.onboarding_register_prompted = Some(true);
        if let Err(error) = crate::settings::update_settings(settings) {
            log::warn!("新人引导标志写入失败（不影响本次引导，但下次启动会再弹）: {error}");
        }
    }
}

/// 新用户首启的「点 Star 领注册礼」邀请。
///
/// 三道闸都在这里收口：资格（无账号 + 未引导过）→ 远端配置有 `star_reward`
/// → 基线星数取得到（star 邀约的基本盘 —— 取不到连比对都做不了，别让用户
/// 看到一个随时兑现不了的 offer）。全过才发 [`ONBOARDING_STAR_REWARD_OFFER`]
/// （payload 含基线），前端拿到即弹；任何一道不过都静默返回，新人引导宁可
/// 少弹一次。
///
/// 返回 `()`：弹不弹的判定完全在 Rust，前端不需要返回值分支（广场页总是要
/// 落的，见 `RelaySection.reloadStatus`）。
#[tauri::command]
pub async fn onboarding_prompt_star_reward(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let eligible = register_prompt_eligible(&state).map_err(|e| e.to_string())?;
    if !eligible {
        return Ok(());
    }
    mark_register_prompted();

    let Some(offer) = star_reward::build_offer().await else {
        return Ok(());
    };
    if let Err(error) = app_handle.emit(ONBOARDING_STAR_REWARD_OFFER, offer) {
        // 发不出去只是这次不弹（标志已置位，下次启动不再问）—— 命令本身
        // 不该因此失败，那会让前端把一次正常的引导当成后端故障。
        log::warn!("发射 star 邀请事件失败: {error}");
    }
    Ok(())
}

/// 打开官方站（BestAPI）注册窗 —— 只有「点 Star 领注册礼」走通之后才被调用，
/// 所以优惠码必给且显式传入。
///
/// 码走**显式参数**而不是塞进 `promo_codes` 码表：那张表是给所有导入无条件
/// 预填的，而这份码要 gate 在 star 后面 —— 两个 owner、两份数据，别合。
///
/// 窗口生命周期在后台跑（命令不能等 `import_site`：它要到用户注册完 / 关窗 /
/// 超时才返回）。注册成功仍发 [`ONBOARDING_REGISTER_COMPLETED`]，`RelaySection`
/// 的 toast + 档位预配 + 列表刷新原样保留。
#[tauri::command]
pub async fn onboarding_open_register_window(
    app_handle: tauri::AppHandle,
    promo_code: String,
) -> Result<(), String> {
    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        match import_site(
            &handle,
            onboarding::OFFICIAL_SITE_ORIGIN,
            BrowserEntrySource::Onboarding,
            Some(&promo_code),
        )
        .await
        {
            Ok(result) => {
                let ImportResult {
                    relay_id,
                    site_name,
                    ..
                } = result;
                let _ = handle.emit(
                    ONBOARDING_REGISTER_COMPLETED,
                    RegisterCompletedPayload {
                        relay_id,
                        site_name,
                    },
                );
            }
            Err(error) => {
                // 关窗 / 超时走这里（RelayImportError::Incomplete）—— 正常结局，
                // 不打扰用户。真异常（协议冲突等）也只进日志：窗口本身已经把
                // 用户可见的失败呈现过了。
                log::info!("新人引导注册窗未完成：{:?}", error.kind);
            }
        }
    });

    Ok(())
}
