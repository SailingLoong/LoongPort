//! Vendor connection receipt over the original application writer.
use crate::{
    app_config::AppType,
    database::vendor_connection_operations as ledger,
    error::AppError,
    services::{application_selection as routing, ProviderService},
    store::AppState,
    vendor::{creds, provision, Vendor},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn err(code: &str) -> AppError {
    AppError::Config(format!("vendorConnection.{code}"))
}
pub(super) fn safe_error(error: &AppError) -> String {
    let code = match error {
        AppError::Config(code) | AppError::Conflict(code) => code.as_str(),
        _ => "",
    };
    match code {
        "vendorConnection.invalidRequest"
        | "vendorConnection.invalidApp"
        | "vendorConnection.invalidPlan"
        | "vendorConnection.invalidReceipt"
        | "vendorConnection.invalidQuery"
        | "vendorConnection.prepareRequired"
        | "vendorConnection.operationScopeChanged"
        | "vendorConnection.accountChanged"
        | "vendorConnection.providerChanged"
        | "vendorConnection.storageChanged"
        | "vendorConnection.storageUnavailable"
        | "vendorConnection.applicationChanged"
        | "vendorConnection.receiptChanged"
        | "vendorConnection.missingOperation"
        | "vendorConnection.outcomeUnknown"
        | "vendorConnection.operationRequiresReconciliation"
        | "vendorConnection.ambiguousOperation"
        | "vendorConnection.invalidTransition" => code.to_owned(),
        _ => "vendorConnection.operationFailed".to_owned(),
    }
}
fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn account_digest(vendor: &str, account: &str) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(&(vendor, account)).map_err(|_| err("invalidAccount"))?;
    Ok(crate::live::engine::sha256_hex(&bytes))
}
fn change(
    id: &str,
    decision: Option<crate::services::provider::codex_direct::CatalogDecision>,
) -> routing::ApplicationRoutingChange {
    routing::ApplicationRoutingChange {
        order: None,
        selection: Some(routing::TierSelection {
            provider_id: id.into(),
            model: None,
        }),
        catalog_decision: decision,
        catalog_preview_only: false,
    }
}
fn check_authority(
    conn: &rusqlite::Connection,
    vault: &crate::secrets::storage::StorageContext,
    receipt: &ledger::Receipt,
) -> Result<(), AppError> {
    let row = creds::get(conn, vault, receipt.row_id)?.ok_or_else(|| err("accountChanged"))?;
    let vendor = Vendor::from_id(&row.vendor_id).ok_or_else(|| err("accountChanged"))?;
    let account = row
        .account_id
        .as_deref()
        .ok_or_else(|| err("accountChanged"))?;
    let app = receipt.app.parse::<AppType>()?;
    if row.vendor_id != receipt.vendor_id
        || account_digest(&row.vendor_id, account)? != receipt.account_identity
        || !provision::VENDOR_APPS.contains(&app)
        || !crate::vendor::plans_for_account(vendor, Some(account))
            .iter()
            .any(|plan| plan.id_segment == receipt.plan)
        || crate::vendor::config_for(vendor, &receipt.plan, &app).is_none()
        || provision::provider_id_for(&receipt.plan, account) != receipt.provider_id
    {
        return Err(err("accountChanged"));
    }
    let revision: String = conn.query_row(
        "SELECT api_key_revision FROM loongport_vendor WHERE id=?1",
        [receipt.row_id],
        |r| r.get(0),
    )?;
    if revision != receipt.credential_revision
        || !crate::database::credential_revision::valid(&revision)
    {
        return Err(err("accountChanged"));
    }
    let provider = crate::Database::get_provider_by_id_on_connection(
        conn,
        vault,
        &receipt.provider_id,
        &receipt.app,
    )?
    .ok_or_else(|| err("providerChanged"))?;
    if crate::Database::provider_update_digest(&provider)? != receipt.provider_revision {
        return Err(err("providerChanged"));
    }
    Ok(())
}
fn with_bound<T>(
    state: &AppState,
    binding: &str,
    f: impl FnOnce(
        &mut rusqlite::Connection,
        &crate::secrets::storage::StorageContext,
    ) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let vault = state.db.secrets.read()?;
    if vault.identity().store_id() != binding {
        return Err(err("storageChanged"));
    }
    let mut conn = state
        .db
        .conn
        .lock()
        .map_err(|_| err("storageUnavailable"))?;
    crate::database::storage::check_identity(&conn, &vault)?;
    f(&mut conn, &vault)
}
struct CaptureTarget<'a> {
    row_id: i64,
    plan: &'a str,
    app: &'a AppType,
    provider_id: &'a str,
}

