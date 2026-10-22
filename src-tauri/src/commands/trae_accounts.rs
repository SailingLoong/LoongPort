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

/// Explicit per-account action; no credentials, host, or device fields from IPC.
#[tauri::command]
pub(crate) async fn checkin_trae_account(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::trae_accounts::checkin::Outcome, String> {
    crate::trae_accounts::checkin::run(
        state.db.secrets.clone(),
        &id,
        std::time::Duration::from_secs(30),
    )
    .await
    .map_err(|_| "trae.checkin_unavailable".to_string())
}

/// Exposes paths and public product metadata only. No credential file is opened.
#[tauri::command]
pub(crate) async fn discover_trae_installations(
    realm: Realm,
) -> Result<Vec<crate::trae_accounts::discovery::Installation>, Error> {
    tauri::async_runtime::spawn_blocking(move || crate::trae_accounts::discovery::discover(realm))
        .await
        .map_err(|_| Error::StorageUnavailable)
}

/// Per-account consent only. Never accepts credentials, a host, or a device.
#[tauri::command]
pub(crate) async fn set_trae_automatic_checkin(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
    policy_revision: u64,
    enabled: bool,
) -> Result<AccountView, String> {
    crate::trae_accounts::checkin::set_automatic(
        state.db.secret_session(),
        &id,
        generation,
        policy_revision,
        enabled,
    )
    .await
    .map_err(|_| "trae.checkin_unavailable".to_string())
}
