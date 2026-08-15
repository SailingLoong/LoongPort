//! 新人引导命令层：薄调度，策略事实都在 [`crate::relay::onboarding`]。
//!
//! 见那个模块的文档 for 模块边界（策略收拢、机制复用、后续调整只动那边）。

use serde::Serialize;
use tauri::{Emitter, State};

use crate::events::ONBOARDING_REGISTER_COMPLETED;
use crate::relay::onboarding;
use crate::store::AppState;

use super::relay::{import_site, user_has_no_accounts, BrowserEntrySource, ImportResult};

/// 新人引导注册窗完成事件的 payload（前端 `src/lib/onboarding.ts` 消费）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegisterCompletedPayload {
    relay_id: i64,
    site_name: String,
}

/// 弹不弹新人引导注册窗的判据。**纯判据，无副作用**：
/// 还没有任何账号（新用户）&& 这个安装还没弹过（一次性标志未置位）。
fn register_prompt_eligible(state: &AppState) -> Result<bool, crate::error::AppError> {
    Ok(crate::settings::get_settings()
        .onboarding_register_prompted
        .is_none()
        && user_has_no_accounts(state)?)
}

/// 标志只置位一次：置位后无论窗口结局如何（注册成功 / 关窗 / 超时 / 没网），
/// 后续启动都不再自动弹。这是有意的不重试 —— 弹过又没成的用户回落到既有的
/// 「添加站点」首启提示，别用同一个窗口反复打扰。
fn mark_register_prompted() {
    let mut settings = crate::settings::get_settings();
    if settings.onboarding_register_prompted.is_none() {
        settings.onboarding_register_prompted = Some(true);
        if let Err(error) = crate::settings::update_settings(settings) {
            log::warn!("新人引导标志写入失败（不影响本次引导，但下次启动会再弹）: {error}");
        }
    }
}

/// 新用户首启时自动打开官方站（BestAPI）注册窗。
///
/// 满足判据就**标记后立即返回 `true`**，窗口生命周期在后台跑 —— 命令不能等
/// `import_site`：它要到用户注册完 / 关窗 / 超时才返回，而前端要用返回值决定
/// 同一会话里还弹不弹「添加站点」首启提示（两个提示只留一个）。
///
/// 注册成功后发 [`ONBOARDING_REGISTER_COMPLETED`]（payload 见该常量的文档），
/// 前端拿它做 toast + 档位预配 + 列表刷新。失败只记日志不上报：这里的失败大多
/// 是正常结局（用户关窗 / 没网），为它们弹错误 toast 是把噪声当反馈。
#[tauri::command]
pub async fn onboarding_prompt_register(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let eligible = register_prompt_eligible(&state).map_err(|e| e.to_string())?;
    if !eligible {
        return Ok(false);
    }
    mark_register_prompted();

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        match import_site(
            &handle,
            onboarding::OFFICIAL_SITE_ORIGIN,
            BrowserEntrySource::Onboarding,
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

    Ok(true)
}
