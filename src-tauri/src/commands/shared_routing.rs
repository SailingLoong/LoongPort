//! Thin IPC over the native shared owner and original application journal.
use crate::{
    app_config::AppType,
    mode::state::SaveRequest,
    services::{
        routing_groups::{Group, RouteTarget},
        shared_routing as owner,
    },
    store::AppState,
};
use std::str::FromStr;
#[tauri::command]
pub fn get_shared_routing_state(
    state: tauri::State<'_, AppState>,
    app_type: String,
) -> Result<owner::SharedRoutingState, String> {
    owner::read_state(
        &state,
        &AppType::from_str(&app_type).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub fn prepare_shared_routing_group(
    state: tauri::State<'_, AppState>,
    draft: owner::GroupDraft,
) -> Result<owner::PreparedGroup, String> {
    owner::prepare_group(&state, draft).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn confirm_shared_routing_group(
    state: tauri::State<'_, AppState>,
    prepared: owner::PreparedGroup,
) -> Result<Group, String> {
    owner::confirm_group(&state, prepared).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn prepare_application_route_target(
    state: tauri::State<'_, AppState>,
    app_type: String,
    target: RouteTarget,
) -> Result<owner::PreparedRouteTarget, String> {
    owner::prepare_target(
        &state,
        &AppType::from_str(&app_type).map_err(|e| e.to_string())?,
        target,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn apply_application_route_target(
    state: tauri::State<'_, AppState>,
    app_type: String,
    prepared: owner::PreparedRouteTarget,
) -> Result<owner::RouteRequestResult, String> {
    let app = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || owner::apply_target(&state, &app, prepared))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_application_route_target_result(
    state: tauri::State<'_, AppState>,
    app_type: String,
    request: SaveRequest,
) -> Result<owner::RouteRequestResult, String> {
    owner::query_target(
        &state,
        &AppType::from_str(&app_type).map_err(|e| e.to_string())?,
        request,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn recover_application_route_target(
    state: tauri::State<'_, AppState>,
    app_type: String,
    request: SaveRequest,
) -> Result<owner::RouteRequestResult, String> {
    let app = AppType::from_str(&app_type).map_err(|e| e.to_string())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || owner::recover_target(&state, &app, request))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
