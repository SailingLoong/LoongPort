//! 新人引导命令层：薄调度，策略事实都在 [`crate::relay::onboarding`]。
//!
//! 见那个模块的文档 for 模块边界（策略收拢、机制复用、后续调整只动那边）。
//!
//! 引导形状（2026-08-17 起）：新人首启只**落到中转站广场**（`RelaySection`
//! 的 `shouldPromptAddSite` 跳转），不弹任何邀约 —— 「点 Star 领注册礼」
//! 推迟到**首个站点注册/登录成功之后**（[`offer_star_reward_after_first_import`]，
//! 挂在 `import_site` 成功路径上）：用户有了使用感觉再邀请，比首启硬弹
//! 打扰小。注册窗（[`onboarding_open_register_window`]）仍是 star 走通
//! 之后的终点。

use serde::Serialize;
use tauri::Emitter;

use crate::events::{ONBOARDING_REGISTER_COMPLETED, ONBOARDING_STAR_REWARD_OFFER};
use crate::relay::onboarding;

use super::relay::{import_site, BrowserEntrySource, ImportResult};
use super::star_reward;

/// 新人引导注册窗完成事件的 payload（前端 `src/lib/onboarding.ts` 消费）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterCompletedPayload {
    relay_id: i64,
    site_name: String,
}

/// 首个站点接入成功后的「点 Star 领注册礼」邀请（不暴露成命令：只有
/// `import_site` 的成功路径这一个调用方）。
///
/// 判据：礼还没领（`star_reward_claimed` 为空）&& 这次安装还没主动弹过
/// （`star_reward_offered` 为空）。注意**不看账号数** —— 调用时机本身就是
/// 「刚接入第一个站点」，比旧版首启判据（无账号）语义更直白。
///
/// 另一道闸（远端配置有 `star_reward`）沿用 [`star_reward::build_offer`]
/// —— 纯本地读缓存，不打网络。不过就静默返回；「压根没拿到 offer」不置位
/// 一次性标志（一次时序不巧不该把邀请永久吃掉），下次接入站点再试。
pub(crate) async fn offer_star_reward_after_first_import(app_handle: &tauri::AppHandle) {
    let settings = crate::settings::get_settings();
    if settings.star_reward_claimed.is_some() || settings.star_reward_offered.is_some() {
        return;
    }

    // 接入通常发生在启动数分钟后、后台目录任务早已把缓存落盘；补这一拉
    // 只兜「启动 5 秒内就完成接入」的极端窗口。幂等：与后台任务写同一份缓存。
    if star_reward::effective_star_reward().is_none() {
        crate::relay::remote_config::refresh_and_cache().await;
    }

    let Some(offer) = star_reward::build_offer() else {
        log::info!("Star 邀约未发出（远端配置无 star_reward），下次接入站点再试");
        return;
    };
    mark_star_reward_offered();
    if let Err(error) = app_handle.emit(ONBOARDING_STAR_REWARD_OFFER, offer) {
        // 发不出去只是这次不弹（标志已置位，不再主动弹）—— 红点仍是
        // 未领取用户的常驻「稍后再说」入口。
        log::warn!("发射 star 邀请事件失败: {error}");
    }
}

/// 标志只置位一次，且**只在确认拿到 offer、正要发事件时置位**：置位后无论
/// 弹出去之后的结局如何（领了 / 取消 / 事件发不出去），都不再主动弹 ——
/// 未领取的用户回落到顶栏红点（常亮，那是他们的「稍后再说」入口）。
fn mark_star_reward_offered() {
    crate::settings::mutate_settings(|settings| {
        if settings.star_reward_offered.is_none() {
            settings.star_reward_offered = Some(true);
        }
    })
    .unwrap_or_else(|error| {
        log::warn!("Star 邀约标志写入失败（不影响本次弹窗，但之后可能再弹一次）: {error}")
    });
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
