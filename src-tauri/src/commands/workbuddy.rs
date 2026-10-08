//! Thin commands: only explicit user actions may authorize or claim.
use crate::{
    store::AppState,
    workbuddy::{
        authorization,
        clock::SystemClock,
        engine::Engine,
        http::OfficialHttp,
        model::{AccountView, Failure},
    },
};
use tauri::State;
#[tauri::command]
pub(crate) async fn list_workbuddy_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<AccountView>, Failure> {
    let session = state.db.secret_session();
    Engine {
        session,
        transport: &OfficialHttp,
        clock: &SystemClock,
    }
    .list()
    .await
}
#[tauri::command]
pub(crate) async fn refresh_workbuddy_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountView, Failure> {
    let session = state.db.secret_session();
    Engine {
        session,
        transport: &OfficialHttp,
        clock: &SystemClock,
    }
    .refresh(&id)
    .await
}
#[tauri::command]
pub(crate) async fn refresh_all_workbuddy_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<AccountView>, Failure> {
    let session = state.db.secret_session();
    Engine {
        session,
        transport: &OfficialHttp,
        clock: &SystemClock,
    }
    .refresh_all()
    .await
}
#[tauri::command]
pub(crate) async fn claim_workbuddy_today(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountView, Failure> {
    let session = state.db.secret_session();
    Engine {
        session,
        transport: &OfficialHttp,
        clock: &SystemClock,
    }
    .claim(&id)
    .await
}
#[tauri::command]
pub(crate) async fn begin_workbuddy_authorization(
    state: State<'_, AppState>,
) -> Result<authorization::Login, Failure> {
    let session = state.db.secret_session();
    authorization::begin(
        session,
        &OfficialHttp,
        chrono::Utc::now().timestamp_millis(),
    )
    .await
}
#[tauri::command]
pub(crate) async fn finish_workbuddy_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<authorization::AuthorizationResult, Failure> {
    let session = state.db.secret_session();
    authorization::finish(
        session,
        &OfficialHttp,
        &flow_id,
        chrono::Utc::now().timestamp_millis(),
    )
    .await
}
