//! Thin commands: only explicit user actions may authorize or opt into automatic claims.
use crate::{
    store::AppState,
    workbuddy::{
        authorization,
        clock::SystemClock,
        engine::Engine,
        http::OfficialHttp,
        model::{AccountView, ClaimResult, Failure},
    },
};
use tauri::State;
#[tauri::command]
pub(crate) async fn set_workbuddy_automatic_claim(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
    revision: u64,
    policy_version: u32,
    enabled: bool,
) -> Result<AccountView, Failure> {
    Engine {
        session: state.db.secret_session(),
        transport: &OfficialHttp,
        clock: &SystemClock,
    }
    .set_automatic(&id, generation, revision, policy_version, enabled)
    .await
}
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
pub(crate) async fn claim_all_workbuddy_today(
    state: State<'_, AppState>,
) -> Result<Vec<ClaimResult>, Failure> {
    let session = state.db.secret_session();
    Engine {
        session,
        transport: &OfficialHttp,
        clock: &SystemClock,
    }
    .claim_all()
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

#[tauri::command]
pub(crate) async fn list_workbuddy_inference_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<crate::workbuddy::inference::InferenceAccount>, Failure> {
    crate::workbuddy::inference_commands::accounts(state.db.secret_session()).await
}
#[tauri::command]
pub(crate) async fn begin_workbuddy_inference_authorization(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::workbuddy::inference_commands::Login, Failure> {
    crate::workbuddy::inference_commands::begin(state.db.secrets.clone(), id).await
}
#[tauri::command]
pub(crate) async fn poll_workbuddy_inference_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<crate::workbuddy::inference_commands::PollResult, Failure> {
    crate::workbuddy::inference_commands::poll(state.db.secrets.clone(), &flow_id).await
}
#[tauri::command]
pub(crate) async fn cancel_workbuddy_inference_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<(), Failure> {
    crate::workbuddy::inference_commands::cancel(state.db.secrets.clone(), &flow_id).await
}
#[tauri::command]
pub(crate) async fn discover_workbuddy_inference_models(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<crate::workbuddy::inference::Model>, Failure> {
    crate::workbuddy::inference_commands::models(state.db.secrets.clone(), &id).await
}
