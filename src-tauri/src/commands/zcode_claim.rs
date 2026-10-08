//! ZCode claims consume saved vault sessions without switching native login.
use crate::{zcode_accounts::claim_runtime, AppState};
use std::path::PathBuf;
use tauri::{AppHandle, State, WebviewWindow};
fn main_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("blocked")
    }
}
fn captcha_window(window: &WebviewWindow) -> Result<(), &'static str> {
    if window.label() == "zcode-claim-captcha" {
        Ok(())
    } else {
        Err("blocked")
    }
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_claim_state(
    window: WebviewWindow,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
) -> Result<claim_runtime::View, &'static str> {
    main_window(&window)?;
    claim_runtime::state(state.db.clone(), data_root).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn set_zcode_claim_auto(
    window: WebviewWindow,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
    enabled: bool,
    participants: Vec<String>,
) -> Result<claim_runtime::View, &'static str> {
    main_window(&window)?;
    claim_runtime::set_auto(state.db.clone(), data_root, enabled, participants).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn start_zcode_claim(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
    ids: Vec<String>,
    preview_only: bool,
) -> Result<claim_runtime::View, &'static str> {
    main_window(&window)?;
    claim_runtime::start(app, state.db.clone(), data_root, ids, preview_only).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn cancel_zcode_claim(
    window: WebviewWindow,
    state: State<'_, AppState>,
    data_root: Option<PathBuf>,
) -> Result<claim_runtime::View, &'static str> {
    main_window(&window)?;
    claim_runtime::cancel(state.db.clone(), data_root).await
}
#[tauri::command]
pub(crate) fn get_zcode_claim_captcha(
    window: WebviewWindow,
) -> Result<claim_runtime::CaptchaView, &'static str> {
    captcha_window(&window)?;
    claim_runtime::captcha_context()
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn submit_zcode_claim_captcha(
    window: WebviewWindow,
    nonce: String,
    param: String,
) -> Result<(), &'static str> {
    captcha_window(&window)?;
    claim_runtime::submit(nonce, param)
}
#[tauri::command]
pub(crate) fn show_zcode_claim_captcha(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<(), &'static str> {
    captcha_window(&window)?;
    claim_runtime::interactive(&app)
}
