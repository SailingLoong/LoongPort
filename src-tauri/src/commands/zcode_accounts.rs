//! No credentials or trusted admission facts cross the renderer boundary.
use crate::store::AppState;
use crate::zcode_accounts::{
    api::{self, LatestVersion, PublicError, SourceContext},
    native_context::{ContextSelection, MetadataDiscovery},
    runtime::{BundleCommitInput, BundlePreview, BundlePreviewInput, CapturePreview, ImportChoice},
    transaction::{CaptureCommitOutcome, CaptureOutcome, CatalogStatus, RecoveryStatus},
};
use tauri::State;
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn open_zcode_for_account_login(
    state: State<'_, AppState>,
    source: ContextSelection,
) -> Result<(), PublicError> {
    api::open_for_login(state.db.clone(), source).await
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn preview_zcode_account_bundle(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
    file: Vec<u8>,
    password: String,
) -> Result<BundlePreview, PublicError> {
    api::preview_bundle(
        state.db.clone(),
        source,
        context_revision,
        BundlePreviewInput {
            revision: catalog_revision,
            file: zeroize::Zeroizing::new(file),
            password: zeroize::Zeroizing::new(password),
        },
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn import_zcode_account_bundle(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
    preview_id: String,
    selected: Vec<ImportChoice>,
) -> Result<Vec<CaptureCommitOutcome>, PublicError> {
    api::commit_bundle(
        state.db.clone(),
        source,
        context_revision,
        BundleCommitInput {
            revision: catalog_revision,
            preview_id,
            selected,
        },
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn cancel_zcode_bundle_preview(
    state: State<'_, AppState>,
    preview_id: String,
) -> Result<(), PublicError> {
    api::cancel_bundle_preview(state.db.clone(), preview_id).await
}

#[tauri::command]
pub(crate) async fn query_zcode_latest_version() -> LatestVersion {
    api::latest_version().await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn discover_zcode_metadata(
    state: State<'_, AppState>,
    install_path: Option<std::path::PathBuf>,
) -> Result<MetadataDiscovery, PublicError> {
    api::discover(state.db.clone(), install_path).await
}
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
pub(crate) async fn preview_zcode_current_account(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
) -> Result<CapturePreview, PublicError> {
    api::preview_capture(state.db.clone(), source, context_revision, catalog_revision).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn save_zcode_account_preview(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
    preview_id: String,
    update_duplicate: bool,
) -> Result<CaptureCommitOutcome, PublicError> {
    api::capture_reviewed(
        state.db.clone(),
        source,
        context_revision,
        catalog_revision,
        preview_id,
        update_duplicate,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn cancel_zcode_account_preview(
    state: State<'_, AppState>,
    preview_id: String,
) -> Result<(), PublicError> {
    api::cancel_capture_preview(state.db.clone(), preview_id).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn switch_zcode_saved_account(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    catalog_revision: String,
    request_id: String,
) -> Result<&'static str, PublicError> {
    api::switch_saved(
        state.db.clone(),
        source,
        context_revision,
        id,
        catalog_revision,
        request_id,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_switch_operation(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
    request_id: String,
) -> Result<Option<crate::zcode_accounts::runtime::OperationStatus>, PublicError> {
    api::operation_status(state.db.clone(), source, context_revision, request_id).await
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
