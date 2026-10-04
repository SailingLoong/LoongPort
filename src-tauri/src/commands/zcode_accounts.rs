//! No credentials or trusted admission facts cross the renderer boundary.
use crate::store::AppState;
use crate::zcode_accounts::{
    api::{self, PublicError, SourceContext},
    native_context::ContextSelection,
    transaction::{CaptureOutcome, CatalogStatus, RecoveryStatus},
};
use tauri::State;

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn inspect_zcode_account_context(
    state: State<'_, AppState>,
    source: ContextSelection,
) -> Result<SourceContext, PublicError> {
    api::inspect(state.db.clone(), source).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_account_status(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
) -> Result<CatalogStatus, PublicError> {
    api::status(state.db.clone(), source, context_revision).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn capture_zcode_current_account(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
) -> Result<CaptureOutcome, PublicError> {
    api::capture(state.db.clone(), source, context_revision, catalog_revision).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn switch_zcode_saved_account(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    catalog_revision: String,
) -> Result<&'static str, PublicError> {
    api::switch_saved(
        state.db.clone(),
        source,
        context_revision,
        id,
        catalog_revision,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_account_recovery(
    state: State<'_, AppState>,
) -> Result<RecoveryStatus, PublicError> {
    api::recovery_status(state.db.clone()).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn archive_zcode_account_recovery(
    state: State<'_, AppState>,
    revision: String,
) -> Result<&'static str, PublicError> {
    api::archive(state.db.clone(), revision).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn confirm_zcode_account_recovery(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    recovery_revision: String,
) -> Result<(), PublicError> {
    api::confirm(
        state.db.clone(),
        source,
        context_revision,
        id,
        recovery_revision,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn recapture_zcode_account_recovery(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    recovery_revision: String,
    catalog_revision: String,
) -> Result<CaptureOutcome, PublicError> {
    api::recapture(
        state.db.clone(),
        source,
        context_revision,
        id,
        recovery_revision,
        catalog_revision,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn delete_zcode_account_recovery(
    state: State<'_, AppState>,
    id: String,
    revision: String,
) -> Result<(), PublicError> {
    api::delete(state.db.clone(), id, revision).await
}
