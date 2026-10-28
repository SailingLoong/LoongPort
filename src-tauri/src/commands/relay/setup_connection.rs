//! Setup only journals the original application's reviewed operation. All
//! current/live/MCP writes and native request recovery stay with routing owners.
use super::*;
use crate::services::application_selection::{
    self as routing, ApplicationRoutingChange, PreparedRoutingRequest, TierSelection,
};
use crate::services::provider::codex_direct::CatalogDecision;
use serde_json::Value;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayConnectionReview {
    pub receipt: RelaySetupReceipt,
    pub confirmation: Option<Value>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayConnectionOutcome {
    pub receipt: RelaySetupReceipt,
    pub result: Option<Value>,
}
pub(super) fn account_for_operation(
    state: &AppState,
    journal: &Journal,
) -> Result<creds::RelayAccount, AppError> {
    let receipt = journal.receipt()?;
    let account = with_conn(state, |conn, vault| {
        creds::get(conn, vault, receipt.relay_id)
    })?
    .ok_or_else(|| setup_error("accountUnavailable"))?;
    if account_identity(&account)? != receipt.account_identity {
        return Err(setup_error("accountChanged"));
    }
    Ok(account)
}
fn target_for<'a>(
    receipt: &'a RelaySetupReceipt,
    app: &str,
) -> Result<&'a RelayTargetFact, AppError> {
    let target = receipt
        .targets
        .iter()
        .find(|target| target.app == app)
        .ok_or_else(|| setup_error("selectionChanged"))?;
    if !matches!(
        target.configuration,
        ConfigurationState::Prepared | ConfigurationState::Existing
    ) {
        return Err(setup_error("configurationNotReady"));
    }
    Ok(target)
}
fn check_projection(
    state: &AppState,
    account: &creds::RelayAccount,
    target: &RelayTargetFact,
) -> Result<(), AppError> {
    let provider = state
        .db
        .get_provider_by_id(&target.provider_id, &target.app)?
        .ok_or_else(|| setup_error("configurationChanged"))?;
    provision::validate_selected_provider_identity(account, &target.group_id, &provider)
}
fn observed(
    state: &AppState,
    app: &AppType,
    target: &RelayTargetFact,
    relay_id: i64,
) -> Result<(bool, String), AppError> {
    let before = crate::services::provider::service_configuration_revision(state, app)?;
    let overview = crate::commands::application_overview::application_overview(state, app)
        .map_err(|_| setup_error("applicationReadFailed"))?;
    let provider = state
        .db
        .get_provider_by_id(&target.provider_id, app.as_str())?;
    let provider_matches = provider
        .as_ref()
        .map(crate::database::Database::provider_update_digest)
        .transpose()?
        .as_deref()
        == target
            .connection
            .intent
            .as_ref()
            .and_then(|intent| intent.provider_revision.as_deref());
    let after = crate::services::provider::service_configuration_revision(state, app)?;
    if before != after {
        return Err(setup_error("applicationChanged"));
    }
    let current=provider.is_some() && provider_matches && overview.configurations.iter().any(|row|row.provider_id==target.provider_id && row.presentation.is_current && matches!(&row.source,crate::commands::application_overview::ConfigurationSource::Relay) && matches!(&row.account,Some(crate::services::service_sources::SourceAccountReference::Relay{id}) if *id==relay_id));
    Ok((current, after))
}

