//! Safe explicit catalog commands; shared invoke registration is integrator-owned.
use crate::{
    store::AppState,
    trae_accounts::models::{self, Error, ModelView, RequestView},
};
use tauri::State;
#[tauri::command]
pub(crate) async fn begin_trae_model_discovery(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<RequestView, Error> {
    models::begin(state.db.secrets.clone(), &account_id).await
}
#[tauri::command]
pub(crate) async fn list_trae_discovered_models(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<Vec<ModelView>, Error> {
    models::list(state.db.secrets.clone(), &request_id).await
}
#[tauri::command]
pub(crate) async fn cancel_trae_model_discovery(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), Error> {
    models::cancel(state.db.secret_session(), &request_id).await
}
