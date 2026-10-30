//! Application tier selection, confirmation and reversible configuration changes.
use crate::services::provider::codex_direct::{CatalogDecision, CatalogPreview};
use crate::{app_config::AppType, error::AppError, services::ProviderService, store::AppState};
#[cfg(feature = "gui")]
use crate::{events::emit_provider_switched, relay::chatgpt_app};
use serde::Serialize;
#[cfg(feature = "gui")]
use tauri::Manager;

/// 切换结果，前端据此出话。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchTierResult {
    pub provider_name: String,
    /// ChatGPT 退出前是不是在跑（决定切换后要不要替用户重开）。
    pub chatgpt_was_running: bool,
    /// 有没有重新打开它。
    pub chatgpt_relaunched: bool,
    /// 非致命的问题（如重开失败），如实带给用户。
    ///
    /// 「退不掉 ChatGPT」**不在这里** —— 那种情况整个命令返回 Err、配置不动，见
    /// [`switch_tier_impl`]。
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
// `rename_all` 只转变体名；变体字段要另加 `rename_all_fields`（serde 1.0.185+）。
// 漏掉它时 `target_name` 蛇形下发、前端读 `targetName` 得 undefined，
// 确认弹窗静默打不开 —— 闸在 tests::switch_tier_command_result_wire_contract。
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SwitchTierCommandResult {
    ConfirmationRequired {
        target_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        catalog: Option<CatalogPreview>,
        quit_required: bool,
    },
    Switched {
        #[serde(flatten)]
        result: SwitchTierResult,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TierSelection {
    pub provider_id: String,
    pub model: Option<String>,
}

pub(crate) fn select_legacy_with_commit(
    state: &AppState,
    app: &AppType,
    selection: &TierSelection,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<crate::services::SwitchResult, AppError> {
    if app.supports_local_proxy() && crate::mode::operation::uses_upstream4_schema(&state.db)? {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    if !app.supports_local_proxy() {
        if selection.model.is_some() {
            return Err(AppError::Config(
                "Application does not support model selection".into(),
            ));
        }
        let result = ProviderService::switch(state, app.clone(), &selection.provider_id)?;
        commit()?;
        return Ok(result);
    }
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    select_legacy_locked_with_commit(state, app, selection, commit)
}

/// The existing legacy transaction, with its original switch lock held by the caller.
/// This is not a native AppWrite and never creates or reports a native save receipt.
fn select_legacy_locked_with_commit(
    state: &AppState,
    app: &AppType,
    selection: &TierSelection,
    commit: impl FnOnce() -> Result<(), AppError>,
) -> Result<crate::services::SwitchResult, AppError> {
    require_legacy_routing(state, app)?;
    let provider = state
        .db
        .get_provider_by_id(&selection.provider_id, app.as_str())?
        .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
    if crate::proxy::managed_accounts::uses_workbuddy_model_route(&provider) {
        return Err(AppError::Config("workbuddy.inference_paused".into()));
    }
    crate::services::provider::validate_provider_selection(&state.db, app, &provider.id)?;
    let selected_settings = selection
        .model
        .as_deref()
        .map(|model| crate::relay::model_catalog::select_model(app, &provider, model))
        .transpose()?;
    crate::services::provider::with_provider_config_transaction(state, app, &provider, || {
        if let Some(settings) = selected_settings {
            state
                .db
                .update_provider_settings_config(app.as_str(), &provider.id, &settings)?;
        }
        crate::proxy::auto_strategy::set_model_pref(
            &state.db,
            app.as_str(),
            selection.model.as_deref(),
        )?;
        let result = ProviderService::switch_locked(state, app.clone(), &selection.provider_id)?;
        commit()?;
        Ok(result)
    })
}

fn order_target(
    state: &AppState,
    app: &AppType,
    order: &RoutingOrder,
) -> Result<crate::mode::state::RoutingOrderTarget, AppError> {
    let (before, planned) = crate::services::order_profiles::prepare_apply(
        &state.db,
        app.as_str(),
        &order.profile_name,
        &order.provider_ids,
    )?;
    Ok(crate::mode::state::RoutingOrderTarget {
        profile_name: order.profile_name.clone(),
        provider_ids: order.provider_ids.clone(),
        before,
        planned,
    })
}

/// Selection, normalized row, explicit model choice and optional order share one intent.
pub(crate) fn select_with_commit(
    state: &AppState,
    app: &AppType,
    selection: &TierSelection,
    order: Option<&RoutingOrder>,
) -> Result<crate::services::SwitchResult, AppError> {
    select_with_commit_with_catalog(state, app, selection, order, None)
}

/// Preview and commit normalize the same selected model without saving an intermediate row.
fn normalized_selection_provider(
    state: &AppState,
    app: &AppType,
    previous: &crate::provider::Provider,
    selection: &TierSelection,
) -> Result<crate::provider::Provider, AppError> {
    let mut provider = previous.clone();
    if let Some(model) = &selection.model {
        provider.settings_config =
            crate::relay::model_catalog::select_model(app, &provider, model)?;
        provider = ProviderService::prepare_provider_update(state, app, provider)?;
    }
    Ok(provider)
}

pub(crate) fn select_with_commit_with_catalog(
    state: &AppState,
    app: &AppType,
    selection: &TierSelection,
    order: Option<&RoutingOrder>,
    catalog_decision: Option<&CatalogDecision>,
) -> Result<crate::services::SwitchResult, AppError> {
    if catalog_decision.is_some() && *app != AppType::Codex {
        return Err(AppError::Config(
            "codex.catalog_takeover_unavailable".into(),
        ));
    }
    if let Some(order) = order {
        if !app.supports_local_proxy() || !order.provider_ids.contains(&selection.provider_id) {
            return Err(AppError::Config(
                "Selected provider is outside the applied order".into(),
            ));
        }
    }
    if !app.supports_local_proxy() || !crate::mode::operation::uses_upstream4_schema(&state.db)? {
        if catalog_decision.is_some() {
            return Err(AppError::Config(
                "codex.catalog_takeover_unavailable".into(),
            ));
        }
        return select_legacy_with_commit(state, app, selection, || {
            if let Some(order) = order {
                crate::services::order_profiles::apply_order(
                    &state.db,
                    app.as_str(),
                    &order.profile_name,
                    &order.provider_ids,
                )?;
            }
            Ok(())
        });
    }
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    select_locked_with_request(state, app, selection, order, catalog_decision, None)
}

/// Caller owns the original per-app switch lock. Never dispatch into legacy locking.
fn select_locked_with_request(
    state: &AppState,
    app: &AppType,
    selection: &TierSelection,
    order: Option<&RoutingOrder>,
    catalog_decision: Option<&CatalogDecision>,
    request: Option<&crate::mode::state::SaveRequest>,
) -> Result<crate::services::SwitchResult, AppError> {
    crate::services::shared_routing::ensure_legacy_selection_allowed(&state.db, app)?;
    if order.is_some() {
        crate::services::shared_routing::ensure_legacy_policy_edit_allowed(&state.db, app)?;
    }
    crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, app)?;
    let previous = state
        .db
        .get_provider_by_id(&selection.provider_id, app.as_str())?
        .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
    let provider = normalized_selection_provider(state, app, &previous, selection)?;
    let mut target = crate::mode::state::PendingTarget {
        save_request: request.cloned(),
        ..Default::default()
    };
    if let Some(model) = &selection.model {
        target.saved_row = Some(crate::mode::state::SavedRow {
            credential_write: None,
            before: crate::database::Database::provider_update_digest(&previous)?,
            provider: crate::database::Database::provider_update_value(&provider)?,
            clear_model_preference: false,
        });
        target.model_preference = Some(crate::mode::state::ModelPreferenceAction::Set {
            model: model.clone(),
        });
    } else {
        target.model_preference = Some(crate::mode::state::ModelPreferenceAction::Clear {});
    }
    target.routing_order = order
        .map(|order| order_target(state, app, order))
        .transpose()?;
    let preserved = crate::mode::controller::select_locked_with_catalog(
        &state.proxy_service,
        app,
        &provider,
        target,
        catalog_decision,
    )?;
    let mut result = crate::services::SwitchResult::default();
    if preserved {
        result
            .warnings
            .push(crate::services::provider::codex_direct::CATALOG_PRESERVED_WARNING.into());
    }
    ProviderService::append_claude_plugin_sync_warning(state, app, &mut result);
    Ok(result)
}

fn apply_order_locked(
    state: &AppState,
    app: &AppType,
    order: &RoutingOrder,
) -> Result<(), AppError> {
    apply_order_locked_with_request(state, app, order, None)
}

fn apply_order_locked_with_request(
    state: &AppState,
    app: &AppType,
    order: &RoutingOrder,
    request: Option<&crate::mode::state::SaveRequest>,
) -> Result<(), AppError> {
    crate::services::shared_routing::ensure_legacy_policy_edit_allowed(&state.db, app)?;
    if !crate::mode::operation::uses_upstream4_schema(&state.db)? {
        return crate::services::order_profiles::apply_order(
            &state.db,
            app.as_str(),
            &order.profile_name,
            &order.provider_ids,
        );
    }
    crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, app)?;
    let target = crate::mode::state::PendingTarget {
        save_request: request.cloned(),
        routing_order: Some(order_target(state, app, order)?),
        ..Default::default()
    };
    crate::mode::controller::apply_order_locked(&state.proxy_service, app, target)
}

pub(crate) fn apply_order_change(
    state: &AppState,
    app: &AppType,
    order: &RoutingOrder,
) -> Result<(), AppError> {
    if !app.supports_local_proxy() {
        return Err(AppError::Config(
            "Application does not support routing order".into(),
        ));
    }
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    apply_order_locked(state, app, order)
}

/// Both current-priority UI entry points use this same locked typed operation.
pub(crate) fn apply_current_order_change(
    state: &AppState,
    app: &AppType,
    ids: &[String],
) -> Result<(), AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    if !app.supports_local_proxy() || !crate::mode::operation::uses_upstream4_schema(&state.db)? {
        return crate::proxy::application_routing::set_order(&state.db, app.as_str(), ids);
    }
    crate::mode::operation::AppWrite::begin_mode(&state.proxy_service, app)?;
    let current = crate::services::order_profiles::get(&state.db, app.as_str())?.current;
    apply_order_locked(
        state,
        app,
        &RoutingOrder {
            profile_name: current,
            provider_ids: ids.to_vec(),
        },
    )
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingOrder {
    pub profile_name: String,
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationRoutingChange {
    pub order: Option<RoutingOrder>,
    pub selection: Option<TierSelection>,
    pub catalog_decision: Option<CatalogDecision>,
    #[serde(default)]
    pub catalog_preview_only: bool,
}

/// Short-lived compatibility review. Its revision is a CAS image, not a
/// persistent completion receipt or permission to enter the native writer.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LegacyRoutingReview {
    pub draft_digest: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(tag = "protocol", rename_all = "camelCase", deny_unknown_fields)]
pub enum PreparedRoutingRequest {
    Native {
        request: crate::mode::state::SaveRequest,
    },
    Legacy {
        review: LegacyRoutingReview,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingRequestResult {
    pub request: crate::mode::state::SaveRequest,
    pub status: String,
}

fn routing_request_scope(request: &crate::mode::state::SaveRequest) -> Result<(), AppError> {
    request.validate()?;
    if request.scope != crate::mode::state::SaveScope::ApplicationRouting {
        return Err(AppError::Config("mode.invalid_save_request".into()));
    }
    Ok(())
}

fn routing_request_digest(
    app: &AppType,
    change: &ApplicationRoutingChange,
) -> Result<String, AppError> {
    if change.catalog_preview_only || (change.order.is_none() && change.selection.is_none()) {
        return Err(AppError::Config("mode.invalid_save_request".into()));
    }
    let bytes = crate::config::serialize_json_bytes(&serde_json::json!([app.as_str(), change]))?;
    Ok(crate::live::engine::sha256_hex(&bytes))
}

fn routing_request_revision(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
) -> Result<String, AppError> {
    crate::mode::controller::routing_request_revision_for_service(
        &state.proxy_service,
        app,
        change
            .selection
            .as_ref()
            .map(|selection| selection.provider_id.as_str()),
    )
}

/// Caller holds the original switch lock. The classification and review image
/// are captured together; a checkpoint never authorizes a legacy fallback.
fn require_legacy_routing(state: &AppState, app: &AppType) -> Result<(), AppError> {
    if *app != AppType::CodexImage
        && (!app.supports_local_proxy()
            || crate::mode::operation::uses_upstream4_schema(&state.db)?)
    {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    crate::secrets::upgrade::checkpoint::ensure_no_pending_checkpoint(
        &crate::live::engine::DeviceStore::for_device(),
    )
}

fn legacy_routing_revision(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
) -> Result<String, AppError> {
    require_legacy_routing(state, app)?;
    if change.catalog_decision.is_some() || change.catalog_preview_only {
        return Err(AppError::Config(
            "codex.catalog_takeover_unavailable".into(),
        ));
    }
    let draft_digest = routing_request_digest(app, change)?;
    // Image has an existing DB-current owner, not a native file/AppWrite owner.
    // Keep its compatibility review explicit and do not admit other nonproxy apps.
    if *app == AppType::CodexImage {
        let selection = change
            .selection
            .as_ref()
            .filter(|selection| selection.model.is_none())
            .ok_or_else(|| AppError::Config("mode.invalid_save_request".into()))?;
        if change.order.is_some()
            || state
                .db
                .get_provider_by_id(&selection.provider_id, app.as_str())?
                .is_none()
        {
            return Err(AppError::Config("mode.invalid_save_request".into()));
        }
        let revision = crate::services::provider::service_configuration_revision(state, app)?;
        let bytes = crate::config::serialize_json_bytes(&serde_json::json!([
            "imageCompatibilityRouting",
            draft_digest,
            revision
        ]))?;
        return Ok(crate::live::engine::sha256_hex(&bytes));
    }
    // Bind only the original owners this draft reads or changes. An order-only
    // review does not become stale merely because an unrelated credential rotates.
    let selection = if let Some(selection) = &change.selection {
        let local = crate::settings::get_current_provider_ready(app)?;
        let current = state.db.get_current_provider(app.as_str())?;
        let digest = |id: &str| -> Result<Option<String>, AppError> {
            state
                .db
                .get_provider_by_id(id, app.as_str())?
                .map(|provider| crate::Database::provider_update_digest(&provider))
                .transpose()
        };
        let selected = digest(&selection.provider_id)?
            .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
        let outgoing = local
            .as_ref()
            .or(current.as_ref())
            .map(|id| digest(id))
            .transpose()?
            .flatten();
        let files = crate::mode::controller::files(app)?
            .into_iter()
            .map(|file| {
                crate::live::engine::read_current(&file.path)
                    .map(|bytes| (file.path, crate::live::engine::digest(bytes.as_deref())))
                    .map_err(AppError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Some(serde_json::json!([
            local,
            current,
            selected,
            outgoing,
            crate::proxy::auto_strategy::get_model_pref_checked(&state.db, app.as_str())?,
            state.db.get_config_snippet(app.as_str())?,
            futures::executor::block_on(state.db.get_live_backup(app.as_str()))?,
            files,
        ]))
    } else {
        None
    };
    let order = change
        .order
        .as_ref()
        .map(|_| crate::database::order_profiles::snapshot(&state.db, app.as_str()))
        .transpose()?;
    // Reclassification stays within the same original app lock and precedes
    // returning the review. Read paths never publish a checkpoint or mutate DB.
    require_legacy_routing(state, app)?;
    let bytes = crate::config::serialize_json_bytes(&serde_json::json!([
        "legacyApplicationRouting",
        app.as_str(),
        draft_digest,
        selection,
        order,
    ]))?;
    Ok(crate::live::engine::sha256_hex(&bytes))
}

fn verify_legacy_routing_review(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    review: &LegacyRoutingReview,
) -> Result<(), AppError> {
    require_legacy_routing(state, app)?;
    if review.draft_digest != routing_request_digest(app, change)?
        || review.revision != legacy_routing_revision(state, app, change)?
    {
        return Err(AppError::Conflict("mode.verification_required".into()));
    }
    Ok(())
}

fn ensure_legacy_change_allowed(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
) -> Result<(), AppError> {
    if change.order.is_some() {
        crate::services::shared_routing::ensure_legacy_policy_edit_allowed(&state.db, app)
    } else {
        crate::services::shared_routing::ensure_legacy_selection_allowed(&state.db, app)
    }
}

pub(crate) fn prepare_routing_strategy(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    id: &str,
) -> Result<PreparedRoutingRequest, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    ensure_legacy_change_allowed(state, app, change)?;
    if *app == AppType::CodexImage
        || (app.supports_local_proxy()
            && !crate::mode::operation::uses_upstream4_schema(&state.db)?)
    {
        let review = LegacyRoutingReview {
            draft_digest: routing_request_digest(app, change)?,
            revision: legacy_routing_revision(state, app, change)?,
        };
        return Ok(PreparedRoutingRequest::Legacy { review });
    }
    if !app.supports_local_proxy() {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    prepare_native_routing_request_locked(state, app, change, id)
        .map(|request| PreparedRoutingRequest::Native { request })
}

/// Existing native tests retain their exact persistent receipt API. Production
/// preparation calls the same locked owner through prepare_routing_strategy.
#[cfg(test)]
pub(crate) fn prepare_routing_request(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    id: &str,
) -> Result<crate::mode::state::SaveRequest, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    prepare_native_routing_request_locked(state, app, change, id)
}

fn prepare_native_routing_request_locked(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    id: &str,
) -> Result<crate::mode::state::SaveRequest, AppError> {
    ensure_legacy_change_allowed(state, app, change)?;
    let request = crate::mode::state::SaveRequest {
        id: id.into(),
        scope: crate::mode::state::SaveScope::ApplicationRouting,
        provider_id: None,
        draft_digest: routing_request_digest(app, change)?,
        revision: routing_request_revision(state, app, change)?,
    };
    routing_request_scope(&request)?;
    Ok(request)
}

/// Caller keeps the original app switch lock across the final CAS and original
/// legacy transaction. Errors retain the original synchronous rollback contract.
fn apply_legacy_routing_locked(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    review: &LegacyRoutingReview,
) -> Result<crate::services::SwitchResult, AppError> {
    verify_legacy_routing_review(state, app, change, review)?;
    if *app == AppType::CodexImage {
        let selection = change
            .selection
            .as_ref()
            .ok_or_else(|| AppError::Config("mode.invalid_save_request".into()))?;
        return ProviderService::switch_with_catalog_locked(
            state,
            app.clone(),
            &selection.provider_id,
            None,
        );
    }
    if let Some(selection) = &change.selection {
        return select_legacy_locked_with_commit(state, app, selection, || {
            if let Some(order) = &change.order {
                crate::services::order_profiles::apply_order(
                    &state.db,
                    app.as_str(),
                    &order.profile_name,
                    &order.provider_ids,
                )?;
            }
            Ok(())
        });
    }
    let order = change
        .order
        .as_ref()
        .ok_or_else(|| AppError::Config("No routing change was supplied".into()))?;
    crate::services::order_profiles::apply_order(
        &state.db,
        app.as_str(),
        &order.profile_name,
        &order.provider_ids,
    )?;
    Ok(crate::services::SwitchResult::default())
}

#[cfg(feature = "gui")]
pub(crate) type RoutingAdmission = Box<dyn FnOnce(&AppState) -> Result<(), AppError> + Send>;
#[cfg(feature = "gui")]
pub(crate) type RoutingAcknowledgement =
    Box<dyn FnOnce(&AppState, &Result<serde_json::Value, AppError>) -> Result<(), AppError> + Send>;

#[cfg(feature = "gui")]
fn apply_reviewed_legacy_routing_locked(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    user_choice: Option<bool>,
    review: &LegacyRoutingReview,
    admission: Option<RoutingAdmission>,
) -> Result<(SwitchTierCommandResult, Option<String>), AppError> {
    verify_legacy_routing_review(state, app, change, review)?;
    let taken_over = crate::services::provider::is_app_taken_over(state, app);
    if let Some(confirmation) = application_routing_confirmation_locked(
        state,
        app,
        change,
        user_choice,
        chatgpt_app::needs_user_attention(),
        taken_over,
    )? {
        return Ok::<_, AppError>((confirmation, None));
    }
    // Capture presentation before any effects. A label read failure must
    // not turn an already-completed legacy write into an ambiguous error.
    let selected = change
        .selection
        .as_ref()
        .map(|selection| selection.provider_id.clone());
    let provider_name = selected
        .as_ref()
        .map(|id| {
            state
                .db
                .get_provider_by_id(id, app.as_str())
                .map(|provider| provider.map(|p| p.name).unwrap_or_default())
        })
        .transpose()?
        .unwrap_or_default();
    // The lock remains held through around(). Quit may flush native bytes;
    // apply_legacy_routing_locked rechecks them before any configuration write.
    verify_legacy_routing_review(state, app, change, review)?;
    if let Some(admit) = admission {
        admit(state)?;
    }
    let action = || apply_legacy_routing_locked(state, app, change, review);
    let (switched, outcome) = if change.selection.is_some()
        && wants_chatgpt_quit(user_choice.unwrap_or(false), app, taken_over)
    {
        chatgpt_app::around(false, action)?
    } else {
        (action()?, chatgpt_app::AroundOutcome::default())
    };
    let mut warnings = outcome.warnings;
    warnings.extend(switched.warnings);
    Ok((
        SwitchTierCommandResult::Switched {
            result: SwitchTierResult {
                provider_name,
                chatgpt_was_running: outcome.was_running,
                chatgpt_relaunched: outcome.relaunched,
                warnings,
            },
        },
        selected,
    ))
}

#[cfg(feature = "gui")]
pub async fn apply_reviewed_legacy_application_routing(
    app_handle: &tauri::AppHandle,
    app: AppType,
    change: ApplicationRoutingChange,
    user_choice: Option<bool>,
    review: LegacyRoutingReview,
) -> Result<SwitchTierCommandResult, AppError> {
    let state = app_handle.state::<AppState>().inner().clone();
    let event_app = app.clone();
    let (response, selected) = tauri::async_runtime::spawn_blocking(move || {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
        apply_reviewed_legacy_routing_locked(&state, &app, &change, user_choice, &review, None)
    })
    .await
    .map_err(|error| AppError::Message(error.to_string()))??;
    if let Some(id) = selected {
        publish_routing_selection(app_handle, &event_app, &id);
    }
    Ok(response)
}

pub(crate) fn query_routing_request_locked(
    state: &AppState,
    app: &AppType,
    request: &crate::mode::state::SaveRequest,
) -> Result<RoutingRequestResult, AppError> {
    Ok(RoutingRequestResult {
        request: request.clone(),
        status: crate::mode::controller::routing_request_status_for_db(&state.db, app, request)?,
    })
}

pub(crate) fn query_routing_request(
    state: &AppState,
    app: &AppType,
    request: crate::mode::state::SaveRequest,
) -> Result<RoutingRequestResult, AppError> {
    routing_request_scope(&request)?;
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    query_routing_request_locked(state, app, &request)
}

/// Caller owns the original switch lock; injected effects use the existing around seam.
#[cfg(feature = "gui")]
pub(crate) fn apply_requested_routing_locked(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    user_choice: Option<bool>,
    request: &crate::mode::state::SaveRequest,
    needs_attention: bool,
    around: impl FnOnce(
        &mut dyn FnMut() -> Result<crate::services::SwitchResult, AppError>,
    )
        -> Result<(crate::services::SwitchResult, chatgpt_app::AroundOutcome), AppError>,
) -> Result<(serde_json::Value, Option<String>), AppError> {
    apply_requested_routing_locked_admitted(
        state,
        app,
        change,
        user_choice,
        request,
        needs_attention,
        None,
        around,
    )
}

#[cfg(feature = "gui")]
#[allow(clippy::too_many_arguments)]
fn apply_requested_routing_locked_admitted(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    user_choice: Option<bool>,
    request: &crate::mode::state::SaveRequest,
    needs_attention: bool,
    admission: Option<RoutingAdmission>,
    around: impl FnOnce(
        &mut dyn FnMut() -> Result<crate::services::SwitchResult, AppError>,
    )
        -> Result<(crate::services::SwitchResult, chatgpt_app::AroundOutcome), AppError>,
) -> Result<(serde_json::Value, Option<String>), AppError> {
    routing_request_scope(request)?;
    if request.draft_digest != routing_request_digest(app, change)? {
        return Err(AppError::Conflict("mode.invalid_save_request".into()));
    }
    let original = query_routing_request_locked(state, app, request)?;
    if matches!(
        original.status.as_str(),
        "completed" | "pending" | "verificationRequired" | "conflict" | "discarded" | "abandoned"
    ) {
        return Ok::<_, AppError>((
            serde_json::json!({"status":"requestResolved", "result":original}),
            None,
        ));
    }
    ensure_legacy_change_allowed(state, app, change)?;
    // Another receipt is unknown, not proof of failure. Only a freshly reviewed
    // request pinned to these original owners may begin a new write.
    if request.revision != routing_request_revision(state, app, change)? {
        return Ok((
            serde_json::json!({"status":"requestResolved", "result":original}),
            None,
        ));
    }
    let taken_over = crate::services::provider::is_app_taken_over(state, app);
    if let Some(confirmation) = application_routing_confirmation_locked(
        state,
        app,
        change,
        user_choice,
        needs_attention,
        taken_over,
    )? {
        return Ok((
            serde_json::to_value(confirmation)
                .map_err(|error| AppError::Message(error.to_string()))?,
            None,
        ));
    }
    if let Some(admit) = admission {
        admit(state)?;
    }
    let result = if let Some(selection) = &change.selection {
        let mut action = || {
            // Quit may flush native files; the reviewed request must still match.
            if request.revision != routing_request_revision(state, app, change)? {
                return Err(AppError::Conflict("mode.verification_required".into()));
            }
            select_locked_with_request(
                state,
                app,
                selection,
                change.order.as_ref(),
                change.catalog_decision.as_ref(),
                Some(request),
            )
        };
        let (switched, outcome) =
            if wants_chatgpt_quit(user_choice.unwrap_or(false), app, taken_over) {
                around(&mut action)?
            } else {
                (action()?, chatgpt_app::AroundOutcome::default())
            };
        let mut warnings = outcome.warnings;
        warnings.extend(switched.warnings);
        let provider_name = state
            .db
            .get_provider_by_id(&selection.provider_id, app.as_str())?
            .map(|provider| provider.name)
            .unwrap_or_default();
        SwitchTierResult {
            provider_name,
            chatgpt_was_running: outcome.was_running,
            chatgpt_relaunched: outcome.relaunched,
            warnings,
        }
    } else {
        let order = change
            .order
            .as_ref()
            .ok_or_else(|| AppError::Config("No routing change was supplied".into()))?;
        apply_order_locked_with_request(state, app, order, Some(request))?;
        SwitchTierResult {
            provider_name: String::new(),
            chatgpt_was_running: false,
            chatgpt_relaunched: false,
            warnings: Vec::new(),
        }
    };
    Ok((
        serde_json::to_value(SwitchTierCommandResult::Switched { result })
            .map_err(|error| AppError::Message(error.to_string()))?,
        change
            .selection
            .as_ref()
            .map(|selection| selection.provider_id.clone()),
    ))
}

/// Request-aware GUI entry: deduplication and all external effects share one original lock.
#[cfg(feature = "gui")]
pub async fn apply_requested_application_routing(
    app_handle: &tauri::AppHandle,
    app: AppType,
    change: ApplicationRoutingChange,
    user_choice: Option<bool>,
    request: crate::mode::state::SaveRequest,
) -> Result<serde_json::Value, AppError> {
    routing_request_scope(&request)?;
    if request.draft_digest != routing_request_digest(&app, &change)? {
        return Err(AppError::Conflict("mode.invalid_save_request".into()));
    }
    let state = app_handle.state::<AppState>().inner().clone();
    let event_app = app.clone();
    let (response, selected) = tauri::async_runtime::spawn_blocking(move || {
        let _switch =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
        apply_requested_routing_locked(
            &state,
            &app,
            &change,
            user_choice,
            &request,
            chatgpt_app::needs_user_attention(),
            |action| chatgpt_app::around(false, action),
        )
    })
    .await
    .map_err(|error| AppError::Message(error.to_string()))??;
    if let Some(id) = selected {
        publish_routing_selection(app_handle, &event_app, &id);
    }
    Ok(response)
}

#[cfg(feature = "gui")]
pub(crate) fn publish_routing_selection(app_handle: &tauri::AppHandle, app: &AppType, id: &str) {
    crate::services::application_overview::record_successful_selection(
        &app_handle.state::<AppState>().db,
        app,
        id,
    );
    emit_provider_switched(app_handle, app, id);
    crate::tray::refresh_tray_menu(app_handle);
}

#[cfg(feature = "gui")]
pub(crate) type RoutingSelectionPublished = Box<dyn FnOnce(&AppType, &str) + Send>;

/// Setup reuses the existing lifecycle lease and original locked writers. The
/// worker owns the lease and acknowledgement: cancelling its IPC waiter cannot
/// release storage protection while a synchronous writer is still running.
#[cfg(feature = "gui")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_prepared_routing_with_admission(
    app_handle: &tauri::AppHandle,
    app: AppType,
    change: ApplicationRoutingChange,
    user_choice: Option<bool>,
    prepared: PreparedRoutingRequest,
    admission: RoutingAdmission,
    acknowledge: RoutingAcknowledgement,
) -> Result<serde_json::Value, AppError> {
    let state = app_handle.state::<AppState>().inner().clone();
    let handle = app_handle.clone();
    run_prepared_routing_owned(
        state,
        app,
        change,
        user_choice,
        prepared,
        admission,
        acknowledge,
        Box::new(move |app, id| publish_routing_selection(&handle, app, id)),
    )
    .await
}

/// The same worker serves IPC and tests; only the UI notification is injected.
/// Both original routing writers and the lifecycle/app leases remain real.
#[cfg(feature = "gui")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_prepared_routing_owned(
    state: AppState,
    app: AppType,
    change: ApplicationRoutingChange,
    user_choice: Option<bool>,
    prepared: PreparedRoutingRequest,
    admission: RoutingAdmission,
    acknowledge: RoutingAcknowledgement,
    selected: RoutingSelectionPublished,
) -> Result<serde_json::Value, AppError> {
    let sync_guard = crate::services::sync_protocol::sync_mutex().lock().await;
    crate::services::sync_protocol::ensure_upgrade_sync_admitted()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _sync_guard = sync_guard;
        let (result, selected_id, acknowledgement) = {
            let _switch =
                futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
            let outcome = match prepared {
                PreparedRoutingRequest::Native { request } => {
                    apply_requested_routing_locked_admitted(
                        &state,
                        &app,
                        &change,
                        user_choice,
                        &request,
                        chatgpt_app::needs_user_attention(),
                        Some(admission),
                        |action| chatgpt_app::around(false, action),
                    )
                }
                PreparedRoutingRequest::Legacy { review } => apply_reviewed_legacy_routing_locked(
                    &state,
                    &app,
                    &change,
                    user_choice,
                    &review,
                    Some(admission),
                )
                .and_then(|(result, selected)| {
                    serde_json::to_value(result)
                        .map(|value| (value, selected))
                        .map_err(|error| AppError::Message(error.to_string()))
                }),
            };
            let (result, selected_id) = match outcome {
                Ok((value, selected_id)) => (Ok(value), selected_id),
                Err(error) => (Err(error), None),
            };
            let acknowledgement = acknowledge(&state, &result);
            (result, selected_id, acknowledgement)
        };
        if let Some(id) = &selected_id {
            selected(&app, id);
        }
        acknowledgement?;
        result
    })
    .await
    .map_err(|error| AppError::Message(error.to_string()))?
}

/// Read-only admission and preview shared by the GUI and original service tests.
/// Returning a confirmation never prepares credentials or writes routing state.
pub(crate) fn application_routing_confirmation(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    user_choice: Option<bool>,
    needs_attention: bool,
    taken_over: bool,
) -> Result<Option<SwitchTierCommandResult>, AppError> {
    let _switch =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    application_routing_confirmation_locked(
        state,
        app,
        change,
        user_choice,
        needs_attention,
        taken_over,
    )
}

fn application_routing_confirmation_locked(
    state: &AppState,
    app: &AppType,
    change: &ApplicationRoutingChange,
    user_choice: Option<bool>,
    needs_attention: bool,
    taken_over: bool,
) -> Result<Option<SwitchTierCommandResult>, AppError> {
    if change.order.is_none() && change.selection.is_none() {
        return Err(AppError::Config("No routing change was supplied".into()));
    }
    let catalog_intent = change.catalog_decision.is_some() || change.catalog_preview_only;
    if catalog_intent && (*app != AppType::Codex || change.selection.is_none()) {
        return Err(AppError::Config(
            "codex.catalog_takeover_unavailable".into(),
        ));
    }
    if let Some(order) = &change.order {
        if !app.supports_local_proxy() {
            return Err(AppError::Config(
                "Application does not support routing order".into(),
            ));
        }
        crate::services::order_profiles::validate_order(
            &state.db,
            app.as_str(),
            &order.profile_name,
            &order.provider_ids,
        )?;
        if let Some(selection) = &change.selection {
            if !order.provider_ids.contains(&selection.provider_id) {
                return Err(AppError::Config(
                    "Selected provider is outside the applied order".into(),
                ));
            }
        }
    }
    if let Some(selection) = &change.selection {
        let previous = state
            .db
            .get_provider_by_id(&selection.provider_id, app.as_str())?
            .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
        crate::services::provider::validate_provider_selection(&state.db, app, &previous.id)?;
        let modern =
            app.supports_local_proxy() && crate::mode::operation::uses_upstream4_schema(&state.db)?;
        let provider = if modern {
            normalized_selection_provider(state, app, &previous, selection).map_err(|error| {
                if *app == AppType::Codex {
                    AppError::Config("codex.catalog_preview_unavailable".into())
                } else {
                    error
                }
            })?
        } else {
            if let Some(model) = selection.model.as_deref() {
                crate::relay::model_catalog::select_model(app, &previous, model)?;
            }
            previous
        };
        let catalog = if modern && *app == AppType::Codex {
            crate::mode::controller::preview_catalog_selection(
                &state.proxy_service,
                &provider,
                change.catalog_decision.as_ref(),
            )?
        } else {
            None
        };
        if catalog_intent && catalog.is_none() {
            return Err(AppError::Config(
                "codex.catalog_takeover_unavailable".into(),
            ));
        }
        let unclaimed = catalog
            .as_ref()
            .is_some_and(|catalog| catalog.ownership == "unclaimedExternal");
        let quit_required =
            should_request_switch_confirmation(app, user_choice, needs_attention, taken_over);
        if change.catalog_preview_only
            || (unclaimed && change.catalog_decision.is_none())
            || quit_required
        {
            return Ok(Some(SwitchTierCommandResult::ConfirmationRequired {
                target_name: match selection.model.as_deref() {
                    Some(model) => format!("{} · {model}", provider.name),
                    None => provider.name,
                },
                catalog: catalog.filter(|_| unclaimed || catalog_intent),
                quit_required,
            }));
        }
    }
    Ok(None)
}

#[cfg(feature = "gui")]
pub async fn apply_application_routing(
    app_handle: &tauri::AppHandle,
    app: AppType,
    change: ApplicationRoutingChange,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    let state = app_handle.state::<AppState>();
    if let Some(confirmation) = application_routing_confirmation(
        &state,
        &app,
        &change,
        user_choice,
        chatgpt_app::needs_user_attention(),
        crate::services::provider::is_app_taken_over(&state, &app),
    )? {
        return Ok(confirmation);
    }
    let selection = change.selection.as_ref();
    let result = if let Some(selection) = selection {
        switch_tier_impl(
            app_handle,
            &selection.provider_id,
            app.clone(),
            selection.model.as_deref(),
            user_choice.unwrap_or(false),
            change.order.clone(),
            change.catalog_decision.clone(),
        )
        .await?
    } else {
        let state = state.inner().clone();
        let order = change
            .order
            .clone()
            .ok_or_else(|| AppError::Config("No routing change was supplied".into()))?;
        tauri::async_runtime::spawn_blocking(move || apply_order_change(&state, &app, &order))
            .await
            .map_err(|e| AppError::Message(e.to_string()))??;
        SwitchTierResult {
            provider_name: String::new(),
            chatgpt_was_running: false,
            chatgpt_relaunched: false,
            warnings: Vec::new(),
        }
    };
    Ok(SwitchTierCommandResult::Switched { result })
}

/// The model picker and tray share the same application selection operation.
#[cfg(feature = "gui")]
pub(crate) async fn switch_tier_model_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: &str,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    apply_application_routing(
        app_handle,
        app_type,
        ApplicationRoutingChange {
            order: None,
            catalog_decision: None,
            catalog_preview_only: false,
            selection: Some(TierSelection {
                provider_id: provider_id.into(),
                model: Some(model.into()),
            }),
        },
        user_choice,
    )
    .await
}

pub(crate) fn should_request_switch_confirmation(
    app_type: &AppType,
    user_choice: Option<bool>,
    needs_attention: bool,
    taken_over: bool,
) -> bool {
    // 这里只裁决「退出正开着的 ChatGPT」；目录选择由独立的纯预览闸裁决。
    // 代管（takeover）态沿用热切换不退出的约定，目录确认不构成退出授权。
    !taken_over && matches!(app_type, AppType::Codex) && user_choice.is_none() && needs_attention
}

/// 代管态下即使传了 `quit_chatgpt=true`（确认弹窗时代留下的入参）也不退：
/// 判据与 `ProviderService::switch` 内部同源
///（[`crate::services::provider::is_app_taken_over`]），目录选择不改变退出授权。
fn wants_chatgpt_quit(quit_chatgpt: bool, app_type: &AppType, taken_over: bool) -> bool {
    !taken_over && should_quit_chatgpt(quit_chatgpt, app_type)
}

#[cfg(feature = "gui")]
pub(crate) async fn switch_tier_command(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    user_choice: Option<bool>,
) -> Result<SwitchTierCommandResult, AppError> {
    apply_application_routing(
        app_handle,
        app_type,
        ApplicationRoutingChange {
            order: None,
            catalog_decision: None,
            catalog_preview_only: false,
            selection: Some(TierSelection {
                provider_id: provider_id.into(),
                model: None,
            }),
        },
        user_choice,
    )
    .await
}

/// 这次切换要不要退 ChatGPT。
///
/// 两个条件都得成立：**用户同意了**（`user_agreed`，来自确认弹窗），
/// **且切的是 codex**（`app_type`）。
///
/// ## 为什么 codex 之外不退
///
/// 不是「其它平台不支持」，是**不需要** —— `chatgpt_app` 管的是 ChatGPT 桌面版
/// （bundle id `com.openai.codex`），它只读 `~/.codex`。切 claude/gemini 的档位时
/// 它压根不涉及，去退它是扰民：关掉用户正开着的、与本次切换毫无关系的对话。
///
/// 判据放后端而不是让前端决定：前端传的 `user_agreed` 表达「用户同意了退出」，
/// 而「这个平台要不要退」是后端事实 —— 两件事别混在一个布尔里。
pub(crate) fn should_quit_chatgpt(user_agreed: bool, app_type: &AppType) -> bool {
    user_agreed && matches!(app_type, AppType::Codex)
}

#[cfg(feature = "gui")]
async fn switch_tier_impl(
    app_handle: &tauri::AppHandle,
    provider_id: &str,
    app_type: AppType,
    model: Option<&str>,
    quit_chatgpt: bool,
    order: Option<RoutingOrder>,
    catalog_decision: Option<CatalogDecision>,
) -> Result<SwitchTierResult, AppError> {
    let quit_chatgpt = wants_chatgpt_quit(
        quit_chatgpt,
        &app_type,
        crate::services::provider::is_app_taken_over(&app_handle.state::<AppState>(), &app_type),
    );
    // `AppType` 没派生 Copy（上游结构，别为此改它），而下面 `ProviderService::list`
    // 会把它 move 掉 —— 事件那一步要用，先留一份。
    let app_type_for_event = app_type.clone();

    // 编排（退 → 切 → 重开，失败要把 ChatGPT 开回去）走 `chatgpt_app::around`。
    // 那套四分支处理原来内联在这里，抽出去是为了让**上游那条通用 provider 切换**
    // 也走同一份（`switch_provider` 里那处）—— 复制第二遍的必然结局是两份分叉，
    // 而分叉的表现是「从 LoongPort 页切没问题、从 provider 页切就静默用错配置」。
    //
    // `abort_on_unconfirmed_exit = false`：切档位只写 `config.toml`，退不掉也能照常切
    // （配置写进去就生效了），提示用户手动重启即可。这与「切回官方登录」相反 ——
    // 那条要删 `auth.json`，而 ChatGPT 退出时会重写它。
    let worker_handle = app_handle.clone();
    let worker_app = app_type.clone();
    let selection = TierSelection {
        provider_id: provider_id.into(),
        model: model.map(str::to_owned),
    };
    let (switched, chatgpt) = tauri::async_runtime::spawn_blocking(move || {
        let state = worker_handle.state::<AppState>();
        let switch_once = || {
            if catalog_decision.is_none() {
                return select_with_commit(&state, &worker_app, &selection, order.as_ref());
            }
            select_with_commit_with_catalog(
                &state,
                &worker_app,
                &selection,
                order.as_ref(),
                catalog_decision.as_ref(),
            )
        };
        if quit_chatgpt {
            chatgpt_app::around(false, switch_once)
        } else {
            Ok((switch_once()?, chatgpt_app::AroundOutcome::default()))
        }
    })
    .await
    .map_err(|e| AppError::Message(e.to_string()))??;

    let mut warnings = chatgpt.warnings;
    warnings.extend(switched.warnings);

    let provider_name = {
        let state = app_handle.state::<AppState>();
        ProviderService::list(&state, app_type)
            .ok()
            .and_then(|list| list.get(provider_id).map(|p| p.name.clone()))
            .unwrap_or_else(|| provider_id.to_string())
    };

    // 广播「当前供应商变了」—— **镜像方向**：切档位之后 provider 页那份列表
    // （react-query 的 `["providers", app]`）也陈旧了，而它的刷新靠的正是这个事件
    // （`App.tsx` 的 `providersApi.onSwitched`）。
    //
    // 两条切换路径都发，共用 `commands::provider::emit_provider_switched` 那一份实现 ——
    // payload 形状复制第二遍的必然结局是两份分叉（那边的文档写了完整理由）。
    crate::services::application_overview::record_successful_selection(
        &app_handle.state::<AppState>().db,
        &app_type_for_event,
        provider_id,
    );
    emit_provider_switched(app_handle, &app_type_for_event, provider_id);

    // 托盘也要跟上：这里不刷，用户从主界面切完档位、再看托盘标题还是旧的
    // （前端 relay 那条路不会替我们调 `update_tray_menu`）。放在 `switch_tier_impl`
    // 而不是各命令壳里 —— relay / vendor / 托盘三个入口一次全修，后来者免接。
    crate::tray::refresh_tray_menu(app_handle);

    Ok(SwitchTierResult {
        provider_name,
        chatgpt_was_running: chatgpt.was_running,
        chatgpt_relaunched: chatgpt.relaunched,
        warnings,
    })
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_original_selection() {
    tests::application_catalog_choice_wire_is_transient_and_camel_case();
    tests::switch_tier_command_result_wire_contract_is_camel_case();
    tests::chatgpt_quit_is_codex_only();
    tests::switch_confirmation_is_decided_before_mutating_the_target();
    tests::takeover_hot_switch_skips_confirmation_and_quit();
    tests::explicit_model_selection_replaces_old_preference_and_rolls_back_on_commit_failure();
}

#[cfg(any(test, feature = "test-hooks"))]
mod tests {
    use super::*;

    #[cfg_attr(test, test)]
    pub(super) fn application_catalog_choice_wire_is_transient_and_camel_case() {
        let change = serde_json::from_value::<ApplicationRoutingChange>(serde_json::json!({
            "selection": {"providerId": "synthetic-provider", "model": "synthetic-model"},
            "catalogDecision": {"choice": "keep", "revision": "synthetic-revision"},
            "catalogPreviewOnly": true
        }));
        assert!(
            change.is_ok(),
            "the existing application command must accept a request-local catalog choice: {change:?}"
        );
        let change = change.unwrap();
        assert!(change.catalog_preview_only);
        let selection = change.selection.unwrap();
        assert_eq!(selection.provider_id, "synthetic-provider");
        assert_eq!(selection.model.as_deref(), Some("synthetic-model"));
        let decision = change.catalog_decision.unwrap();
        assert_eq!(
            decision.choice,
            crate::services::provider::codex_direct::CatalogChoice::Keep
        );
        assert_eq!(decision.revision, "synthetic-revision");
        assert!(
            serde_json::from_value::<ApplicationRoutingChange>(serde_json::json!({
                "selection": {"providerId": "synthetic-provider"},
                "catalog_decision": {"choice": "keep", "revision": "synthetic-revision"}
            }))
            .is_err()
        );
        let original = serde_json::from_value::<ApplicationRoutingChange>(serde_json::json!({
            "selection": {"providerId": "synthetic-provider"}
        }))
        .unwrap();
        assert!(original.catalog_decision.is_none());
        assert!(!original.catalog_preview_only);
    }

    /// 契约闸：前端 TS 类型手写断言了这份 wire 形状（`src/lib/api/relay.ts` 的
    /// `SwitchTierCommandResult`），serde 的 enum 级 `rename_all` 只转变体名、
    /// 不转变体字段 —— 没有这条闸的话 casing 分叉编译期完全静默
    /// （2026-08-16 线上事故：`target_name` 蛇形下发，确认弹窗永不打开）。
    #[cfg_attr(test, test)]
    pub(super) fn switch_tier_command_result_wire_contract_is_camel_case() {
        let confirmation = serde_json::to_value(SwitchTierCommandResult::ConfirmationRequired {
            target_name: "站点 · 分组".into(),
            catalog: None,
            quit_required: true,
        })
        .unwrap();
        assert_eq!(confirmation["status"], "confirmationRequired");
        assert!(
            confirmation.get("targetName").is_some(),
            "变体字段必须驼峰下发：{confirmation}"
        );
        assert!(
            confirmation.get("target_name").is_none(),
            "蛇形键意味着前端读到 undefined：{confirmation}"
        );
        assert_eq!(confirmation["quitRequired"], true);
        assert!(confirmation.get("quit_required").is_none());
        assert!(confirmation.get("catalog").is_none());

        let catalog = serde_json::to_value(SwitchTierCommandResult::ConfirmationRequired {
            target_name: "synthetic-provider".into(),
            catalog: Some(CatalogPreview {
                ownership: "unclaimedExternal",
                choice: crate::services::provider::codex_direct::CatalogChoice::Keep,
                can_manage: true,
                can_restore: false,
                revision: "synthetic-revision".into(),
                before_pointer: Some("synthetic-external.json".into()),
                after_pointer: Some("synthetic-external.json".into()),
            }),
            quit_required: false,
        })
        .unwrap();
        assert_eq!(catalog["status"], "confirmationRequired");
        assert_eq!(catalog["quitRequired"], false);
        assert_eq!(catalog["catalog"]["choice"], "keep");
        assert_eq!(catalog["catalog"]["canManage"], true);
        assert_eq!(
            catalog["catalog"]["beforePointer"],
            "synthetic-external.json"
        );
        assert_eq!(
            catalog["catalog"]["afterPointer"],
            "synthetic-external.json"
        );
        assert!(catalog["catalog"].get("can_manage").is_none());

        let switched = serde_json::to_value(SwitchTierCommandResult::Switched {
            result: SwitchTierResult {
                provider_name: "p".into(),
                chatgpt_was_running: false,
                chatgpt_relaunched: false,
                warnings: vec![],
            },
        })
        .unwrap();
        assert_eq!(switched["status"], "switched");
        assert!(
            switched.get("providerName").is_some(),
            "flatten 的结构体字段同样是驼峰契约：{switched}"
        );
    }

    #[cfg_attr(test, test)]
    #[cfg_attr(test, serial_test::serial)]
    pub(super) fn explicit_model_selection_replaces_old_preference_and_rolls_back_on_commit_failure(
    ) {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        // This callback failure exercises the original legacy snapshot transaction.
        // Build its actual source schema, rather than changing a modern DB's version.
        let root = crate::config::get_app_config_dir();
        assert!(
            root.starts_with(_home.path()),
            "legacy fixture escaped its isolated test home"
        );
        let key_store = crate::secrets::testing::MemoryKeyStore::default();
        let session = crate::SecretSession::open(&root, &key_store, None).unwrap();
        let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
        crate::Database::create_tables_on_conn(&conn).unwrap();
        crate::Database::apply_upgrade_source_migrations_on_conn(&conn).unwrap();
        crate::database::loongport_schema::apply(&conn).unwrap();
        crate::database::vault::stamp(&conn, session.read().unwrap().encrypted().unwrap()).unwrap();
        session.complete_migration().unwrap();
        crate::settings::unlock_settings_for_test(session.clone()).unwrap();
        let db = std::sync::Arc::new(crate::Database::from_connection(conn, session));
        assert_eq!(
            crate::Database::get_user_version(&db.conn.lock().unwrap()).unwrap(),
            crate::database::UPSTREAM4_SOURCE_SCHEMA_VERSION
        );
        assert!(!crate::mode::operation::uses_upstream4_schema(&db).unwrap());
        let mut local_settings = crate::settings::get_settings();
        local_settings.claude_config_dir =
            Some(_home.path().join(".claude").to_string_lossy().into_owned());
        crate::settings::update_settings(local_settings).unwrap();
        let settings = serde_json::json!({"env":{"ANTHROPIC_BASE_URL":"https://relay.example", "ANTHROPIC_AUTH_TOKEN":"example-key","ANTHROPIC_MODEL":"model-one","CLAUDE_CODE_SUBAGENT_MODEL":"worker"},"modelCatalog":{"models":[{"model":"model-one"},{"model":"model-two"}]}});
        let provider = crate::provider::Provider::with_id(
            "custom".into(),
            "Custom".into(),
            settings.clone(),
            None,
        );
        db.save_provider("claude", &provider).unwrap();
        db.set_current_provider("claude", "custom").unwrap();
        crate::services::provider::write_standalone_live_snapshot(&AppType::Claude, &provider)
            .unwrap();
        crate::proxy::auto_strategy::set_model_pref(&db, "claude", Some("model-one")).unwrap();
        let state = AppState::new(db.clone()).unwrap();
        assert!(!crate::mode::operation::uses_upstream4_schema(&db).unwrap());
        let selection = TierSelection {
            provider_id: "custom".into(),
            model: Some("model-two".into()),
        };
        select_legacy_with_commit(&state, &AppType::Claude, &selection, || Ok(())).unwrap();
        assert_eq!(
            crate::proxy::application_routing::effective_model(&db, "claude").as_deref(),
            Some("model-two")
        );
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(&db, "claude")
                .unwrap()
                .as_deref(),
            Some("model-two")
        );
        let after = db.get_provider_by_id("custom", "claude").unwrap().unwrap();
        assert_eq!(after.settings_config["env"]["ANTHROPIC_MODEL"], "model-two");
        assert_eq!(
            after.settings_config["env"]["CLAUDE_CODE_SUBAGENT_MODEL"],
            "worker"
        );
        let live = std::fs::read(crate::config::get_claude_settings_path()).unwrap();
        let applied: serde_json::Value = serde_json::from_slice(&live).unwrap();
        assert_eq!(applied["env"]["ANTHROPIC_MODEL"], "model-two");
        assert_eq!(applied["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "worker");
        let error = select_legacy_with_commit(
            &state,
            &AppType::Claude,
            &TierSelection {
                model: Some("model-one".into()),
                ..selection
            },
            || Err(AppError::Config("commit rejected".into())),
        )
        .unwrap_err();
        assert!(error.to_string().contains("commit rejected"));
        assert_eq!(
            crate::proxy::auto_strategy::get_model_pref_checked(&db, "claude")
                .unwrap()
                .as_deref(),
            Some("model-two")
        );
        assert_eq!(
            db.get_provider_by_id("custom", "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            after.settings_config
        );
        assert_eq!(
            crate::proxy::application_routing::effective_model(&db, "claude").as_deref(),
            Some("model-two")
        );
        assert_eq!(
            std::fs::read(crate::config::get_claude_settings_path()).unwrap(),
            live
        );
        let mut edited = db.get_provider_by_id("custom", "claude").unwrap().unwrap();
        edited.settings_config["env"]["ANTHROPIC_MODEL"] = serde_json::json!("model-one");
        ProviderService::update(&state, AppType::Claude, None, edited).unwrap();
        assert!(crate::proxy::auto_strategy::get_model_pref(&db, "claude").is_none());
        let live: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::config::get_claude_settings_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(live["env"]["ANTHROPIC_MODEL"], "model-one");
        assert_eq!(live["env"]["CLAUDE_CODE_SUBAGENT_MODEL"], "worker");
    }

    #[cfg_attr(test, test)]
    pub(super) fn chatgpt_quit_is_codex_only() {
        // 用户同意 + codex ⇒ 退。
        assert!(should_quit_chatgpt(true, &AppType::Codex));
        // 用户同意但切的是别的平台 ⇒ **不退**。ChatGPT 桌面版只读 ~/.codex，
        // 切 claude/gemini 档位去关它纯属扰民（关掉用户正开着的、与本次切换无关的对话）。
        assert!(!should_quit_chatgpt(true, &AppType::Claude));
        assert!(!should_quit_chatgpt(true, &AppType::Gemini));
        // 用户没同意 ⇒ 一律不退，哪怕是 codex。
        assert!(!should_quit_chatgpt(false, &AppType::Codex));
    }

    #[cfg_attr(test, test)]
    pub(super) fn switch_confirmation_is_decided_before_mutating_the_target() {
        assert!(should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true,
            false
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Claude,
            None,
            true,
            false
        ));
        assert!(!should_request_switch_confirmation(
            &AppType::Codex,
            Some(false),
            true,
            false
        ));
    }

    /// 代管（热切换）态：确认弹窗与退/重开一并跳过 —— 热切换不写 CLI 配置，
    /// 退了重开 codex 也不会加载任何新东西，两样都是纯打断。
    /// 回归背景：v6.26.0 前代管态切档/换模型仍弹「退出并切换」并真的退重开。
    #[cfg_attr(test, test)]
    pub(super) fn takeover_hot_switch_skips_confirmation_and_quit() {
        // codex + 用户未选 + ChatGPT 在跑：非代管 ⇒ 弹确认（旧状照旧）。
        assert!(should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true,
            false
        ));
        // 同样条件下代管 ⇒ 不弹。
        assert!(!should_request_switch_confirmation(
            &AppType::Codex,
            None,
            true,
            true
        ));
        // 用户已确认要退（quit=true），代管 ⇒ 也不退。
        assert!(!wants_chatgpt_quit(true, &AppType::Codex, true));
        // 非代管照旧退；非 codex 照旧不退。
        assert!(wants_chatgpt_quit(true, &AppType::Codex, false));
        assert!(!wants_chatgpt_quit(true, &AppType::Claude, false));
        assert!(!wants_chatgpt_quit(false, &AppType::Codex, false));
    }
}

/// Existing routing metadata DAOs retain their own atomic updates. This is only
/// app-scoped admission, never an implicit operation recovery or a second journal.
fn with_metadata_write<T>(
    state: &AppState,
    app: &AppType,
    write: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app.as_str()));
    crate::mode::operation::admit_metadata_write(&state.proxy_service, app)?;
    write()
}

pub(crate) fn save_order_profile(
    state: &AppState,
    app: &AppType,
    name: &str,
    ids: &[String],
) -> Result<(), AppError> {
    with_metadata_write(state, app, || {
        crate::services::order_profiles::save(&state.db, app.as_str(), name, ids)
    })
}
pub(crate) fn rename_order_profile(
    state: &AppState,
    app: &AppType,
    from: &str,
    to: &str,
) -> Result<(), AppError> {
    with_metadata_write(state, app, || {
        crate::services::order_profiles::rename(&state.db, app.as_str(), from, to)
    })
}
pub(crate) fn remove_order_profile(
    state: &AppState,
    app: &AppType,
    name: &str,
) -> Result<(), AppError> {
    with_metadata_write(state, app, || {
        crate::services::order_profiles::remove(&state.db, app.as_str(), name)
    })
}
pub(crate) fn import_order_profiles(
    state: &AppState,
    app: &AppType,
    profiles: Vec<crate::services::order_profiles::OrderProfile>,
) -> Result<usize, AppError> {
    with_metadata_write(state, app, || {
        crate::services::order_profiles::import(&state.db, app.as_str(), profiles)
    })
}
pub(crate) async fn set_tier_blocked(
    state: &AppState,
    app: &AppType,
    id: &str,
    blocked: bool,
) -> Result<(), AppError> {
    let _guard = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    crate::mode::operation::admit_metadata_write(&state.proxy_service, app)?;
    crate::proxy::application_routing::set_tier_blocked(&state.db, app.as_str(), id, blocked)
}

/// Compatibility alias for the retired manual-mode command; no policy routing.
pub(crate) async fn initialize_manual_routing(
    state: &AppState,
    app: &AppType,
) -> Result<(), AppError> {
    let _guard = state.proxy_service.lock_switch_for_app(app.as_str()).await;
    crate::services::shared_routing::ensure_legacy_policy_edit_allowed(&state.db, app)?;
    crate::mode::operation::admit_metadata_write(&state.proxy_service, app)?;
    crate::proxy::application_routing::migrate(&state.db, app.as_str())
}

#[cfg(test)]
#[path = "application_selection_legacy_tests.rs"]
mod legacy_routing_tests;

#[cfg(test)]
mod relay_connection_owner_tests {
    use super::*;
    #[cfg(feature = "gui")]
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    #[cfg(feature = "gui")]
    fn image_fixture() -> (
        crate::secrets::testing::TestHome,
        AppState,
        ApplicationRoutingChange,
        PreparedRoutingRequest,
    ) {
        let home = crate::secrets::testing::TestHome::new().unwrap();
        let db = Arc::new(crate::Database::memory().unwrap());
        crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
        let state = AppState::new(db.clone()).unwrap();
        let provider = crate::provider::Provider::with_id(
            "setup-owned".into(),
            "Owned fixture".into(),
            serde_json::json!({"auth":{"OPENAI_API_KEY":"fixture-only"},"config":"model = \"gpt-image-1\""}),
            None,
        );
        db.save_provider("codex-image", &provider).unwrap();
        let change = ApplicationRoutingChange {
            order: None,
            selection: Some(TierSelection {
                provider_id: provider.id,
                model: None,
            }),
            catalog_decision: None,
            catalog_preview_only: false,
        };
        let prepared =
            prepare_routing_strategy(&state, &AppType::CodexImage, &change, "owned-fixture")
                .unwrap();
        (home, state, change, prepared)
    }
    #[cfg(feature = "gui")]
    async fn wait_for_sync_owner() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if crate::services::sync_protocol::sync_mutex()
                    .try_lock()
                    .is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("routing worker never acquired the lifecycle lease");
    }
    #[cfg(feature = "gui")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn setup_admission_runs_after_app_wait_and_before_original_image_write() {
        let (_home, state, change, prepared) = image_fixture();
        let lock = state.proxy_service.lock_switch_for_app("codex-image").await;
        let valid = Arc::new(AtomicBool::new(true));
        let checked = valid.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let task = tokio::spawn(run_prepared_routing_owned(
            state.clone(),
            AppType::CodexImage,
            change,
            Some(false),
            prepared,
            Box::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                if checked.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(AppError::Config(
                        "fixture authority expired while queued".into(),
                    ))
                }
            }),
            Box::new(|_, result| {
                assert!(result.is_err());
                Ok(())
            }),
            Box::new(|_, _| panic!("rejected request published a selection")),
        ));
        wait_for_sync_owner().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        valid.store(false, Ordering::SeqCst);
        drop(lock);
        assert!(task.await.unwrap().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(state
            .db
            .get_current_provider("codex-image")
            .unwrap()
            .is_none());
    }
    #[cfg(feature = "gui")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn setup_admission_runs_after_lifecycle_wait_not_before_it() {
        let (_home, state, change, prepared) = image_fixture();
        let lease = crate::services::sync_protocol::sync_mutex().lock().await;
        let valid = Arc::new(AtomicBool::new(true));
        let checked = valid.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let task = run_prepared_routing_owned(
            state.clone(),
            AppType::CodexImage,
            change,
            Some(false),
            prepared,
            Box::new(move |_| {
                count.fetch_add(1, Ordering::SeqCst);
                if checked.load(Ordering::SeqCst) {
                    Ok(())
                } else {
                    Err(AppError::Config(
                        "fixture authority expired while lifecycle was held".into(),
                    ))
                }
            }),
            Box::new(|_, result| {
                assert!(result.is_err());
                Ok(())
            }),
            Box::new(|_, _| panic!("rejected request published a selection")),
        );
        futures::pin_mut!(task);
        assert!(futures::poll!(task.as_mut()).is_pending());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        valid.store(false, Ordering::SeqCst);
        drop(lease);
        assert!(task.await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(state
            .db
            .get_current_provider("codex-image")
            .unwrap()
            .is_none());
    }
    // ACK is a condition, not a latency guarantee for the original OS writer.
    #[cfg(feature = "gui")]
    async fn await_setup_ack_entered(
        entered: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        entered.await
    }

    #[cfg(feature = "gui")]
    #[tokio::test(start_paused = true)]
    async fn setup_ack_wait_tracks_the_condition_not_elapsed_scheduling_time() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let waiting = await_setup_ack_entered(entered_rx);
        futures::pin_mut!(waiting);
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        entered_tx.send(()).unwrap();
        waiting.await.unwrap();
    }

    #[cfg(feature = "gui")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn cancelled_setup_waiter_keeps_lifecycle_lease_until_original_ack_finishes() {
        let (_home, state, change, prepared) = image_fixture();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let task = tokio::spawn(run_prepared_routing_owned(
            state.clone(),
            AppType::CodexImage,
            change,
            Some(false),
            prepared,
            Box::new(|_| Ok(())),
            Box::new(move |state, result| {
                assert!(result.is_ok());
                assert_eq!(
                    state.db.get_current_provider("codex-image")?.as_deref(),
                    Some("setup-owned")
                );
                count.fetch_add(1, Ordering::SeqCst);
                if entered_tx.send(()).is_err() {
                    return Err(AppError::Message("test ACK observer was dropped".into()));
                }
                // Only the caller's release (or sender drop on failure) ends
                // this ACK. Scheduler delay must never release the lease.
                release_rx
                    .recv()
                    .map_err(|_| AppError::Message("test ACK release sender was dropped".into()))
            }),
            Box::new(|_, _| {}),
        ));
        if await_setup_ack_entered(entered_rx).await.is_err() {
            // Join a failed original worker before TestHome can be destroyed.
            panic!("original worker ended before ACK: {:?}", task.await);
        }
        task.abort();
        let cancelled = task.await;
        let held_during_ack = crate::services::sync_protocol::sync_mutex()
            .try_lock()
            .is_err();
        let mut app_wait = Box::pin(state.proxy_service.lock_switch_for_app("codex-image"));
        let app_held_during_ack = futures::poll!(app_wait.as_mut()).is_pending();
        drop(app_wait);
        let released = release_tx.send(());
        // Acquiring the original owner proves the real worker finished ACK.
        // The existing CI job watchdog bounds deadlocks; there is deliberately
        // no per-phase performance deadline for OS probing or thread scheduling.
        let lease = crate::services::sync_protocol::sync_mutex().lock().await;
        drop(lease);
        // Fail only after releasing and joining the worker's protected phase.
        assert!(cancelled.unwrap_err().is_cancelled());
        assert!(
            held_during_ack,
            "cancelled IPC must not unblock restore while ACK runs"
        );
        assert!(
            app_held_during_ack,
            "original application owner must remain held through ACK"
        );
        released.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            state
                .db
                .get_current_provider("codex-image")
                .unwrap()
                .as_deref(),
            Some("setup-owned")
        );
    }
    #[test]
    #[serial_test::serial]
    fn image_review_uses_existing_selection_owner_and_rejects_stale_and_other_nonproxy() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let db = std::sync::Arc::new(crate::Database::memory().unwrap());
        crate::settings::unlock_settings_for_test(db.secrets.clone()).unwrap();
        let state = AppState::new(db.clone()).unwrap();
        let mut provider = crate::provider::Provider::with_id(
            "setup-image".into(),
            "Setup image".into(),
            serde_json::json!({"auth":{"OPENAI_API_KEY":"fixture-only"},"config":"base_url = \"https://fixture.invalid/v1\"\nmodel = \"gpt-image-1\""}),
            None,
        );
        db.save_provider("codex-image", &provider).unwrap();
        let change = ApplicationRoutingChange {
            order: None,
            selection: Some(TierSelection {
                provider_id: provider.id.clone(),
                model: None,
            }),
            catalog_decision: None,
            catalog_preview_only: false,
        };
        let prepared =
            prepare_routing_strategy(&state, &AppType::CodexImage, &change, "request-image")
                .unwrap();
        let PreparedRoutingRequest::Legacy { review } = prepared else {
            panic!("Image cannot manufacture a native request")
        };
        assert!(db.get_current_provider("codex-image").unwrap().is_none());
        let lock =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app("codex-image"));
        provider.name = "Edited after review".into();
        db.save_provider("codex-image", &provider).unwrap();
        assert!(
            apply_legacy_routing_locked(&state, &AppType::CodexImage, &change, &review).is_err()
        );
        assert!(db.get_current_provider("codex-image").unwrap().is_none());
        drop(lock);
        let PreparedRoutingRequest::Legacy { review } =
            prepare_routing_strategy(&state, &AppType::CodexImage, &change, "request-image-2")
                .unwrap()
        else {
            panic!("Image must retain compatibility review")
        };
        let lock =
            futures::executor::block_on(state.proxy_service.lock_switch_for_app("codex-image"));
        apply_legacy_routing_locked(&state, &AppType::CodexImage, &change, &review).unwrap();
        assert_eq!(
            db.get_current_provider("codex-image").unwrap().as_deref(),
            Some("setup-image")
        );
        assert!(
            apply_legacy_routing_locked(&state, &AppType::CodexImage, &change, &review).is_err()
        );
        drop(lock);
        for app in [
            AppType::OpenCode,
            AppType::OpenClaw,
            AppType::Hermes,
            AppType::Mcode,
            AppType::Pi,
            AppType::ClaudeDesktop,
        ] {
            assert!(prepare_routing_strategy(&state, &app, &change, "unsupported").is_err());
        }
        // The old public API still delegates to the same existing Image owner.
        ProviderService::switch(&state, AppType::CodexImage, &provider.id).unwrap();
    }
}