#[tauri::command]
pub async fn relay_prepare_application_connection(
    app: tauri::AppHandle,
    operation_id: String,
    target_app: String,
    expected_receipt_revision: u64,
    catalog_decision: Option<CatalogDecision>,
) -> Result<RelayConnectionReview, String> {
    prepare_connection(
        &app,
        &operation_id,
        &target_app,
        expected_receipt_revision,
        catalog_decision,
    )
    .await
    .map_err(|e| e.to_string())
}
async fn prepare_connection(
    app: &tauri::AppHandle,
    operation: &str,
    target_app: &str,
    expected_revision: u64,
    catalog_decision: Option<CatalogDecision>,
) -> Result<RelayConnectionReview, AppError> {
    let state = app.state::<AppState>();
    let journal = Journal::new(&state, operation)?;
    let account = account_for_operation(&state, &journal)?;
    let lock = provision::relay_provision_lock(&account)?;
    let _guard = lock.lock().await;
    let account = account_for_operation(&state, &journal)?;
    let receipt = journal.receipt()?;
    if receipt.receipt_revision != expected_revision {
        return Err(setup_error("receiptChanged"));
    }
    let target = target_for(&receipt, target_app)?;
    if matches!(
        target.connection.state,
        ConnectionState::Applying | ConnectionState::Unknown | ConnectionState::Applied
    ) {
        return Err(setup_error("connectionRequiresReconciliation"));
    }
    check_projection(&state, &account, target)?;
    let app_type = AppType::from_str(target_app)?;
    let change = ApplicationRoutingChange {
        order: None,
        selection: Some(TierSelection {
            provider_id: target.provider_id.clone(),
            model: None,
        }),
        catalog_decision,
        catalog_preview_only: false,
    };
    let before_revision =
        crate::services::provider::service_configuration_revision(&state, &app_type)?;
    let provider = state
        .db
        .get_provider_by_id(&target.provider_id, target_app)?
        .ok_or_else(|| setup_error("configurationChanged"))?;
    provision::validate_selected_provider_identity(&account, &target.group_id, &provider)?;
    let provider_revision = Some(crate::database::Database::provider_update_digest(
        &provider,
    )?);
    // Preview is read-only and cannot itself authorize publication.
    let mut preview = change.clone();
    preview.catalog_preview_only = preview.catalog_decision.is_some();
    let confirmation = routing::application_routing_confirmation(
        &state,
        &app_type,
        &preview,
        None,
        chatgpt_app::needs_user_attention(),
        crate::services::provider::is_app_taken_over(&state, &app_type),
    )?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let prepared = routing::prepare_routing_strategy(&state, &app_type, &change, &request_id)?;
    if before_revision
        != crate::services::provider::service_configuration_revision(&state, &app_type)?
    {
        return Err(setup_error("applicationChanged"));
    }
    let intent = RelayConnectionIntent {
        prepared_request: serde_json::to_value(prepared)
            .map_err(|_| setup_error("invalidConnectionIntent"))?,
        change: serde_json::to_value(change).map_err(|_| setup_error("invalidConnectionIntent"))?,
        before_revision,
        provider_revision,
    };
    let receipt = journal.with(|conn| {
        ledger::prepare_connection_on(
            conn,
            &journal.binding,
            operation,
            target_app,
            expected_revision,
            &request_id,
            intent,
            now(),
        )
    })?;
    Ok(RelayConnectionReview {
        receipt,
        confirmation: confirmation
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| setup_error("invalidConnectionIntent"))?,
    })
}

#[tauri::command]
pub async fn relay_apply_prepared_tier(
    app: tauri::AppHandle,
    operation_id: String,
    target_app: String,
    expected_receipt_revision: u64,
    quit_chatgpt: Option<bool>,
) -> Result<RelayConnectionOutcome, String> {
    apply_connection(
        &app,
        &operation_id,
        &target_app,
        expected_receipt_revision,
        quit_chatgpt,
    )
    .await
    .map_err(|e| e.to_string())
}
async fn apply_connection(
    app: &tauri::AppHandle,
    operation: &str,
    target_app: &str,
    expected_revision: u64,
    quit_chatgpt: Option<bool>,
) -> Result<RelayConnectionOutcome, AppError> {
    let state = app.state::<AppState>();
    let journal = Journal::new(&state, operation)?;
    let account = account_for_operation(&state, &journal)?;
    let lock = provision::relay_provision_lock(&account)?;
    let _guard = lock.lock().await;
    let account = account_for_operation(&state, &journal)?;
    let receipt = journal.receipt()?;
    if receipt.receipt_revision != expected_revision {
        return Err(setup_error("receiptChanged"));
    }
    let target = target_for(&receipt, target_app)?;
    if target.connection.state != ConnectionState::ConfirmationRequired {
        return Err(setup_error("connectionRequiresReview"));
    }
    check_projection(&state, &account, target)?;
    let intent = target
        .connection
        .intent
        .as_ref()
        .ok_or_else(|| setup_error("connectionRequiresReview"))?;
    let request_id = target
        .connection
        .request_id
        .as_deref()
        .ok_or_else(|| setup_error("connectionRequiresReview"))?;
    let app_type = AppType::from_str(target_app)?;
    if intent.before_revision
        != crate::services::provider::service_configuration_revision(&state, &app_type)?
    {
        let receipt = journal.with(|conn| {
            ledger::connection_result_on(
                conn,
                &journal.binding,
                operation,
                target_app,
                request_id,
                ConnectionState::FailedBeforeWrite,
                None,
                None,
                false,
                Some("applicationChanged"),
                now(),
            )
        })?;
        return Ok(RelayConnectionOutcome {
            receipt,
            result: None,
        });
    }
    let change: ApplicationRoutingChange = serde_json::from_value(intent.change.clone())
        .map_err(|_| setup_error("invalidConnectionIntent"))?;
    let prepared: PreparedRoutingRequest = serde_json::from_value(intent.prepared_request.clone())
        .map_err(|_| setup_error("invalidConnectionIntent"))?;
    let journal = journal.for_account(&account)?;
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let admission_journal = journal.clone();
    let admitted = started.clone();
    let admission_rejected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let rejected = admission_rejected.clone();
    let is_native = matches!(&prepared, PreparedRoutingRequest::Native { .. });
    let admitted_app = target_app.to_owned();
    let acknowledge_journal = journal.clone();
    let acknowledged_app = target_app.to_owned();
    let original_request_id = request_id.to_owned();
    let result = routing::apply_prepared_routing_with_admission(
        app,
        app_type,
        change,
        quit_chatgpt,
        prepared,
        Box::new(move |state| {
            if !Arc::ptr_eq(&state.db, &admission_journal.db) {
                return Err(setup_error("storageChanged"));
            }
            let acceptance = admission_journal.with_storage(|conn, storage| {
                ledger::begin_connection_authorized_on(
                    conn,
                    &admission_journal.binding,
                    &admission_journal.operation,
                    &admitted_app,
                    expected_revision,
                    quit_chatgpt,
                    now(),
                    |tx| {
                        crate::database::Database::validate_relay_setup_authority_on(
                            tx,
                            storage,
                            admission_journal
                                .authority
                                .as_ref()
                                .ok_or_else(|| setup_error("accountChanged"))?,
                            state.db.secrets.lifecycle_epoch(),
                        )
                        .map(|_| ())
                    },
                )
            });
            if acceptance.is_err() {
                rejected.store(true, std::sync::atomic::Ordering::Release);
            }
            acceptance?;
            admitted.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }),
        Box::new(move |state, result| {
            let value = result.as_ref().ok();
            if value.is_some_and(|value| value["status"] == "confirmationRequired") {
                acknowledge_journal.with(|conn| {
                    ledger::connection_result_on(
                        conn,
                        &acknowledge_journal.binding,
                        &acknowledge_journal.operation,
                        &acknowledged_app,
                        &original_request_id,
                        ConnectionState::ConfirmationRequired,
                        None,
                        None,
                        false,
                        None,
                        now(),
                    )
                })?;
            } else if value.is_some_and(|value| value["status"] == "requestResolved") {
                // Every original native receipt outranks this invocation's local
                // not-started flag. Pending/conflict/unknown cannot be replaced.
                observe_connection(state, &acknowledge_journal, &acknowledged_app, false)?;
            } else if !started.load(std::sync::atomic::Ordering::Acquire)
                && (!is_native || admission_rejected.load(std::sync::atomic::Ordering::Acquire))
            {
                // This original owner never accepted the write; unlike a lost
                // post-admission reply, the failure is proven before effects.
                acknowledge_journal.with(|conn| {
                    ledger::connection_result_on(
                        conn,
                        &acknowledge_journal.binding,
                        &acknowledge_journal.operation,
                        &acknowledged_app,
                        &original_request_id,
                        ConnectionState::FailedBeforeWrite,
                        None,
                        None,
                        false,
                        Some("connectionRejectedBeforeWrite"),
                        now(),
                    )
                })?;
            } else {
                observe_connection(
                    state,
                    &acknowledge_journal,
                    &acknowledged_app,
                    value.is_some_and(|value| value["status"] == "switched"),
                )?;
            }
            Ok(())
        }),
    )
    .await;
    Ok(RelayConnectionOutcome {
        receipt: journal.receipt()?,
        result: result.ok(),
    })
}

