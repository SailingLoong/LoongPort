//! 新人引导命令层：薄调度，策略事实都在 [`crate::relay::onboarding`]。
//!
//! 见那个模块的文档 for 模块边界（策略收拢、机制复用、后续调整只动那边）。
//!
//! 引导形状（2026-09-06 起）：新人首启只**落到中转站广场**（`RelaySection`
//! 的 `shouldPromptAddSite` 跳转），全程不弹任何邀约。「点 Star 领注册礼」
//! 的入口只剩顶栏 GitHub 按钮的红点（常亮到领取为止，用户自己点）——
//! 曾经挂在 `import_site` 成功路径上的主动弹窗已删：刚接入站点时用户
//! 还没有任何使用感，此时弹点赞礼只会被打断，实测用户不愿意点。
//! 注册窗（[`onboarding_open_register_window`]）仍是 Star 对话框领取后
//! 打开的终点。

use serde::Serialize;
use tauri::Emitter;

use crate::events::ONBOARDING_REGISTER_COMPLETED;
use crate::relay::onboarding;

use super::relay::{import_site, BrowserEntrySource, ImportResult};

/// 新人引导注册窗完成事件的 payload（前端 `src/lib/onboarding.ts` 消费）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterCompletedPayload {
    relay_id: i64,
    site_name: String,
}

/// 打开官方站（BestAPI）注册窗 —— Star 对话框「领取」点击后调用，
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
