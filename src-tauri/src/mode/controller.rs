//! Upstream v4.0.2 route-mode controller, using LoongPort's original service,
//! encrypted operation and current-pointer owners. Stack remains unadmitted.
use super::{
    contract, current,
    operation::{self, AppWrite, RecoveryOutcome},
    state::{self, op, Contract, Mode, ModeState, PendingTarget, SavedRow},
};
use crate::live::engine::{DeviceStore, LiveFile};
use crate::live::project::{
    claude::{
        direct_patch, proxy_projection, ClaudeProjection, ProxyAuth, PROXY_TOKEN_PLACEHOLDER,
    },
    gemini::GeminiProjection,
    grok::GrokProjection,
};
use crate::proxy::providers::codex_oauth_auth::CodexLiveAuthGuard;
use crate::services::provider::{claude_direct, codex_direct, gemini_direct, grok_direct};
use crate::{
    app_config::AppType, database::Database, error::AppError, provider::Provider,
    services::ProxyService,
};

pub(crate) const PROXY_APPS: [AppType; 4] = [
    AppType::Claude,
    AppType::Codex,
    AppType::Gemini,
    AppType::GrokBuild,
];
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn invalid() -> AppError {
    AppError::Config("mode.verification_required".into())
}

fn mode(service: &ProxyService, app: &AppType) -> Result<ModeState, AppError> {
    let write = AppWrite::begin_mode(service, app)?;
    current::validate_known_mode(&write.store, &write.vault, app)
}
fn provider(service: &ProxyService, app: &AppType, id: &str) -> Result<Provider, AppError> {
    service
        .database()
        .get_provider_by_id(id, app.as_str())?
        .ok_or_else(invalid)
}
fn admit(service: &ProxyService, app: &AppType, target: &Provider) -> Result<(), AppError> {
    if !app.supports_local_proxy() {
        return Err(invalid());
    }
    if crate::proxy::application_routing::blocked_tier_ids_checked(
        service.database(),
        app.as_str(),
    )?
    .contains(&target.id)
    {
        return Err(AppError::Config("routing.blocked".into()));
    }
    crate::services::provider::validate_provider_selection(service.database(), app, &target.id)?;
    if !crate::proxy::provider_router::provider_supports_proxy_routing(app.as_str(), target) {
        return Err(AppError::Config("mode.unsupported_route".into()));
    }
    Ok(())
}

enum LiveNow {
    Direct(Option<Provider>),
    Proxy {
        contract: Option<Contract>,
        route: Option<Provider>,
    },
}
impl LiveNow {
    fn of(service: &ProxyService, app: &AppType, mode: &ModeState) -> Result<Self, AppError> {
        if mode.attached {
            let route = mode
                .proxy_route
                .as_deref()
                .map(|id| service.database().get_provider_by_id(id, app.as_str()))
                .transpose()?
                .flatten();
            Ok(Self::Proxy {
                contract: mode.contract.clone(),
                route,
            })
        } else {
            let direct = current::direct_provider(service.database(), app)?
                .map(|provider| service.database().resolve_vendor_auth(app, &provider))
                .transpose()?;
            Ok(Self::Direct(direct))
        }
    }
    fn claude_owner(&self) -> Option<ClaudeProjection> {
        match self {
            Self::Direct(p) => p.as_ref().map(|p| ClaudeProjection::of(&p.settings_config)),
            Self::Proxy {
                contract: Some(contract),
                ..
            } => Some(ClaudeProjection {
                exclusive: contract.exclusive.clone(),
                ..Default::default()
            }),
            Self::Proxy { route, .. } => route
                .as_ref()
                .map(|p| ClaudeProjection::of(&p.settings_config)),
        }
    }
    fn codex_owner(&self) -> codex_direct::Owner<'_> {
        match self {
            Self::Direct(p) => p
                .as_ref()
                .map(codex_direct::Owner::Provider)
                .unwrap_or(codex_direct::Owner::None),
            Self::Proxy {
                contract: Some(contract),
                route,
            } => codex_direct::Owner::Contract {
                contract,
                route: route.as_ref(),
            },
            Self::Proxy { route, .. } => route
                .as_ref()
                .map(codex_direct::Owner::Provider)
                .unwrap_or(codex_direct::Owner::None),
        }
    }
    fn direct_owner(&self) -> Option<&Provider> {
        match self {
            Self::Direct(p) => p.as_ref(),
            _ => None,
        }
    }
}

fn claude_proxy_projection(route: &Provider, url: &str, key: &str) -> ClaudeProjection {
    let auth = if route.uses_managed_account_auth() {
        ProxyAuth::Managed {
            auth_token: !route.is_github_copilot() || !route.claude_uses_api_key_field(),
        }
    } else {
        ProxyAuth::FollowRow
    };
    let mut projection = proxy_projection(
        &ClaudeProjection::of(&route.settings_config),
        url,
        auth,
        None,
    );
    for value in projection.env.values_mut() {
        if value.as_str() == Some(PROXY_TOKEN_PLACEHOLDER) {
            *value = serde_json::Value::String(key.to_string());
        }
    }
    projection
}

fn gemini_proxy_projection(route: &Provider, url: &str, key: &str) -> GeminiProjection {
    GeminiProjection::proxy_contract(
        &GeminiProjection::of(&route.settings_config, false),
        url,
        key,
    )
}

fn grok_proxy_projection(
    route: &Provider,
    url: &str,
    key: &str,
) -> Result<GrokProjection, AppError> {
    GrokProjection::proxy_contract(
        &grok_direct::projection(route)?,
        &format!("{}/grokbuild/v1", url.trim_end_matches('/')),
        key,
    )
    .map_err(|error| AppError::Config(error.to_string()))
}

fn expected_proxy_contract(
    service: &ProxyService,
    app: &AppType,
    route: &Provider,
    live: &LiveNow,
    url: &str,
) -> Result<Contract, AppError> {
    Ok(match app {
        AppType::Claude => contract::claude(&claude_proxy_projection(
            route,
            url,
            &crate::proxy::client_access::managed::key(service.database(), "claude")?,
        )),
        AppType::Gemini => contract::gemini(&gemini_proxy_projection(
            route,
            url,
            &crate::proxy::client_access::managed::key(service.database(), "gemini")?,
        )),
        AppType::GrokBuild => contract::grok(&grok_proxy_projection(
            route,
            url,
            &crate::proxy::client_access::managed::key(service.database(), "grokbuild")?,
        )?),
        AppType::Codex => codex_direct::planned_proxy_contract(
            service.database(),
            &live.codex_owner(),
            route,
            &format!("{}/v1", url.trim_end_matches('/')),
        )?,
        _ => return Err(invalid()),
    })
}

