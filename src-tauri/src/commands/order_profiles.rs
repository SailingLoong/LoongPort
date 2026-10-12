//! Desktop commands for named order snapshots; persistence belongs to the service.
use crate::{services::order_profiles as profiles, store::AppState};
pub use profiles::{OrderProfile, OrderProfilesState};
use std::path::PathBuf;
use tauri::State;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub fn get_order_profiles(
    state: State<'_, AppState>,
    app_type: String,
) -> Result<OrderProfilesState, String> {
    profiles::get(&state.db, &app_type).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    name: String,
    provider_ids: Vec<String>,
) -> Result<(), String> {
    crate::services::application_selection::save_order_profile(
        &state,
        &app_type
            .parse()
            .map_err(|error: crate::error::AppError| error.to_string())?,
        &name,
        &provider_ids,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    from: String,
    to: String,
) -> Result<(), String> {
    crate::services::application_selection::rename_order_profile(
        &state,
        &app_type
            .parse()
            .map_err(|error: crate::error::AppError| error.to_string())?,
        &from,
        &to,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_order_profile(
    state: State<'_, AppState>,
    app_type: String,
    name: String,
) -> Result<(), String> {
    crate::services::application_selection::remove_order_profile(
        &state,
        &app_type
            .parse()
            .map_err(|error: crate::error::AppError| error.to_string())?,
        &name,
    )
    .map_err(|e| e.to_string())
}

/// 导出全部配置档为 JSON 文件（自带保存对话框）。返回写入路径；用户取消返回 None。
#[tauri::command]
pub async fn export_order_profiles<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    app_type: String,
) -> Result<Option<String>, String> {
    let json = profiles::export_json(&state.db, &app_type).map_err(|e| e.to_string())?;
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .set_file_name(format!("{app_type}-order-profiles.json"))
        .blocking_save_file()
        .map(|p| p.to_string())
    else {
        return Ok(None);
    };
    std::fs::write(PathBuf::from(&path), json).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

/// 从 JSON 文件导入配置档（自带打开对话框，同名单档覆盖）。
/// 整份验证后一次导入；返回导入条数；取消返回 None。
#[tauri::command]
pub async fn import_order_profiles<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    app_type: String,
) -> Result<Option<usize>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("JSON", &["json"])
        .blocking_pick_file()
        .map(|p| p.to_string())
    else {
        return Ok(None);
    };
    let raw = std::fs::read_to_string(PathBuf::from(&path)).map_err(|e| e.to_string())?;
    let parsed: Vec<OrderProfile> =
        serde_json::from_str(&raw).map_err(|e| format!("invalid profile file: {e}"))?;
    crate::services::application_selection::import_order_profiles(
        &state,
        &app_type
            .parse()
            .map_err(|error: crate::error::AppError| error.to_string())?,
        parsed,
    )
    .map(Some)
    .map_err(|e| e.to_string())
}
