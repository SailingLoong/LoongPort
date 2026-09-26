//! Compatibility command entry points; selection belongs to the application service.
use super::*;
pub(crate) use crate::services::application_selection::{
    switch_tier_command, switch_tier_model_command,
};
pub use crate::services::application_selection::{SwitchTierCommandResult, SwitchTierResult};

/// 切换档位：退 ChatGPT → 切换 → 重开。
///
/// `quit_chatgpt` 由前端在用户确认弹窗后传 true。传 false 则只切换（用户自己管重启）。
///
/// `app` 是**必需参数**，不能从 `provider_id` 反推（spec §三）：
/// `provider_id_for(site_origin, group_id)` 不含 platform，而四段 Key 契约恰恰写明
/// 「分组 id 只在平台内唯一，跨平台会撞号」—— 所以同一个 `loongport-<hash>` 可以合法地
/// 存在于两个 app_type 行下（`providers` 主键是 `(id, app_type)`），哈希单向反解不出来。
/// 调用方（前端）本来就知道当前是哪个 tab。
#[tauri::command]
pub async fn relay_switch_tier(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchTierCommandResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_command(&app_handle, &provider_id, app_type, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}

/// Select a supported model from a managed Codex tier and activate that tier.
///
/// The model catalog stored with the provider is the authority for validation;
/// this keeps a stale frontend from writing an arbitrary model into
/// `config.toml`. Updating the provider before the normal switch flow also
/// means ChatGPT is restarted with the selected model already in place.
#[tauri::command]
pub async fn relay_switch_tier_model(
    app_handle: tauri::AppHandle,
    provider_id: String,
    app: String,
    model: String,
    quit_chatgpt: Option<bool>,
) -> Result<SwitchTierCommandResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    switch_tier_model_command(&app_handle, &provider_id, app_type, &model, quit_chatgpt)
        .await
        .map_err(|e| e.to_string())
}