fn write_proxy(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    route: &Provider,
    live: &LiveNow,
    target: PendingTarget,
    url: &str,
) -> Result<bool, AppError> {
    write_proxy_with_catalog(service, app, operation, route, live, target, url, None)
}

#[allow(clippy::too_many_arguments)]
fn write_proxy_with_catalog(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    route: &Provider,
    live: &LiveNow,
    mut target: PendingTarget,
    url: &str,
    catalog: Option<&codex_direct::CatalogDecision>,
) -> Result<bool, AppError> {
    service
        .database()
        .provision_managed_client_credential(app.as_str())?;
    let mut next = target.state.clone().ok_or_else(invalid)?;
    match app {
        AppType::Codex => {
            return codex_direct::apply_mode_with_catalog(
                service,
                &live.codex_owner(),
                codex_direct::Target::Proxy {
                    route,
                    base_url: &format!("{}/v1", url.trim_end_matches('/')),
                },
                operation,
                target,
                catalog,
            );
        }
        AppType::Claude => {
            let projection = claude_proxy_projection(
                route,
                url,
                &crate::proxy::client_access::managed::key(service.database(), "claude")?,
            );
            next.contract = Some(contract::claude(&projection));
            target.state = Some(next);
            let patch = direct_patch(live.claude_owner().as_ref(), &projection);
            let write = AppWrite::begin_mode(service, app)?;
            claude_direct::run_with_write(&write, operation, Some(&patch), target)?;
        }
        AppType::Gemini => {
            let projection = gemini_proxy_projection(
                route,
                url,
                &crate::proxy::client_access::managed::key(service.database(), "gemini")?,
            );
            next.contract = Some(contract::gemini(&projection));
            target.state = Some(next);
            let write = AppWrite::begin_mode(service, app)?;
            gemini_direct::run_with_write(&write, operation, Some(&projection), target)?;
        }
        AppType::GrokBuild => {
            let projection = grok_proxy_projection(
                route,
                url,
                &crate::proxy::client_access::managed::key(service.database(), "grokbuild")?,
            )?;
            next.contract = Some(contract::grok(&projection));
            target.state = Some(next);
            let write = AppWrite::begin_mode(service, app)?;
            grok_direct::run_with_write(
                &write,
                operation,
                live.direct_owner(),
                Some(&projection),
                target,
                None,
            )?;
        }
        _ => return Err(invalid()),
    }
    Ok(false)
}

fn write_direct(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    direct: Option<&Provider>,
    live: &LiveNow,
    target: PendingTarget,
) -> Result<bool, AppError> {
    write_direct_with_catalog(service, app, operation, direct, live, target, None)
}

fn write_direct_with_catalog(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    direct: Option<&Provider>,
    live: &LiveNow,
    target: PendingTarget,
    catalog: Option<&codex_direct::CatalogDecision>,
) -> Result<bool, AppError> {
    if let Some(provider) = direct {
        crate::services::provider::managed_inference::reject_direct(provider)?;
    }
    if *app == AppType::Codex {
        if catalog.is_none() {
            return codex_direct::apply_mode(
                service,
                &live.codex_owner(),
                codex_direct::Target::Direct(direct),
                operation,
                target,
            );
        }
        return codex_direct::apply_mode_with_catalog(
            service,
            &live.codex_owner(),
            codex_direct::Target::Direct(direct),
            operation,
            target,
            catalog,
        );
    }
    let resolved = direct
        .map(|p| service.database().resolve_vendor_auth(app, p))
        .transpose()?;
    let direct = resolved.as_ref();
    let write = AppWrite::begin_mode(service, app)?;
    match app {
        AppType::Claude => {
            let projection = direct
                .map(|p| ClaudeProjection::of(&p.settings_config))
                .unwrap_or_default();
            let patch = direct_patch(live.claude_owner().as_ref(), &projection);
            claude_direct::run_with_write(&write, operation, Some(&patch), target)?;
        }
        AppType::Gemini => {
            let projection = direct
                .map(gemini_direct::projection)
                .transpose()?
                .unwrap_or_else(GeminiProjection::empty);
            gemini_direct::run_with_write(&write, operation, Some(&projection), target)?;
        }
        AppType::GrokBuild => {
            let projection = direct
                .map(grok_direct::projection)
                .transpose()?
                .unwrap_or(GrokProjection { table: None });
            grok_direct::run_with_write(
                &write,
                operation,
                live.direct_owner(),
                Some(&projection),
                target,
                None,
            )?;
        }
        _ => return Err(invalid()),
    }
    Ok(false)
}

fn write_target_only(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    target: PendingTarget,
) -> Result<(), AppError> {
    if *app == AppType::Codex {
        return codex_direct::apply_target_only(service, operation, target);
    }
    AppWrite::begin_mode(service, app)?
        .run(operation, &[], target)
        .map(|_| ())
}

