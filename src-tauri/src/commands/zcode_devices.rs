//! Local device selection is available only to the main account-management UI.
use crate::{zcode_accounts::device_settings, AppState};
use std::path::PathBuf;
use tauri::{State, WebviewWindow};
fn main_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("blocked")
    }
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_device_state(
    window: WebviewWindow,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
) -> Result<device_settings::View, &'static str> {
    main_window(&window)?;
    device_settings::state(state.db.clone(), data_root).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn import_zcode_device_ids(
    window: WebviewWindow,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
    input: String,
    revision: u64,
) -> Result<device_settings::View, &'static str> {
    main_window(&window)?;
    device_settings::import(state.db.clone(), data_root, input, revision).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn select_zcode_device_id(
    window: WebviewWindow,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
    device_mid: Option<String>,
    revision: u64,
) -> Result<device_settings::View, &'static str> {
    main_window(&window)?;
    device_settings::select(state.db.clone(), data_root, device_mid, revision).await
}