pub(super) fn observe_connections(state: &AppState, journal: &Journal) -> Result<(), AppError> {
    for target in journal.receipt()?.targets {
        if matches!(
            target.connection.state,
            ConnectionState::Applying
                | ConnectionState::Unknown
                | ConnectionState::Applied
                | ConnectionState::Changed
        ) {
            observe_connection(state, journal, &target.app, false)?;
        }
    }
    Ok(())
}
fn observe_connection(
    state: &AppState,
    journal: &Journal,
    app: &str,
    acknowledged: bool,
) -> Result<(), AppError> {
    let receipt = journal.receipt()?;
    let target = target_for(&receipt, app)?;
    let Some(intent) = &target.connection.intent else {
        return Ok(());
    };
    let Some(request_id) = &target.connection.request_id else {
        return Ok(());
    };
    let app_type = AppType::from_str(app)?;
    let prepared: PreparedRoutingRequest = serde_json::from_value(intent.prepared_request.clone())
        .map_err(|_| setup_error("invalidConnectionIntent"))?;
    let completed = acknowledged
        || target.connection.write_acknowledged == Some(true)
        || target.connection.state == ConnectionState::Applied
        || match prepared {
            PreparedRoutingRequest::Native { request } => {
                routing::query_routing_request(state, &app_type, request)
                    .is_ok_and(|result| result.status == "completed")
            }
            PreparedRoutingRequest::Legacy { .. } => false,
        };
    let observation = observed(state, &app_type, target, receipt.relay_id).ok();
    let current = observation.as_ref().map(|(current, _)| *current);
    let revision = observation.map(|(_, revision)| revision);
    let outcome = match (&current, &revision) {
        (Some(current), Some(revision)) => {
            connection_observation_state(&target.connection, completed, *current, revision)
        }
        _ => ConnectionState::Unknown,
    };
    let reason = if outcome == ConnectionState::Changed {
        Some("applicationChanged")
    } else if !completed {
        Some("applicationOutcomeUnknown")
    } else if current.is_none() {
        Some("applicationReadFailed")
    } else {
        None
    };
    journal.with(|conn| {
        ledger::connection_result_on(
            conn,
            &journal.binding,
            &journal.operation,
            app,
            request_id,
            outcome,
            current,
            revision,
            completed,
            reason,
            now(),
        )
    })?;
    Ok(())
}
