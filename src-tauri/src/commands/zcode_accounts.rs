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
    data_root: Option<std::path::PathBuf>,
    catalog_revision: String,
    file: Vec<u8>,
    password: String,
) -> Result<BundlePreview, PublicError> {
    api::preview_bundle(
        state.db.clone(),
        data_root,
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
    data_root: Option<std::path::PathBuf>,
    catalog_revision: String,
    preview_id: String,
    selected: Vec<ImportChoice>,
) -> Result<Vec<CaptureCommitOutcome>, PublicError> {
    api::commit_bundle(
        state.db.clone(),
        data_root,
        BundleCommitInput {
            revision: catalog_revision,
            preview_id,
            selected,
        },
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn check_zcode_account_bundle(
    state: State<'_, AppState>,
    preview_id: String,
    selected: Vec<ImportChoice>,
    allow_official_check: bool,
) -> Result<crate::zcode_accounts::import_reviews::CheckProgress, PublicError> {
    api::check_bundle(state.db.clone(), preview_id, selected, allow_official_check).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_bundle_check_progress(
    state: State<'_, AppState>,
    preview_id: String,
) -> Result<crate::zcode_accounts::import_reviews::CheckProgress, PublicError> {
    api::bundle_check_progress(state.db.clone(), preview_id).await
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

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn begin_zcode_official_login(
    state: State<'_, AppState>,
    family: String,
    data_root: Option<std::path::PathBuf>,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::begin(state.db.clone(), &family, data_root).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn begin_saved_zcode_coding(
    state: State<'_, AppState>,
    data_root: Option<std::path::PathBuf>,
    catalog_revision: String,
    id: String,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::begin_saved(
        state.db.clone(),
        data_root,
        catalog_revision,
        id,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_login_progress(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::query(state.db.clone(), flow_id).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn confirm_zcode_login_key(
    state: State<'_, AppState>,
    flow_id: String,
    organization_id: String,
    project_id: String,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::confirm(
        state.db.clone(),
        flow_id,
        organization_id,
        project_id,
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn decline_zcode_login_key(
    state: State<'_, AppState>,
    flow_id: String,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::decline(state.db.clone(), flow_id).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn save_zcode_login_account(
    state: State<'_, AppState>,
    flow_id: String,
    update_duplicate: bool,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::save(state.db.clone(), flow_id, update_duplicate).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn cancel_zcode_official_login(
    flow_id: String,
) -> Result<crate::zcode_accounts::oauth::LoginProgress, PublicError> {
    crate::zcode_accounts::oauth_runtime::cancel(flow_id)
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_account_library(
    state: State<'_, AppState>,
    data_root: Option<std::path::PathBuf>,
) -> Result<CatalogStatus, PublicError> {
    crate::zcode_accounts::oauth_runtime::catalog(state.db.clone(), data_root).await
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_last_login_progress(
    state: State<'_, AppState>,
    data_root: Option<std::path::PathBuf>,
) -> Result<Option<crate::zcode_accounts::oauth::LoginProgress>, PublicError> {
    crate::zcode_accounts::oauth_runtime::last_progress(state.db.clone(), data_root).await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn set_zcode_account_label(
    state: State<'_, AppState>,
    data_root: Option<std::path::PathBuf>,
    catalog_revision: String,
    id: String,
    label: Option<String>,
) -> Result<CatalogStatus, PublicError> {
    crate::zcode_accounts::oauth_runtime::set_label(
        state.db.clone(),
        data_root,
        catalog_revision,
        id,
        label,
    )
    .await
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BundleExportInput {
    request_id: String,
    data_root: Option<std::path::PathBuf>,
    catalog_revision: String,
    profile_ids: Vec<String>,
    destination: std::path::PathBuf,
    password: String,
    password_confirmation: String,
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn export_zcode_account_bundle(
    state: State<'_, AppState>,
    input: BundleExportInput,
) -> Result<crate::zcode_accounts::bundle_export::ExportResult, PublicError> {
    crate::zcode_accounts::oauth_runtime::export_bundle(
        state.db.clone(),
        input.data_root,
        crate::zcode_accounts::bundle_export::ExportRequest {
            request_id: input.request_id,
            catalog_revision: input.catalog_revision,
            profile_ids: input.profile_ids,
            destination: input.destination,
            password: zeroize::Zeroizing::new(input.password),
            password_confirmation: zeroize::Zeroizing::new(input.password_confirmation),
        },
    )
    .await
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn get_zcode_bundle_export_result(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<crate::zcode_accounts::bundle_export::ExportResult, PublicError> {
    crate::zcode_accounts::oauth_runtime::export_result(state.db.clone(), request_id).await
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn read_zcode_current_identity(
    state: State<'_, AppState>,
    source: ContextSelection,
    context_revision: String,
) -> Result<crate::zcode_accounts::runtime::CurrentIdentity, PublicError> {
    api::read_current_identity(state.db.clone(), source, context_revision).await
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn check_zcode_account_connections(
    state: State<'_, AppState>,
    request_id: String,
    data_root: Option<std::path::PathBuf>,
    catalog_revision: String,
    id: String,
    allow_official_check: bool,
) -> Result<CatalogStatus, PublicError> {
    api::check_saved_connections(
        state.db.clone(),
        request_id,
        data_root,
        catalog_revision,
        id,
        allow_official_check,
    )
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn cancel_zcode_connection_check(
    request_id: String,
) -> Result<&'static str, PublicError> {
    api::cancel_connection_check(&request_id)
}