/// Caller holds the existing switch lock. This reads original owners only; no recovery.
pub(crate) fn routing_request_revision_for_service(
    service: &crate::services::ProxyService,
    app: &AppType,
    selected_provider: Option<&str>,
) -> Result<String, AppError> {
    // Original direct pointer owners can change while native bytes stay identical.
    // Resolve before pinning the vault; the caller already owns the switch lock.
    let direct_pointer = crate::mode::current::local_direct_pointer(app)?;
    let current_pointer = service.database().get_current_provider(app.as_str())?;
    let write = crate::mode::operation::AppWrite::begin_mode(service, app)?;
    let live = crate::mode::state::load_app(&write.store, &write.vault, app.as_str())?;
    let selected = selected_provider
        .map(|id| {
            let provider = service
                .database()
                .get_provider_by_id_with_vault(
                    id,
                    app.as_str(),
                    service.database().secret_session(),
                    &write.vault,
                )?
                .ok_or_else(|| AppError::Config("Provider does not exist".into()))?;
            crate::database::Database::provider_update_digest(&provider)
        })
        .transpose()?;
    let files = crate::mode::controller::files(app)?
        .into_iter()
        .map(|file| {
            crate::live::engine::read_current(&file.path)
                .map(|bytes| crate::live::engine::digest(bytes.as_deref()))
                .map_err(AppError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let bytes = crate::config::serialize_json_bytes(&serde_json::json!([
        live.apps.get(app.as_str()),
        direct_pointer,
        current_pointer,
        crate::database::order_profiles::snapshot(service.database(), app.as_str())?,
        crate::proxy::auto_strategy::get_model_pref_checked(service.database(), app.as_str())?,
        selected,
        files,
    ]))?;
    Ok(crate::live::engine::sha256_hex(&bytes))
}

pub(crate) fn routing_request_status_for_db(
    db: &crate::database::Database,
    app: &AppType,
    request: &crate::mode::state::SaveRequest,
) -> Result<String, AppError> {
    request.validate()?;
    if request.scope != state::SaveScope::ApplicationRouting {
        return Err(AppError::Config("mode.invalid_save_request".into()));
    }
    let vault = db.secret_session().read()?;
    let live = crate::mode::state::load_app(
        &crate::live::engine::DeviceStore::for_device(),
        &vault,
        app.as_str(),
    )?;
    let status = live
        .apps
        .get(app.as_str())
        .map(|entry| crate::mode::operation::saved_request_status(app.as_str(), entry, request))
        .transpose()?
        .unwrap_or("notRecorded");
    Ok(status.into())
}

/// One explicit empty-application ENTER, using the original app receipt.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitialProxyRoute {
    pub provider_id: String,
    pub request: crate::mode::state::SaveRequest,
}

pub(crate) fn proxy_enter_digest(app: &str, provider_id: &str) -> Result<String, AppError> {
    if !matches!(app, "claude" | "codex") || provider_id.trim().is_empty() {
        return Err(AppError::Config("mode.invalid_save_request".into()));
    }
    let bytes = crate::config::serialize_json_bytes(&serde_json::json!([
        "explicitProxyEnter-v1",
        app,
        provider_id
    ]))?;
    Ok(crate::live::engine::sha256_hex(&bytes))
}

/// The caller holds the original app lock. Preparation records no intent or receipt.
pub(crate) fn prepare_proxy_enter_locked(
    service: &crate::services::ProxyService,
    app: &AppType,
    provider_id: &str,
    id: &str,
) -> Result<crate::mode::state::SaveRequest, AppError> {
    admit_initial_route(service, app, provider_id)?;
    let request = crate::mode::state::SaveRequest {
        id: id.into(),
        scope: crate::mode::state::SaveScope::ApplicationRouting,
        provider_id: None,
        draft_digest: proxy_enter_digest(app.as_str(), provider_id)?,
        revision: routing_request_revision_for_service(service, app, Some(provider_id))?,
    };
    request.validate()?;
    Ok(request)
}

/// Returns true only for this exact completed receipt. Unknown prior receipts
/// can begin only with the current original owner revision; pending cannot replay.
pub(crate) fn check_proxy_enter_locked(
    service: &crate::services::ProxyService,
    app: &AppType,
    initial: &InitialProxyRoute,
) -> Result<bool, AppError> {
    initial.request.validate()?;
    if initial.request.scope != state::SaveScope::ApplicationRouting {
        return Err(AppError::Config("mode.invalid_save_request".into()));
    }
    if initial.request.draft_digest != proxy_enter_digest(app.as_str(), &initial.provider_id)? {
        return Err(AppError::Conflict("mode.invalid_save_request".into()));
    }
    let status = routing_request_status_for_db(service.database(), app, &initial.request)?;
    match status.as_str() {
        "completed" => return Ok(true),
        "unknown" | "notRecorded" => {}
        _ => return Err(AppError::Conflict("mode.verification_required".into())),
    }
    if initial.request.revision
        != routing_request_revision_for_service(service, app, Some(&initial.provider_id))?
    {
        return Err(AppError::Conflict("mode.verification_required".into()));
    }
    Ok(false)
}

/// Explicit bootstrap is admitted only for a known empty Direct application.
/// ATTACH and recovery retain their original recorded route and never call this.
pub(crate) fn admit_initial_route(
    service: &ProxyService,
    app: &AppType,
    provider_id: &str,
) -> Result<Provider, AppError> {
    if !matches!(app, AppType::Claude | AppType::Codex)
        || !operation::uses_upstream4_schema(service.database())?
    {
        return Err(invalid());
    }
    let before = mode(service, app)?;
    if before.mode != Some(Mode::Direct)
        || before.attached
        || before.proxy_route.is_some()
        || current::direct_provider(service.database(), app)?.is_some()
    {
        return Err(invalid());
    }
    let route = provider(service, app, provider_id)?;
    if crate::services::provider::managed_inference::is_managed_inference(&route) {
        crate::services::provider::managed_inference::validate(app, &route)?;
    } else if crate::services::vendor_source_binding::is_reference(&route)
        && route
            .meta
            .as_ref()
            .and_then(|meta| meta.auth_binding.as_ref())
            .is_some_and(|binding| {
                binding.auth_provider.as_deref()
                    == Some(crate::vendor::Vendor::OpenCode.vendor_id())
            })
    {
        // Validate the original account/plan/destination/key owner, but retain
        // only the saved reference for the original native proxy projection.
        drop(service.database().resolve_vendor_auth(app, &route)?);
    } else {
        return Err(invalid());
    }
    admit(service, app, &route)?;
    Ok(route)
}

pub(crate) async fn enter_initial_route_locked(
    service: &ProxyService,
    app: &AppType,
    initial: InitialProxyRoute,
) -> Result<(), String> {
    if check_proxy_enter_locked(service, app, &initial).map_err(err)? {
        return Ok(());
    }
    enter_with_initial_route_locked(service, app, op::ENTER, Some(initial)).await
}

/// Caller owns the existing service lifecycle and app switch locks.
pub(crate) async fn enter_locked(
    service: &ProxyService,
    app: &AppType,
    operation: &'static str,
) -> Result<(), String> {
    enter_with_initial_route_locked(service, app, operation, None).await
}

async fn enter_with_initial_route_locked(
    service: &ProxyService,
    app: &AppType,
    operation: &'static str,
    initial: Option<InitialProxyRoute>,
) -> Result<(), String> {
    service
        .database()
        .provision_managed_client_credential(app.as_str())
        .map_err(err)?;
    let before = mode(service, app).map_err(err)?;
    let route = if let Some(initial) = &initial {
        admit_initial_route(service, app, &initial.provider_id).map_err(err)?
    } else {
        match before.proxy_route.as_deref() {
            Some(id) => provider(service, app, id).map_err(err)?,
            None if operation == op::ENTER && !before.is_proxy() => {
                current::direct_provider(service.database(), app)
                    .map_err(err)?
                    .ok_or_else(|| err(invalid()))?
            }
            None => return Err(err(invalid())),
        }
    };
    admit(service, app, &route).map_err(err)?;
    let live = LiveNow::of(service, app, &before).map_err(err)?;
    service.invalidate_app_requests(app.as_str()).map_err(err)?;
    let explicit_bootstrap = initial.is_some();
    let created = if explicit_bootstrap {
        service.start_for_mode_owned().await?
    } else {
        service.start_for_mode().await?;
        false
    };
    let result = async {
        let (url, _) = service.build_proxy_urls().await?;
        let mut target = PendingTarget::mode(ModeState {
            mode: Some(Mode::Proxy),
            attached: true,
            proxy_route: Some(route.id.clone()),
            ..Default::default()
        });
        target.save_request = initial.as_ref().map(|initial| initial.request.clone());
        let service = service.owner()?;
        let app = app.clone();
        let crash = operation::failpoint::current_crash();
        tokio::task::spawn_blocking(move || {
            operation::failpoint::in_worker(crash, || {
                // Starting the listener yields. Recheck the same original row,
                // mode, pointers, profile and native snapshots before publication.
                if let Some(initial) = &initial {
                    if check_proxy_enter_locked(&service, &app, initial)? {
                        return Ok(false);
                    }
                    admit_initial_route(&service, &app, &initial.provider_id)?;
                }
                write_proxy(&service, &app, operation, &route, &live, target, &url)
            })
        })
        .await
        .map_err(err)?
        .map(|_| ())
        .map_err(err)
    }
    .await;
    match result {
        Err(error) if explicit_bootstrap && created => {
            // Never stop a pre-existing standalone listener or a listener another
            // start won. The original pending/attached owner retains dependencies.
            if let Err(cleanup) = stop_if_unused(service).await {
                return Err(format!("{error}; listener cleanup failed: {cleanup}"));
            }
            Err(error)
        }
        other => other,
    }
}

pub(crate) fn exit_locked(
    service: &ProxyService,
    app: &AppType,
    keep_mode: bool,
) -> Result<(), AppError> {
    let before = mode(service, app)?;
    if !before.attached && (keep_mode || !before.is_proxy()) {
        return Ok(());
    }
    service.invalidate_app_requests(app.as_str())?;
    let direct = current::direct_provider(service.database(), app)?;
    let live = LiveNow::of(service, app, &before)?;
    let next = ModeState {
        mode: Some(if keep_mode && before.is_proxy() {
            Mode::Proxy
        } else {
            Mode::Direct
        }),
        attached: false,
        proxy_route: before.proxy_route,
        ..Default::default()
    };
    if !before.attached {
        write_target_only(service, app, op::EXIT, PendingTarget::mode(next))?;
        return Ok(());
    }
    write_direct(
        service,
        app,
        if keep_mode { op::DETACH } else { op::EXIT },
        direct.as_ref(),
        &live,
        PendingTarget::mode(next),
    )
    .map(|_| ())
}

pub(crate) async fn switch_route_locked(
    service: &ProxyService,
    app: &AppType,
    route: Provider,
) -> Result<(), String> {
    switch_route_locked_with_catalog(service, app, route, None).await
}

pub(crate) async fn switch_route_locked_with_catalog(
    service: &ProxyService,
    app: &AppType,
    route: Provider,
    catalog: Option<codex_direct::CatalogDecision>,
) -> Result<(), String> {
    if crate::proxy::managed_accounts::uses_workbuddy_model_route(&route) {
        return Err("workbuddy.inference_paused".into());
    }
    crate::services::shared_routing::ensure_legacy_selection_allowed(service.database(), app)
        .map_err(err)?;
    if catalog.is_some() && *app != AppType::Codex {
        return Err(err(invalid()));
    }
    let before = mode(service, app).map_err(err)?;
    if !before.is_proxy() {
        return Err(err(invalid()));
    }
    admit(service, app, &route).map_err(err)?;
    let live = LiveNow::of(service, app, &before).map_err(err)?;
    let mut next = before.clone();
    next.proxy_route = Some(route.id.clone());
    if !before.attached {
        if catalog.is_some() {
            return Err(err(invalid()));
        }
        let service = service.owner()?;
        let app = app.clone();
        let crash = operation::failpoint::current_crash();
        return tokio::task::spawn_blocking(move || {
            operation::failpoint::in_worker(crash, || {
                write_target_only(&service, &app, op::ROUTE, PendingTarget::mode(next))
            })
        })
        .await
        .map_err(err)?
        .map_err(err);
    }
    let (url, _) = service.build_proxy_urls().await?;
    let service = service.owner()?;
    let app = app.clone();
    let crash = operation::failpoint::current_crash();
    tokio::task::spawn_blocking(move || {
        operation::failpoint::in_worker(crash, || {
            write_proxy_with_catalog(
                &service,
                &app,
                op::ROUTE,
                &route,
                &live,
                PendingTarget::mode(next),
                &url,
                catalog.as_ref(),
            )
        })
    })
    .await
    .map_err(err)?
    .map(|_| ())
    .map_err(err)
}

/// Current and non-current saves share the existing journal, without live backfill.
pub(crate) fn save_row_locked(
    service: &ProxyService,
    app: &AppType,
    previous: &Provider,
    provider: &Provider,
    clear_model_preference: bool,
) -> Result<(), AppError> {
    save_row_locked_with_credential_intent(
        service,
        app,
        previous,
        provider,
        clear_model_preference,
        false,
    )
}

pub(crate) fn save_row_locked_with_credential_intent(
    service: &ProxyService,
    app: &AppType,
    previous: &Provider,
    provider: &Provider,
    clear_model_preference: bool,
    reissue: bool,
) -> Result<(), AppError> {
    let (before, in_use, mut target) =
        save_row_target(service, app, previous, provider, clear_model_preference)?;
    if reissue {
        let conn = service.database().conn.lock()?;
        if crate::database::credential_revision::original_provider_uses_revisions(&conn)? {
            target
                .saved_row
                .as_mut()
                .ok_or_else(invalid)?
                .credential_write = Some(crate::database::credential_revision::WriteProof {
                before: crate::database::credential_revision::provider_on(
                    &conn,
                    app.as_str(),
                    &previous.id,
                )?,
                next: crate::database::credential_revision::new_revision(),
            });
        }
    }
    if in_use.as_deref() != Some(provider.id.as_str()) || (before.is_proxy() && !before.attached) {
        write_target_only(service, app, op::APPLY, target)?;
        return Ok(());
    }
    let live = LiveNow::of(service, app, &before)?;
    if before.is_proxy() {
        admit(service, app, provider)?;
        let (url, _) =
            futures::executor::block_on(service.build_proxy_urls()).map_err(AppError::Message)?;
        write_proxy(
            service,
            app,
            op::APPLY,
            provider,
            &live,
            PendingTarget {
                state: Some(before),
                ..target
            },
            &url,
        )
        .map(|_| ())
    } else {
        write_direct(service, app, op::APPLY, Some(provider), &live, target).map(|_| ())
    }
}

/// The selection service owns the app lock and supplies a typed durable target.
pub(crate) fn select_locked(
    service: &ProxyService,
    app: &AppType,
    provider: &Provider,
    target: PendingTarget,
) -> Result<bool, AppError> {
    select_locked_with_catalog(service, app, provider, target, None)
}

/// The caller holds the app switch lock; this is the original selection writer.
pub(crate) fn select_locked_with_catalog(
    service: &ProxyService,
    app: &AppType,
    provider: &Provider,
    mut target: PendingTarget,
    catalog: Option<&codex_direct::CatalogDecision>,
) -> Result<bool, AppError> {
    if crate::proxy::managed_accounts::uses_workbuddy_model_route(provider) {
        return Err(AppError::Config("workbuddy.inference_paused".into()));
    }
    crate::services::shared_routing::ensure_legacy_selection_allowed(service.database(), app)?;
    if catalog.is_some() && *app != AppType::Codex {
        return Err(invalid());
    }
    let before = mode(service, app)?;
    if crate::proxy::application_routing::blocked_tier_ids_checked(
        service.database(),
        app.as_str(),
    )?
    .contains(&provider.id)
    {
        return Err(AppError::Config("routing.blocked".into()));
    }
    crate::services::provider::validate_provider_selection(service.database(), app, &provider.id)?;
    let live = LiveNow::of(service, app, &before)?;
    if before.is_proxy() {
        admit(service, app, provider)?;
        let attached = before.attached;
        let mut next = before;
        next.proxy_route = Some(provider.id.clone());
        target.state = Some(next);
        if !attached {
            if catalog.is_some() {
                return Err(invalid());
            }
            write_target_only(service, app, op::APPLY, target)?;
            return Ok(false);
        }
        let (url, _) =
            futures::executor::block_on(service.build_proxy_urls()).map_err(AppError::Message)?;
        write_proxy_with_catalog(
            service,
            app,
            op::APPLY,
            provider,
            &live,
            target,
            &url,
            catalog,
        )
    } else {
        target.pointer = Some(provider.id.clone());
        write_direct_with_catalog(
            service,
            app,
            op::APPLY,
            Some(provider),
            &live,
            target,
            catalog,
        )
    }
}

/// Read-only directory sub-plan for the current application selection. No auth
/// preparation or CLI discovery occurs here. Caller holds the app switch lock.
pub(crate) fn preview_catalog_selection(
    service: &ProxyService,
    provider: &Provider,
    catalog: Option<&codex_direct::CatalogDecision>,
) -> Result<Option<codex_direct::CatalogPreview>, AppError> {
    crate::services::shared_routing::ensure_legacy_selection_allowed(
        service.database(),
        &AppType::Codex,
    )?;
    let result = (|| {
        let before = mode(service, &AppType::Codex)?;
        if before.is_proxy() && !before.attached {
            return Ok(None);
        }
        let live = LiveNow::of(service, &AppType::Codex, &before)?;
        codex_direct::preview_catalog_choice(
            service.database(),
            &live.codex_owner(),
            provider,
            catalog,
        )
    })();
    result.map_err(|error| match error {
        AppError::Conflict(_) => AppError::Conflict("codex.catalog_source_changed".into()),
        _ => AppError::Config("codex.catalog_preview_unavailable".into()),
    })
}

pub(crate) fn apply_order_locked(
    service: &ProxyService,
    app: &AppType,
    target: PendingTarget,
) -> Result<(), AppError> {
    crate::services::shared_routing::ensure_legacy_policy_edit_allowed(service.database(), app)?;
    mode(service, app)?;
    write_target_only(service, app, op::APPLY, target)
}

pub(crate) fn files(app: &AppType) -> Result<Vec<LiveFile>, AppError> {
    Ok(match app {
        AppType::Claude => vec![claude_direct::settings_file()],
        AppType::Gemini => vec![gemini_direct::env_file(), gemini_direct::settings_file()],
        AppType::GrokBuild => vec![grok_direct::config_file()],
        AppType::Codex => crate::services::provider::codex_direct::files(),
        _ => return Err(invalid()),
    })
}

pub(crate) fn recover_locked(
    service: &ProxyService,
    app: &AppType,
) -> Result<Option<RecoveryOutcome>, AppError> {
    recover_locked_with_checks(service, app, None)
}

pub(crate) fn recover_locked_with_checks(
    service: &ProxyService,
    app: &AppType,
    checks: Option<&operation::AppRecoveryChecks<'_>>,
) -> Result<Option<RecoveryOutcome>, AppError> {
    if *app == AppType::Codex {
        return codex_direct::recover_locked_with_checks(service, checks);
    }
    let write = AppWrite::open_mode_with_recovery(service, app, checks)?;
    let original_pending = state::pending(&write.store, &write.vault, app.as_str())?;
    if let Some(pending) = &original_pending {
        operation::verify_saved_row(
            service.database(),
            service.database().secret_session(),
            &write.vault,
            app,
            &pending.target,
        )?;
    }
    let verify = |pending: &state::Pending, live: Option<&state::LiveState>| {
        operation::verify_saved_row(
            service.database(),
            service.database().secret_session(),
            &write.vault,
            app,
            &pending.target,
        )?;
        checks.map_or(Ok(()), |checks| {
            (checks.verify)(&write.vault, pending, live)
        })
    };
    let finished = |pending: &state::Pending, live: Option<&state::LiveState>| {
        write.verify_recovered_target(pending, live)
    };
    let admission = checks
        .map(|checks| operation::RecoveryAdmission {
            expected_pending: checks.pending,
            verify: &verify,
            finished: &finished,
        })
        .or_else(|| {
            original_pending
                .as_ref()
                .filter(|p| {
                    p.target.route_target.is_some()
                        || p.target
                            .saved_row
                            .as_ref()
                            .is_some_and(|row| row.credential_write.is_some())
                })
                .map(|pending| operation::RecoveryAdmission {
                    expected_pending: pending,
                    verify: &verify,
                    finished: &finished,
                })
        });
    operation::recover_checked_guarded(
        &write.store,
        &write.vault,
        &write.guard,
        &files(app)?,
        &|target| write.commit(target),
        &|_| Ok(None),
        admission.as_ref(),
    )
}

/// Read-only listener dependency proof from the original route/contract owners.
/// Call outside a pinned vault read: the existing planners acquire their own.
pub(crate) fn recovery_listener_required(
    service: &ProxyService,
    app: &AppType,
    pending: &state::Pending,
    address: &str,
    port: u16,
) -> Result<bool, AppError> {
    let store = DeviceStore::for_device();
    let before = {
        let vault = service.database().secret_session().read()?;
        current::validate_known_mode(&store, &vault, app)?
    };
    let forward = operation::recovery_will_roll_forward(pending)?;
    let intended = if forward {
        pending.target.state.as_ref().unwrap_or(&before)
    } else {
        &before
    };
    if !intended.attached {
        return Ok(false);
    }
    if !intended.is_proxy()
        || port == 0
        || !address
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    {
        return Err(invalid());
    }
    let id = intended.proxy_route.as_deref().ok_or_else(invalid)?;
    let route = match &pending.target.saved_row {
        Some(saved) if forward => {
            let row = operation::saved_provider(saved)?;
            if row.id == id {
                row
            } else {
                provider(service, app, id)?
            }
        }
        _ => provider(service, app, id)?,
    };
    admit(service, app, &route)?;
    let live = LiveNow::of(service, app, &before)?;
    // The original builder handles IPv6 and produces the client URL. With no
    // listener/config mismatch accepted, it cannot mix two endpoint identities.
    let (url, _) =
        futures::executor::block_on(service.build_proxy_urls()).map_err(AppError::Message)?;
    let expected = expected_proxy_contract(service, app, &route, &live, &url)?;
    if intended.contract.as_ref() != Some(&expected) {
        return Err(invalid());
    }
    Ok(true)
}

/// Enumerate persisted applications without interpreting peer subtrees. The
/// existing loop admits each app independently; missing/shared-unknown state
/// never instructs it to infer old flags or create a Direct/Proxy decision.
fn persisted_apps(service: &ProxyService) -> Result<Vec<AppType>, AppError> {
    let store = DeviceStore::for_device();
    if crate::live::engine::read_current(&store.state_path())?.is_none() {
        return Err(invalid());
    }
    let vault = service.database().secret_session().read()?;
    let live = state::read_review_snapshot(&store, &vault)?;
    if live.has_shared_extensions() {
        return Err(invalid());
    }
    Ok(PROXY_APPS
        .into_iter()
        .filter(|app| live.app_names().any(|name| name == app.as_str()))
        .collect())
}

/// A listener change affects every attached app. The caller holds the existing
/// configuration-import locks, so these admitted facts cannot change before the
/// new listener is projected. Detached Proxy intent must remain detached.
pub(crate) fn reconfiguration_apps_locked(
    service: &ProxyService,
) -> Result<Vec<AppType>, AppError> {
    let mut attached = Vec::new();
    for app in persisted_apps(service)? {
        let before = mode(service, &app)?;
        if before.is_proxy() && before.attached {
            let route = before.proxy_route.as_deref().ok_or_else(invalid)?;
            admit(service, &app, &provider(service, &app, route)?)?;
            attached.push(app);
        }
    }
    Ok(attached)
}

/// Re-run the same projection after a saved-config retry as well as a restart.
/// An app that failed before intent publication must not be reported repaired
/// merely because the listener/DB already contain the requested configuration.
pub(crate) async fn reconfigure_attached_locked(
    service: &ProxyService,
    apps: Vec<AppType>,
) -> Result<(), String> {
    let (url, _) = service.build_proxy_urls().await?;
    let mut failures = Vec::new();
    for app in apps {
        let expected = (|| {
            let before = mode(service, &app)?;
            let route = provider(
                service,
                &app,
                before.proxy_route.as_deref().ok_or_else(invalid)?,
            )?;
            let live = LiveNow::of(service, &app, &before)?;
            let expected = expected_proxy_contract(service, &app, &route, &live, &url)?;
            Ok::<_, AppError>(before.contract.as_ref() != Some(&expected))
        })();
        let result = match expected {
            Ok(false) => Ok(()),
            Ok(true) => enter_locked(service, &app, op::ATTACH).await,
            Err(error) => Err(err(error)),
        };
        if let Err(error) = result {
            failures.push(format!("{}: {error}", app.as_str()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// The service owns takeover lifecycle; each app retains its original switch
/// lock and independently verified result. An incomplete app keeps the listener.
pub(crate) async fn restore_all_locked(
    service: &ProxyService,
    keep_mode: bool,
) -> Result<(), String> {
    let apps = persisted_apps(service).map_err(err)?;
    let mut failures = Vec::new();
    for app in apps {
        let _switch = service.lock_switch_for_app(app.as_str()).await;
        let owner = service.owner()?;
        let worker_app = app.clone();
        let crash = operation::failpoint::current_crash();
        let result = tokio::task::spawn_blocking(move || {
            operation::failpoint::in_worker(crash, || exit_locked(&owner, &worker_app, keep_mode))
        })
        .await
        .map_err(err)
        .and_then(|result| result.map_err(err));
        if let Err(error) = result {
            failures.push(format!("{}: {error}", app.as_str()));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    stop_if_unused(service).await
}

/// Startup reattaches only the saved mode. Pending operations require explicit
/// checked recovery; attach failure never launches a compensating exit.
pub(crate) async fn startup_locked(service: &ProxyService) -> Result<(), String> {
    // The original checkpoint still suspends automatic takeover during review.
    // Explicit app operations retain their independently verified native path.
    crate::secrets::upgrade::checkpoint::ensure_no_pending_checkpoint(&DeviceStore::for_device())
        .map_err(err)?;
    let apps = persisted_apps(service).map_err(err)?;
    let mut failures = Vec::new();
    for app in apps {
        let _switch = service.lock_switch_for_app(app.as_str()).await;
        let result = match mode(service, &app) {
            Ok(before) if before.is_proxy() => enter_locked(service, &app, op::ATTACH).await,
            Ok(_) => Ok(()),
            Err(error) => Err(err(error)),
        };
        if let Err(error) = result {
            failures.push(format!("{}: {error}", app.as_str()));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    stop_if_unused(service).await
}

/// The same authenticated snapshot supplies the hint and locked stop check.
/// An opaque app is not proof that stopping is safe. Shared envelope uncertainty
/// still fails globally; compatibility flags never supply routing authority.
pub(crate) fn needs_listener(service: &ProxyService) -> Result<bool, AppError> {
    let store = DeviceStore::for_device();
    if crate::live::engine::read_current(&store.state_path())?.is_none() {
        return Err(invalid());
    }
    let vault = service.database().secret_session().read()?;
    let live = state::read_review_snapshot(&store, &vault)?;
    if live.has_shared_extensions() {
        return Err(invalid());
    }
    for name in live.app_names() {
        if !PROXY_APPS.iter().any(|app| app.as_str() == name) {
            return Ok(true);
        }
        let selected = match live.app_view(name) {
            Ok(selected) => selected,
            Err(_) => return Ok(true),
        };
        let entry = selected.apps.get(name).ok_or_else(invalid)?;
        if state::validate_app_evidence_for_update(name, entry).is_err()
            || entry.mode.is_none()
            || entry.stack.enabled
            || entry.attached
            || entry.pending.is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) async fn stop_if_unused(service: &ProxyService) -> Result<(), String> {
    if !needs_listener(service).map_err(err)? && service.is_running().await {
        service.stop().await?;
    }
    Ok(())
}

fn save_row_target(
    service: &ProxyService,
    app: &AppType,
    previous: &Provider,
    provider: &Provider,
    clear_model_preference: bool,
) -> Result<(ModeState, Option<String>, PendingTarget), AppError> {
    if previous.id != provider.id {
        return Err(invalid());
    }
    crate::proxy::application_routing::blocked_tier_ids_checked(service.database(), app.as_str())?;
    let before = {
        let vault = service.database().secret_session().read()?;
        let store = DeviceStore::for_device();
        if state::pending(&store, &vault, app.as_str())?.is_some() {
            return Err(invalid());
        }
        current::validate_known_mode(&store, &vault, app)?
    };
    let in_use = current::provider_for(service.database(), app, current::Purpose::InUse)?;
    let row = SavedRow {
        credential_write: None,
        before: Database::provider_update_digest(previous)?,
        provider: Database::provider_update_value(provider)?,
        clear_model_preference: clear_model_preference
            && in_use.as_deref() == Some(provider.id.as_str()),
    };
    let target = PendingTarget {
        saved_row: Some(row),
        ..Default::default()
    };
    Ok((before, in_use, target))
}

/// A request-local plan, not a second draft or persistent operation owner.
pub(crate) struct SaveRowPlan {
    pub(crate) credential_reissue: bool,
    credential_before: Option<String>,
    pub(crate) files: Vec<(LiveFile, crate::live::patch::Guarded)>,
    pub(crate) target: PendingTarget,
    pub(crate) revision: String,
    pub(crate) preserved_catalog: bool,
    pub(crate) generation: Option<String>,
    pub(crate) catalog: Option<codex_direct::CatalogPreview>,
    catalog_inputs: Option<codex_direct::NativeCatalogInputs>,
}

fn planned_patch(
    file: LiveFile,
    patch: &dyn crate::live::patch::LivePatch,
) -> Result<(LiveFile, crate::live::patch::Guarded), AppError> {
    let pre = crate::live::engine::read_current(&file.path)?;
    let after = patch
        .apply_file(&file.path, pre.as_deref())
        .map_err(|_| invalid())?;
    Ok((
        file,
        crate::live::patch::Guarded {
            expected_pre: crate::live::engine::digest(pre.as_deref()),
            then: after
                .map(crate::live::patch::WholeFile::Write)
                .unwrap_or(crate::live::patch::WholeFile::Delete),
        },
    ))
}

pub(crate) fn plan_row_save(
    service: &ProxyService,
    app: &AppType,
    previous: &Provider,
    provider: &Provider,
    request_id: &str,
    catalog_decision: Option<&codex_direct::CatalogDecision>,
) -> Result<SaveRowPlan, AppError> {
    plan_row_save_with_generation(
        service,
        app,
        previous,
        provider,
        request_id,
        catalog_decision,
        None,
    )
}

/// Pure row planning may borrow a generation already pinned by the caller.
/// The caller still owns lifecycle/app serialization and completion checks.
pub(crate) fn plan_row_save_with_generation(
    service: &ProxyService,
    app: &AppType,
    previous: &Provider,
    provider: &Provider,
    request_id: &str,
    catalog_decision: Option<&codex_direct::CatalogDecision>,
    auth_generation: Option<&CodexLiveAuthGuard<'_>>,
) -> Result<SaveRowPlan, AppError> {
    let changed = crate::relay::provider_config::selected_model(app, &previous.settings_config)
        != crate::relay::provider_config::selected_model(app, &provider.settings_config);
    let (before, in_use, mut target) = save_row_target(service, app, previous, provider, changed)?;
    // SavedRow above stays reference-only; only file projections receive credentials.
    let resolved = service.database().resolve_vendor_auth(app, provider)?;
    let provider = &resolved;
    let live_state = {
        let vault = service.database().secret_session().read()?;
        state::load_app(&DeviceStore::for_device(), &vault, app.as_str())?
    };
    let entry = live_state.apps.get(app.as_str()).ok_or_else(invalid)?;
    state::validate_app_evidence_for_update(app.as_str(), entry)?;
    let direct = current::provider_for(service.database(), app, current::Purpose::Direct)?;
    let mut files = vec![];
    let mut preserved_catalog = false;
    let mut generation = None;
    let mut codex_revision = None;
    let mut catalog = None;
    let mut catalog_inputs = None;
    let mut endpoint = None;
    if in_use.as_deref() == Some(provider.id.as_str()) && (!before.is_proxy() || before.attached) {
        let live = LiveNow::of(service, app, &before)?;
        if before.is_proxy() {
            admit(service, app, provider)?;
            endpoint = Some(
                futures::executor::block_on(service.build_proxy_urls_existing())
                    .map_err(|_| invalid())?
                    .0,
            );
            target.state = Some(before.clone());
        }
        match app {
            AppType::Claude => {
                let projection = match endpoint.as_ref() {
                    Some(url) => claude_proxy_projection(
                        provider,
                        url,
                        &crate::proxy::client_access::managed::key(service.database(), "claude")?,
                    ),
                    None => ClaudeProjection::of(&provider.settings_config),
                };
                if let Some(next) = &mut target.state {
                    next.contract = Some(contract::claude(&projection));
                }
                files.push(planned_patch(
                    claude_direct::settings_file(),
                    &direct_patch(live.claude_owner().as_ref(), &projection),
                )?);
            }
            AppType::Gemini => {
                let projection = match &endpoint {
                    Some(url) => gemini_proxy_projection(
                        provider,
                        url,
                        &crate::proxy::client_access::managed::key(service.database(), "gemini")?,
                    ),
                    None => gemini_direct::projection(provider)?,
                };
                if let Some(next) = &mut target.state {
                    next.contract = Some(contract::gemini(&projection));
                }
                files.push(planned_patch(
                    gemini_direct::env_file(),
                    &projection.env_patch(),
                )?);
                files.push(planned_patch(
                    gemini_direct::settings_file(),
                    &projection.settings_patch(),
                )?);
            }
            AppType::GrokBuild => {
                let projection = match &endpoint {
                    Some(url) => grok_proxy_projection(
                        provider,
                        url,
                        &crate::proxy::client_access::managed::key(
                            service.database(),
                            "grokbuild",
                        )?,
                    )?,
                    None => grok_direct::projection(provider)?,
                };
                if let Some(next) = &mut target.state {
                    next.contract = Some(contract::grok(&projection));
                }
                target.written = Some(state::Written {
                    tables: projection.written_tables(),
                    ..Default::default()
                });
                let patch = crate::live::project::grok::GrokConfigPatch::direct(
                    &projection,
                    grok_direct::retired_tables_from_written(
                        entry.written.as_ref(),
                        live.direct_owner(),
                    ),
                    PROXY_TOKEN_PLACEHOLDER,
                );
                files.push(planned_patch(
                    grok_direct::config_file(),
                    &crate::live::patch::toml::TomlSteps(vec![&patch]),
                )?);
            }
            AppType::Codex => {
                let url = endpoint
                    .as_ref()
                    .map(|url| format!("{}/v1", url.trim_end_matches('/')));
                let desired = match &url {
                    Some(url) => codex_direct::Target::Proxy {
                        route: provider,
                        base_url: url,
                    },
                    None => codex_direct::Target::Direct(Some(provider)),
                };
                let output = match auth_generation {
                    Some(generation) => codex_direct::plan_editor_save_with_generation(
                        service,
                        &live.codex_owner(),
                        desired,
                        target,
                        request_id,
                        catalog_decision,
                        Some(generation),
                    ),
                    None => codex_direct::plan_editor_save(
                        service,
                        &live.codex_owner(),
                        desired,
                        target,
                        request_id,
                        catalog_decision,
                    ),
                }?;
                files = output.0;
                target = output.1;
                preserved_catalog = output.2;
                codex_revision = Some(output.3);
                generation = Some(output.4);
                catalog = Some(output.5);
                catalog_inputs = Some(output.6);
            }
            _ => return Err(invalid()),
        }
    }
    if catalog_decision.is_some() && catalog.is_none() {
        return Err(invalid());
    }
    // Hash only the original inputs actually used by this plan. No durable
    // before-image framework; confirmed files use the existing Guarded patch.
    let file_versions = files
        .iter()
        .map(|(file, patch)| (&file.path, file.private, &patch.expected_pre))
        .collect::<Vec<_>>();
    let credential_before = {
        let conn = service.database().conn.lock()?;
        if crate::database::credential_revision::original_provider_uses_revisions(&conn)? {
            Some(crate::database::credential_revision::provider_on(
                &conn,
                app.as_str(),
                &previous.id,
            )?)
        } else {
            // No shared issuance proof exists in an original retained schema.
            // Preserve that original plan image rather than adding a null field.
            None
        }
    };
    // Keep both pre-existing wire images exact: LP31 prefixes its authoritative
    // revision; retained pre31 plans use the original tuple, not a new null field.
    let source = if let Some(credential_before) = &credential_before {
        serde_json::to_vec(&(
            credential_before,
            request_id,
            app.as_str(),
            service.database().secret_session().root(),
            &target.saved_row,
            &before,
            &in_use,
            &direct,
            entry,
            &file_versions,
            &endpoint,
            &codex_revision,
            crate::proxy::auto_strategy::get_model_pref_checked(service.database(), app.as_str())?,
        ))
    } else {
        serde_json::to_vec(&(
            request_id,
            app.as_str(),
            service.database().secret_session().root(),
            &target.saved_row,
            &before,
            &in_use,
            &direct,
            entry,
            &file_versions,
            &endpoint,
            &codex_revision,
            crate::proxy::auto_strategy::get_model_pref_checked(service.database(), app.as_str())?,
        ))
    }
    .map_err(|source| AppError::JsonSerialize { source })?;
    Ok(SaveRowPlan {
        credential_reissue: false,
        credential_before,
        files,
        target,
        revision: crate::live::engine::sha256_hex(&source),
        preserved_catalog,
        generation,
        catalog,
        catalog_inputs,
    })
}

/// Borrow the original gateway projection plan as readback witnesses only.
/// No credential provisioning, file rewrite, or alternate recovery owner.
pub(crate) fn gateway_route_plan(
    service: &ProxyService,
    app: &AppType,
    request_id: &str,
) -> Result<SaveRowPlan, AppError> {
    let before = {
        let write = AppWrite::begin_mode(service, app)?;
        let before = current::validate_known_mode(&write.store, &write.vault, app)?;
        if !before.is_proxy() || !before.attached {
            return Err(AppError::Config(
                "routing_groups.connection_required".into(),
            ));
        }
        before
    };
    let id = before.proxy_route.as_ref().ok_or_else(invalid)?;
    let provider = service
        .database()
        .get_provider_by_id(id, app.as_str())?
        .ok_or_else(invalid)?;
    let mut plan = plan_row_save(service, app, &provider, &provider, request_id, None)?;
    if plan.files.is_empty() || plan.target.state.as_ref() != Some(&before) {
        return Err(invalid());
    }
    if *app == AppType::Codex {
        let vault = service.database().secret_session().read()?;
        if !codex_direct::planned_files_match(&plan.files, &vault)? {
            return Err(invalid());
        }
    }
    for (file, patch) in &mut plan.files {
        let bytes = crate::live::engine::read_current(&file.path)?;
        if crate::live::engine::digest(bytes.as_deref()) != patch.expected_pre {
            return Err(invalid());
        }
        if *app != AppType::Codex {
            let expected = match &patch.then {
                crate::live::patch::WholeFile::Write(bytes) => Some(bytes.as_slice()),
                crate::live::patch::WholeFile::Delete => None,
            };
            if expected != bytes.as_deref() {
                return Err(invalid());
            }
        }
        // Retain exact original bytes, including formatting and encrypted stash.
        patch.then = bytes
            .map(crate::live::patch::WholeFile::Write)
            .unwrap_or(crate::live::patch::WholeFile::Delete);
    }
    Ok(plan)
}

pub(crate) fn consume_row_save(
    service: &ProxyService,
    app: &AppType,
    plan: SaveRowPlan,
    request: state::SaveRequest,
) -> Result<(), AppError> {
    if let Some(inputs) = &plan.catalog_inputs {
        inputs.verify_references()?;
    }
    let mut target = plan.target;
    if plan.credential_reissue {
        if request.scope != state::SaveScope::Provider {
            return Err(invalid());
        }
        let row = target.saved_row.as_mut().ok_or_else(invalid)?;
        if let Some(before) = plan.credential_before {
            row.credential_write = Some(crate::database::credential_revision::WriteProof {
                before,
                next: crate::database::credential_revision::new_revision(),
            });
        }
        // Legacy key-touched edits retain the original saved-row CAS and journal,
        // without fabricating a future-schema issuance proof or shared authority.
    }
    // The private proof is persisted/authenticated by this original journal
    // before any native files or SQL row are published. Recovery never regenerates it.
    target.save_request = Some(request.clone());
    if plan.files.is_empty() {
        return write_target_only(service, app, op::APPLY, target);
    }
    let changes = plan
        .files
        .iter()
        .map(|(file, patch)| operation::FileChange {
            file: file.clone(),
            patch: patch as &dyn crate::live::patch::LivePatch,
        })
        .collect::<Vec<_>>();
    if let Some(expected) = plan.generation {
        return service
            .codex_manager()
            .try_with_live_auth_guard(|generation| {
                if generation.native_revision(&request.id)? != expected {
                    return Err(invalid());
                }
                AppWrite::begin_codex_mode(service, generation)?
                    .run(op::APPLY, &changes, target)
                    .map(|_| ())
            });
    }
    AppWrite::begin_mode(service, app)?
        .run(op::APPLY, &changes, target)
        .map(|_| ())
}

#[cfg(all(feature = "gui", feature = "test-hooks"))]
pub(crate) fn verify_credential_write_fixture(case: &str) {
    super::controller_tests::verify_credential_write(case);
}
