//! Thin MiniMax Code commands; credentials and claim admission remain in one owner.
use crate::{
    minimax_code::{
        auth,
        catalog::{AccountView, Failure, Region},
        checkin::Engine,
        http::OfficialHttp,
    },
    store::AppState,
};
use tauri::State;

#[tauri::command]
pub(crate) async fn list_minimax_code_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<AccountView>, Failure> {
    Engine {
        session: state.db.secret_session(),
        transport: &OfficialHttp,
    }
    .list()
    .await
}

#[tauri::command]
pub(crate) async fn refresh_minimax_code_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountView, Failure> {
    Engine {
        session: state.db.secret_session(),
        transport: &OfficialHttp,
    }
    .refresh(&id)
    .await
}

#[tauri::command]
pub(crate) async fn claim_minimax_code_today(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountView, Failure> {
    Engine {
        session: state.db.secret_session(),
        transport: &OfficialHttp,
    }
    .claim(&id)
    .await
}

#[tauri::command]
pub(crate) async fn recheck_minimax_code_claim(
    state: State<'_, AppState>,
    id: String,
) -> Result<AccountView, Failure> {
    Engine {
        session: state.db.secret_session(),
        transport: &OfficialHttp,
    }
    .recheck(&id)
    .await
}

#[tauri::command]
pub(crate) async fn retire_minimax_code_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), Failure> {
    Engine {
        session: state.db.secret_session(),
        transport: &OfficialHttp,
    }
    .retire(&id)
    .await
}

#[tauri::command]
pub(crate) async fn begin_minimax_code_authorization(
    state: State<'_, AppState>,
    region: Region,
) -> Result<auth::Login, Failure> {
    auth::begin(state.db.secret_session(), &OfficialHttp, region).await
}
#[tauri::command]
pub(crate) async fn poll_minimax_code_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<auth::AuthorizationResult, Failure> {
    auth::poll(state.db.secret_session(), &OfficialHttp, &flow_id).await
}
#[tauri::command]
pub(crate) async fn cancel_minimax_code_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<(), Failure> {
    auth::cancel(state.db.secret_session(), &flow_id).await
}
