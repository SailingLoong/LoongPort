//! Thin commands; selected materials and authorization remain in one owner.
use crate::{
    store::AppState,
    trae_accounts::{
        entry::{self, Error, FlowView},
        store::{AccountView, Realm},
    },
};
use tauri::State;
#[tauri::command]
pub(crate) async fn list_trae_accounts(
    state: State<'_, AppState>,
) -> Result<Vec<AccountView>, Error> {
    entry::list(state.db.secret_session()).await
}
#[tauri::command]
pub(crate) async fn begin_trae_authorization(
    state: State<'_, AppState>,
    storage_path: String,
    product_path: String,
    realm: Realm,
) -> Result<FlowView, Error> {
    entry::begin(state.db.secret_session(), storage_path, product_path, realm).await
}
#[tauri::command]
pub(crate) async fn confirm_trae_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<AccountView, Error> {
    entry::confirm(state.db.secrets.clone(), &flow_id).await
}
#[tauri::command]
pub(crate) async fn cancel_trae_authorization(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<(), Error> {
    entry::cancel(state.db.secret_session(), &flow_id).await
}
#[tauri::command]
pub(crate) async fn retire_trae_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), Error> {
    entry::retire(state.db.secret_session(), &id).await
}