fn capture_locked(
    state: &AppState,
    target: CaptureTarget<'_>,
    request: Option<routing::PreparedRoutingRequest>,
    catalog_decision: Option<crate::services::provider::codex_direct::CatalogDecision>,
    id: &str,
) -> Result<(ledger::Receipt, u64), AppError> {
    let CaptureTarget {
        row_id,
        plan,
        app,
        provider_id,
    } = target;
    let configuration_revision =
        crate::services::provider::service_configuration_revision(state, app)?;
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    let epoch = state.db.secrets.lifecycle_epoch();
    let receipt = with_bound(state, &binding, |conn, vault| {
        let row = creds::get(conn, vault, row_id)?.ok_or_else(|| err("accountChanged"))?;
        let account = row
            .account_id
            .as_deref()
            .ok_or_else(|| err("accountChanged"))?;
        let provider = crate::Database::get_provider_by_id_on_connection(
            conn,
            vault,
            provider_id,
            app.as_str(),
        )?
        .ok_or_else(|| err("providerChanged"))?;
        let revision: String = conn.query_row(
            "SELECT api_key_revision FROM loongport_vendor WHERE id=?1",
            [row_id],
            |r| r.get(0),
        )?;
        let receipt = ledger::Receipt {
            schema_version: 1,
            operation_id: id.into(),
            storage_binding: binding.clone(),
            account_identity: account_digest(&row.vendor_id, account)?,
            row_id,
            vendor_id: row.vendor_id,
            plan: plan.into(),
            app: app.as_str().into(),
            provider_id: provider_id.into(),
            provider_revision: crate::Database::provider_update_digest(&provider)?,
            credential_revision: revision,
            configuration_revision,
            completed_configuration_revision: None,
            catalog_decision,
            prepared_request: request,
            state: "prepared".into(),
            receipt_revision: 1,
        };
        check_authority(conn, vault, &receipt)?;
        ledger::prepare_on(conn, &receipt)
    })?;
    if state.db.secrets.lifecycle_epoch() != epoch {
        return Err(err("storageChanged"));
    }
    Ok((receipt, epoch))
}
fn admit(state: &AppState, receipt: &ledger::Receipt, epoch: u64) -> Result<(), AppError> {
    if state.db.secrets.lifecycle_epoch() != epoch {
        return Err(err("storageChanged"));
    }
    with_bound(state, &receipt.storage_binding, |conn, vault| {
        ledger::transition_on(
            conn,
            &receipt.storage_binding,
            &receipt.operation_id,
            receipt.receipt_revision,
            "applying",
            |tx| {
                if state.db.secrets.lifecycle_epoch() != epoch {
                    return Err(err("storageChanged"));
                }
                check_authority(tx, vault, receipt)
            },
        )?;
        Ok(())
    })
}
fn acknowledge(
    state: &AppState,
    receipt: &ledger::Receipt,
    epoch: u64,
    success: bool,
) -> Result<(), AppError> {
    if state.db.secrets.lifecycle_epoch() != epoch {
        return Err(err("storageChanged"));
    }
    let completed_revision = if success {
        let app = receipt.app.parse::<AppType>()?;
        Some(crate::services::provider::service_configuration_revision(
            state, &app,
        )?)
    } else {
        None
    };
    with_bound(state, &receipt.storage_binding, |conn, vault| {
        let current =
            ledger::read_on(conn, &receipt.operation_id)?.ok_or_else(|| err("missingOperation"))?;
        if current.state != "applying" {
            return Err(err("receiptChanged"));
        }
        ledger::transition_with_configuration_on(
            conn,
            &receipt.storage_binding,
            &receipt.operation_id,
            current.receipt_revision,
            if success { "completed" } else { "unknown" },
            completed_revision,
            |_tx| {
                if state.db.secrets.lifecycle_epoch() != epoch {
                    return Err(err("storageChanged"));
                }
                let _ = vault;
                Ok(())
            },
        )?;
        Ok(())
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptView<'a> {
    operation_id: &'a str,
    row_id: i64,
    plan: &'a str,
    app: &'a str,
    provider_id: &'a str,
    receipt_revision: u64,
    status: &'a str,
    operation_state: &'a str,
    completed_configuration_revision: Option<&'a str>,
}
fn receipt_view(receipt: &ledger::Receipt, status: &str) -> serde_json::Value {
    serde_json::to_value(ReceiptView {
        operation_id: &receipt.operation_id,
        row_id: receipt.row_id,
        plan: &receipt.plan,
        app: &receipt.app,
        provider_id: &receipt.provider_id,
        receipt_revision: receipt.receipt_revision,
        status,
        operation_state: &receipt.state,
        completed_configuration_revision: receipt.completed_configuration_revision.as_deref(),
    })
    .expect("receipt view contains only serializable scalar fields")
}
fn receipt_wire(receipt: &ledger::Receipt) -> serde_json::Value {
    receipt_view(receipt, &receipt.state)
}

// The new target is reviewed independently of the old store binding. The
// proof contains only hashes and revisions, and is recomputed under both owners.
fn replacement_proof_locked(
    state: &AppState,
    app: &AppType,
    operation: &str,
    target_row: i64,
    target_plan: &str,
) -> Result<serde_json::Value, AppError> {
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    let epoch = state.db.secrets.lifecycle_epoch();
    let configuration = crate::services::provider::service_configuration_revision(state, app)?;
    with_bound(state, &binding, |conn, vault| {
        let old = ledger::read_on(conn, operation)?.ok_or_else(|| err("missingOperation"))?;
        if old.app != app.as_str()
            || !matches!(old.state.as_str(), "prepared" | "applying" | "unknown")
        {
            return Err(err("invalidTransition"));
        }
        let row = creds::get(conn, vault, target_row)?.ok_or_else(|| err("accountChanged"))?;
        let vendor = Vendor::from_id(&row.vendor_id).ok_or_else(|| err("accountChanged"))?;
        let account = row
            .account_id
            .as_deref()
            .ok_or_else(|| err("accountChanged"))?;
        if !crate::vendor::plans_for_account(vendor, Some(account))
            .iter()
            .any(|p| p.id_segment == target_plan)
            || crate::vendor::config_for(vendor, target_plan, app).is_none()
        {
            return Err(err("invalidPlan"));
        }
        let provider_id = provision::provider_id_for(target_plan, account);
        let provider = crate::Database::get_provider_by_id_on_connection(
            conn,
            vault,
            &provider_id,
            app.as_str(),
        )?
        .ok_or_else(|| err("providerChanged"))?;
        let provider_revision = crate::Database::provider_update_digest(&provider)?;
        let credential_revision: String = conn.query_row(
            "SELECT api_key_revision FROM loongport_vendor WHERE id=?1",
            [target_row],
            |r| r.get(0),
        )?;
        if !crate::database::credential_revision::valid(&credential_revision) {
            return Err(err("accountChanged"));
        }
        if state.db.secrets.lifecycle_epoch() != epoch {
            return Err(err("storageChanged"));
        }
        let material = serde_json::to_vec(&(
            &binding,
            epoch,
            app.as_str(),
            operation,
            old.receipt_revision,
            target_row,
            target_plan,
            account_digest(&row.vendor_id, account)?,
            &provider_id,
            provider_revision,
            credential_revision,
            configuration,
        ))
        .map_err(|_| err("invalidRequest"))?;
        Ok(
            serde_json::json!({"status":"newIntentReviewRequired", "operationId":old.operation_id,
            "receiptRevision":old.receipt_revision, "app":old.app, "originalRowId":old.row_id,
            "originalPlan":old.plan, "targetRowId":target_row, "targetPlan":target_plan,
            "targetProviderId":provider_id, "reviewToken":crate::live::engine::sha256_hex(&material)}),
        )
    })
}

pub(super) async fn prepare_owned(
    state: AppState,
    row_id: i64,
    plan: String,
    app: AppType,
    operation: String,
    catalog_decision: Option<crate::services::provider::codex_direct::CatalogDecision>,
) -> Result<serde_json::Value, AppError> {
    if !provision::VENDOR_APPS.contains(&app)
        || uuid::Uuid::parse_str(&operation).is_err()
        || (catalog_decision.is_some() && app != AppType::Codex)
        || catalog_decision
            .as_ref()
            .is_some_and(|decision| !is_digest(&decision.revision))
    {
        return Err(err("invalidRequest"));
    }
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()?;
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    let existing = with_bound(&state, &binding, |conn, _| {
        ledger::read_on(conn, &operation)
    })?;
    if let Some(existing) = existing {
        if existing.storage_binding != binding
            || existing.row_id != row_id
            || existing.app != app.as_str()
            || existing.plan != plan
            || !matches!((serde_json::to_value(&existing.catalog_decision), serde_json::to_value(&catalog_decision)), (Ok(left), Ok(right)) if left == right)
        {
            return Err(err("operationScopeChanged"));
        }
        with_bound(&state, &binding, |conn, vault| {
            let row = creds::get(conn, vault, row_id)?.ok_or_else(|| err("accountChanged"))?;
            let account = row
                .account_id
                .as_deref()
                .ok_or_else(|| err("accountChanged"))?;
            if row.vendor_id != existing.vendor_id
                || account_digest(&row.vendor_id, account)? != existing.account_identity
            {
                return Err(err("accountChanged"));
            }
            Ok(())
        })?;
        return Ok(receipt_wire(&existing));
    }
    let provider = with_bound(&state, &binding, |conn, vault| {
        let row = creds::get(conn, vault, row_id)?.ok_or_else(|| err("accountChanged"))?;
        let account = row
            .account_id
            .as_deref()
            .ok_or_else(|| err("accountChanged"))?;
        let vendor = Vendor::from_id(&row.vendor_id).ok_or_else(|| err("accountChanged"))?;
        if !crate::vendor::plans_for_account(vendor, Some(account))
            .iter()
            .any(|p| p.id_segment == plan)
            || crate::vendor::config_for(vendor, &plan, &app).is_none()
        {
            return Err(err("invalidPlan"));
        }
        Ok(provision::provider_id_for(&plan, account))
    })?;
    let prepared = if app.supports_local_proxy() {
        Some(routing::prepare_routing_strategy(
            &state,
            &app,
            &change(&provider, catalog_decision.clone()),
            &operation,
        )?)
    } else {
        None
    };
    let _app = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    let (receipt, _) = capture_locked(
        &state,
        CaptureTarget {
            row_id,
            plan: &plan,
            app: &app,
            provider_id: &provider,
        },
        prepared,
        catalog_decision,
        &operation,
    )?;
    Ok(receipt_wire(&receipt))
}

pub(super) async fn query_owned(
    state: &AppState,
    app: AppType,
    operation: Option<String>,
) -> Result<serde_json::Value, AppError> {
    if !provision::VENDOR_APPS.contains(&app) {
        return Err(err("invalidRequest"));
    }
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()?;
    let _app = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    let requested_id = operation.is_some();
    let result = with_bound(state, &binding, |conn, _| match operation {
        Some(id) => {
            let receipt = ledger::read_on(conn, &id)?;
            if receipt
                .as_ref()
                .is_some_and(|r| r.storage_binding != binding || r.app != app.as_str())
            {
                return Err(err("storageChanged"));
            }
            Ok(receipt)
        }
        None => ledger::latest_for_app(conn, &binding, app.as_str()),
    });
    if let Ok(Some(receipt)) = &result {
        let identity_matches = with_bound(state, &binding, |conn, vault| {
            let row =
                creds::get(conn, vault, receipt.row_id)?.ok_or_else(|| err("accountChanged"))?;
            let account = row
                .account_id
                .as_deref()
                .ok_or_else(|| err("accountChanged"))?;
            Ok(row.vendor_id == receipt.vendor_id
                && account_digest(&row.vendor_id, account)? == receipt.account_identity)
        });
        if !matches!(identity_matches, Ok(true)) {
            return Ok(
                serde_json::json!({"status":"unknown", "operationId":receipt.operation_id, "receiptRevision":receipt.receipt_revision, "app":receipt.app}),
            );
        }
    }
    if let Ok(Some(receipt)) = &result {
        if matches!(receipt.state.as_str(), "applying" | "unknown") {
            if let Some(routing::PreparedRoutingRequest::Native { request }) =
                &receipt.prepared_request
            {
                if routing::query_routing_request_locked(state, &app, request)
                    .is_ok_and(|value| value.status == "completed")
                {
                    let reconciled = with_bound(state, &binding, |conn, _| {
                        ledger::transition_on(
                            conn,
                            &binding,
                            &receipt.operation_id,
                            receipt.receipt_revision,
                            "completed",
                            |_| Ok(()),
                        )
                    })?;
                    return Ok(receipt_wire(&reconciled));
                }
            }
        }
    }
    match result {
        Ok(Some(receipt))
            if matches!(receipt.state.as_str(), "prepared" | "applying" | "unknown") =>
        {
            Ok(receipt_view(&receipt, "unknown"))
        }
        Ok(Some(receipt)) => Ok(receipt_wire(&receipt)),
        Ok(None) if requested_id => Ok(serde_json::json!({"status":"unknown"})),
        Ok(None) => Ok(serde_json::json!({"status":"none"})),
        Err(_) => with_bound(state, &binding, |conn, _| {
            let outstanding = ledger::outstanding_for_review(conn, app.as_str())?;
            Ok(match outstanding {
                Some(receipt) => {
                    serde_json::json!({"status":"unknown", "operationId":receipt.operation_id, "receiptRevision":receipt.receipt_revision, "app":receipt.app})
                }
                None => serde_json::json!({"status":"unknown"}),
            })
        }),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_for(
    handle: &tauri::AppHandle,
    state: AppState,
    row_id: i64,
    plan: String,
    app: AppType,
    provider_id: String,
    operation_id: String,
    expected_revision: u64,
    quit: Option<bool>,
) -> Result<serde_json::Value, AppError> {
    let worker_handle = handle.clone();
    apply_for_owned(
        state,
        row_id,
        plan,
        app,
        provider_id,
        operation_id,
        expected_revision,
        quit,
        Box::new(move |app, id| routing::publish_routing_selection(&worker_handle, app, id)),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_for_owned(
    state: AppState,
    row_id: i64,
    plan: String,
    app: AppType,
    provider_id: String,
    operation_id: String,
    expected_revision: u64,
    quit: Option<bool>,
    selected: routing::RoutingSelectionPublished,
) -> Result<serde_json::Value, AppError> {
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    let receipt = with_bound(&state, &binding, |conn, _| {
        ledger::read_on(conn, &operation_id)
    })?
    .ok_or_else(|| err("prepareRequired"))?;
    if receipt.storage_binding != binding
        || receipt.app != app.as_str()
        || receipt.row_id != row_id
        || receipt.plan != plan
        || receipt.provider_id != provider_id
    {
        return Err(err("operationScopeChanged"));
    }
    apply_owned(state, operation_id, expected_revision, quit, selected).await
}

#[tauri::command]
pub async fn vendor_prepare_connection(
    state: tauri::State<'_, AppState>,
    row_id: i64,
    plan: String,
    app: String,
    operation_id: String,
    catalog_decision: Option<crate::services::provider::codex_direct::CatalogDecision>,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    prepare_owned(
        state.inner().clone(),
        row_id,
        plan,
        app,
        operation_id,
        catalog_decision,
    )
    .await
    .map_err(|error| safe_error(&error))
}

#[tauri::command]
pub async fn vendor_query_connection(
    state: tauri::State<'_, AppState>,
    app: String,
    operation_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    query_owned(state.inner(), app, operation_id)
        .await
        .map_err(|error| safe_error(&error))
}

#[tauri::command]
pub async fn vendor_preview_connection_catalog(
    state: tauri::State<'_, AppState>,
    app: String,
    operation_id: String,
    catalog_decision: crate::services::provider::codex_direct::CatalogDecision,
    quit_chatgpt: Option<bool>,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    if app != AppType::Codex {
        return Err("vendorConnection.invalidRequest".into());
    }
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()
        .map_err(|error| safe_error(&error))?;
    let binding = state
        .db
        .secrets
        .read()
        .map_err(|error| safe_error(&error))?
        .identity()
        .store_id()
        .to_owned();
    let receipt = with_bound(&state, &binding, |conn, vault| {
        let receipt =
            ledger::read_on(conn, &operation_id)?.ok_or_else(|| err("missingOperation"))?;
        if receipt.storage_binding != binding
            || receipt.app != app.as_str()
            || receipt.state != "prepared"
        {
            return Err(err("receiptChanged"));
        }
        check_authority(conn, vault, &receipt)?;
        Ok(receipt)
    })
    .map_err(|error| safe_error(&error))?;
    let mut draft = change(&receipt.provider_id, Some(catalog_decision));
    draft.catalog_preview_only = true;
    let preview = routing::application_routing_confirmation(
        &state,
        &app,
        &draft,
        quit_chatgpt,
        crate::relay::chatgpt_app::needs_user_attention(),
        crate::services::provider::is_app_taken_over(&state, &app),
    )
    .map_err(|error| safe_error(&error))?
    .ok_or_else(|| "vendorConnection.invalidResult".to_string())?;
    serde_json::to_value(preview).map_err(|_| "vendorConnection.invalidResult".to_string())
}

#[tauri::command]
pub async fn vendor_preview_connection_review(
    state: tauri::State<'_, AppState>,
    app: String,
    operation_id: String,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    if !provision::VENDOR_APPS.contains(&app) {
        return Err("vendorConnection.invalidRequest".into());
    }
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()
        .map_err(|error| safe_error(&error))?;
    let _app = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    let revision = crate::services::provider::service_configuration_revision(&state, &app)
        .map_err(|error| safe_error(&error))?;
    let binding = state
        .db
        .secrets
        .read()
        .map_err(|error| safe_error(&error))?
        .identity()
        .store_id()
        .to_owned();
    with_bound(&state, &binding, |conn, vault| {
        let receipt = ledger::read_on(conn, &operation_id)?.ok_or_else(|| err("missingOperation"))?;
        if receipt.app != app.as_str() || !matches!(receipt.state.as_str(), "prepared" | "applying" | "unknown") { return Err(err("invalidTransition")); }
        check_authority(conn, vault, &receipt)?;
        Ok(serde_json::json!({"status":"reviewRequired", "operationId":receipt.operation_id, "app":receipt.app, "receiptRevision":receipt.receipt_revision, "observedRevision":revision}))
    }).map_err(|error| safe_error(&error))
}

#[tauri::command]
pub async fn vendor_review_connection(
    state: tauri::State<'_, AppState>,
    app: String,
    operation_id: String,
    expected_revision: u64,
    observed_revision: String,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    review_unknown_owned(
        state.inner().clone(),
        app,
        operation_id,
        expected_revision,
        observed_revision,
    )
    .await
    .map_err(|error| safe_error(&error))
}

#[tauri::command]
pub async fn vendor_preview_new_connection_intent(
    state: tauri::State<'_, AppState>,
    app: String,
    operation_id: String,
    target_row_id: i64,
    target_plan: String,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    replacement_intent_owned(
        state.inner().clone(),
        app,
        operation_id,
        target_row_id,
        target_plan,
        None,
    )
    .await
    .map_err(|error| safe_error(&error))
}

#[tauri::command]
pub async fn vendor_confirm_new_connection_intent(
    state: tauri::State<'_, AppState>,
    app: String,
    operation_id: String,
    target_row_id: i64,
    target_plan: String,
    review_token: String,
) -> Result<serde_json::Value, String> {
    let app = app.parse::<AppType>().map_err(|error| safe_error(&error))?;
    replacement_intent_owned(
        state.inner().clone(),
        app,
        operation_id,
        target_row_id,
        target_plan,
        Some(review_token),
    )
    .await
    .map_err(|error| safe_error(&error))
}

pub(super) async fn replacement_intent_owned(
    state: AppState,
    app: AppType,
    operation: String,
    target_row: i64,
    target_plan: String,
    review_token: Option<String>,
) -> Result<serde_json::Value, AppError> {
    if !provision::VENDOR_APPS.contains(&app) {
        return Err(err("invalidRequest"));
    }
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()?;
    let _app = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    let epoch = state.db.secrets.lifecycle_epoch();
    let review = replacement_proof_locked(&state, &app, &operation, target_row, &target_plan)?;
    if state.db.secrets.lifecycle_epoch() != epoch {
        return Err(err("storageChanged"));
    }
    let Some(token) = review_token else {
        return Ok(review);
    };
    if review["reviewToken"].as_str() != Some(token.as_str()) {
        return Err(err("applicationChanged"));
    }
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    with_bound(&state, &binding, |conn, _| {
        if state.db.secrets.lifecycle_epoch() != epoch {
            return Err(err("storageChanged"));
        }
        let mut old = ledger::read_on(conn, &operation)?.ok_or_else(|| err("missingOperation"))?;
        if Some(old.receipt_revision) != review["receiptRevision"].as_u64() {
            return Err(err("receiptChanged"));
        }
        if old.state == "applying" {
            old = ledger::transition_on(
                conn,
                &old.storage_binding,
                &old.operation_id,
                old.receipt_revision,
                "unknown",
                |_| Ok(()),
            )?;
        }
        let next = match old.state.as_str() {
            "prepared" => "failedBeforeWrite",
            "unknown" => "reviewed",
            _ => return Err(err("invalidTransition")),
        };
        ledger::transition_on(
            conn,
            &old.storage_binding,
            &old.operation_id,
            old.receipt_revision,
            next,
            |_| Ok(()),
        )?;
        Ok(serde_json::json!({"status":"newIntentPermitted", "app":app.as_str()}))
    })
}

pub(super) async fn apply_owned(
    state: AppState,
    operation: String,
    expected: u64,
    quit: Option<bool>,
    selected: routing::RoutingSelectionPublished,
) -> Result<serde_json::Value, AppError> {
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    let epoch = state.db.secrets.lifecycle_epoch();
    let receipt = with_bound(&state, &binding, |conn, _| {
        ledger::read_on(conn, &operation)
    })?
    .ok_or_else(|| err("missingOperation"))?;
    if receipt.storage_binding != binding {
        return Err(err("storageChanged"));
    }
    match receipt.state.as_str() {
        "completed" => return Ok(receipt_wire(&receipt)),
        "prepared" if receipt.receipt_revision == expected => {}
        "prepared" => return Err(err("receiptChanged")),
        _ => {
            return Ok(serde_json::json!({"status":"unknown", "operationId":receipt.operation_id}))
        }
    }
    let app = receipt.app.parse::<AppType>()?;
    match &receipt.prepared_request {
        Some(routing::PreparedRoutingRequest::Native { request })
            if app.supports_local_proxy()
                && request.id == receipt.operation_id
                && request.scope == crate::mode::state::SaveScope::ApplicationRouting
                && request.validate().is_ok() => {}
        Some(routing::PreparedRoutingRequest::Legacy { review })
            if app.supports_local_proxy()
                && is_digest(&review.draft_digest)
                && is_digest(&review.revision) => {}
        None if !app.supports_local_proxy() => {}
        _ => return Err(err("invalidReceipt")),
    }
    if let Some(prepared) = receipt.prepared_request.clone() {
        let started = Arc::new(AtomicBool::new(false));
        let admitted = started.clone();
        let original_db = state.db.clone();
        let before = receipt.clone();
        let after = receipt.clone();
        return routing::run_prepared_routing_owned(
            state,
            app,
            change(&receipt.provider_id, receipt.catalog_decision.clone()),
            quit,
            prepared,
            Box::new(move |worker| {
                if !Arc::ptr_eq(&worker.db, &original_db) {
                    return Err(err("storageChanged"));
                }
                admit(worker, &before, epoch)?;
                admitted.store(true, Ordering::Release);
                Ok(())
            }),
            Box::new(move |worker, outcome| {
                if started.load(Ordering::Acquire) {
                    acknowledge(
                        worker,
                        &after,
                        epoch,
                        outcome.as_ref().is_ok_and(|v| v["status"] == "switched"),
                    )?;
                }
                Ok(())
            }),
            selected,
        )
        .await;
    }
    let sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _sync = sync;
        let _app =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
        if crate::services::provider::service_configuration_revision(&state, &app)?
            != receipt.configuration_revision
        {
            return Err(err("applicationChanged"));
        }
        admit(&state, &receipt, epoch)?;
        let provider_name = state.db.get_provider_by_id(&receipt.provider_id, app.as_str())?
            .map(|provider| provider.name)
            .unwrap_or_default();
        let result = ProviderService::switch_with_catalog_locked(
            &state,
            app.clone(),
            &receipt.provider_id,
            None,
        );
        acknowledge(&state, &receipt, epoch, result.is_ok())?;
        let result = result?;
        drop(_app);
        selected(&app, &receipt.provider_id);
        Ok(serde_json::json!({"status":"switched","providerName":provider_name,"chatgptWasRunning":false,"chatgptRelaunched":false,"warnings":result.warnings}))
    })
    .await
    .map_err(|error| AppError::Message(error.to_string()))?
}

/// Explicitly retire an uncertain prior local attempt after reviewing the current image.
/// This keeps the original binding; it never asserts that a client adopted the target.
pub(super) async fn review_unknown_owned(
    state: AppState,
    app: AppType,
    operation: String,
    expected: u64,
    observed_revision: String,
) -> Result<serde_json::Value, AppError> {
    if !provision::VENDOR_APPS.contains(&app) {
        return Err(err("invalidRequest"));
    }
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()?;
    let _app = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    if crate::services::provider::service_configuration_revision(&state, &app)? != observed_revision
    {
        return Err(err("applicationChanged"));
    }
    let binding = state.db.secrets.read()?.identity().store_id().to_owned();
    with_bound(&state, &binding, |conn, vault| {
        let mut receipt =
            ledger::read_on(conn, &operation)?.ok_or_else(|| err("missingOperation"))?;
        if receipt.app != app.as_str() || receipt.receipt_revision != expected {
            return Err(err("receiptChanged"));
        }
        check_authority(conn, vault, &receipt)?;
        if receipt.state == "applying" {
            receipt = ledger::transition_on(
                conn,
                &receipt.storage_binding,
                &receipt.operation_id,
                receipt.receipt_revision,
                "unknown",
                |_| Ok(()),
            )?;
        }
        let next = match receipt.state.as_str() {
            "unknown" => "reviewed",
            "prepared" => "failedBeforeWrite",
            _ => return Err(err("invalidTransition")),
        };
        let result = ledger::transition_on(
            conn,
            &receipt.storage_binding,
            &receipt.operation_id,
            receipt.receipt_revision,
            next,
            |_| Ok(()),
        )?;
        Ok(receipt_wire(&result))
    })
}
