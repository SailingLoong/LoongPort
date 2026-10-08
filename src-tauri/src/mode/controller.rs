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
            Ok(Self::Direct(current::direct_provider(
                service.database(),
                app,
            )?))
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

fn write_proxy(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    route: &Provider,
    live: &LiveNow,
    mut target: PendingTarget,
    url: &str,
) -> Result<(), AppError> {
    let mut next = target.state.clone().ok_or_else(invalid)?;
    match app {
        AppType::Codex => {
            codex_direct::apply_mode(
                service,
                &live.codex_owner(),
                codex_direct::Target::Proxy {
                    route,
                    base_url: &format!("{}/v1", url.trim_end_matches('/')),
                },
                operation,
                target,
            )?;
        }
        AppType::Claude => {
            let auth = if route.uses_managed_account_auth() {
                ProxyAuth::Managed {
                    auth_token: !route.is_github_copilot() || !route.claude_uses_api_key_field(),
                }
            } else {
                ProxyAuth::FollowRow
            };
            let projection = proxy_projection(
                &ClaudeProjection::of(&route.settings_config),
                url,
                auth,
                None,
            );
            next.contract = Some(contract::claude(&projection));
            target.state = Some(next);
            let patch = direct_patch(live.claude_owner().as_ref(), &projection);
            let write = AppWrite::begin_mode(service, app)?;
            claude_direct::run_with_write(&write, operation, Some(&patch), target)?;
        }
        AppType::Gemini => {
            let projection = GeminiProjection::proxy_contract(
                &GeminiProjection::of(&route.settings_config, false),
                url,
                PROXY_TOKEN_PLACEHOLDER,
            );
            next.contract = Some(contract::gemini(&projection));
            target.state = Some(next);
            let write = AppWrite::begin_mode(service, app)?;
            gemini_direct::run_with_write(&write, operation, Some(&projection), target)?;
        }
        AppType::GrokBuild => {
            let projection = GrokProjection::proxy_contract(
                &grok_direct::projection(route)?,
                &format!("{}/grokbuild/v1", url.trim_end_matches('/')),
                PROXY_TOKEN_PLACEHOLDER,
            )
            .map_err(|e| AppError::Config(e.to_string()))?;
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
    Ok(())
}

fn write_direct(
    service: &ProxyService,
    app: &AppType,
    operation: &str,
    direct: Option<&Provider>,
    live: &LiveNow,
    target: PendingTarget,
) -> Result<(), AppError> {
    if *app == AppType::Codex {
        return codex_direct::apply_mode(
            service,
            &live.codex_owner(),
            codex_direct::Target::Direct(direct),
            operation,
            target,
        );
    }
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
    Ok(())
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

/// Caller owns the existing service lifecycle and app switch locks.
pub(crate) async fn enter_locked(
    service: &ProxyService,
    app: &AppType,
    operation: &'static str,
) -> Result<(), String> {
    let before = mode(service, app).map_err(err)?;
    let route = match before.proxy_route.as_deref() {
        Some(id) => provider(service, app, id).map_err(err)?,
        None if operation == op::ENTER && !before.is_proxy() => {
            current::direct_provider(service.database(), app)
                .map_err(err)?
                .ok_or_else(|| err(invalid()))?
        }
        None => return Err(err(invalid())),
    };
    admit(service, app, &route).map_err(err)?;
    let live = LiveNow::of(service, app, &before).map_err(err)?;
    service.start_for_mode().await?;
    let (url, _) = service.build_proxy_urls().await?;
    let target = PendingTarget::mode(ModeState {
        mode: Some(Mode::Proxy),
        attached: true,
        proxy_route: Some(route.id.clone()),
        ..Default::default()
    });
    let service = service.owner()?;
    let app = app.clone();
    let crash = operation::failpoint::current_crash();
    tokio::task::spawn_blocking(move || {
        operation::failpoint::in_worker(crash, || {
            write_proxy(&service, &app, operation, &route, &live, target, &url)
        })
    })
    .await
    .map_err(err)?
    .map_err(err)
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
}

pub(crate) async fn switch_route_locked(
    service: &ProxyService,
    app: &AppType,
    route: Provider,
) -> Result<(), String> {
    let before = mode(service, app).map_err(err)?;
    if !before.is_proxy() {
        return Err(err(invalid()));
    }
    admit(service, app, &route).map_err(err)?;
    let live = LiveNow::of(service, app, &before).map_err(err)?;
    let mut next = before.clone();
    next.proxy_route = Some(route.id.clone());
    if !before.attached {
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
            write_proxy(
                &service,
                &app,
                op::ROUTE,
                &route,
                &live,
                PendingTarget::mode(next),
                &url,
            )
        })
    })
    .await
    .map_err(err)?
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
    if previous.id != provider.id {
        return Err(invalid());
    }
    crate::proxy::application_routing::blocked_tier_ids_checked(service.database(), app.as_str())?;
    let before = mode(service, app)?;
    let in_use = current::provider_for(service.database(), app, current::Purpose::InUse)?;
    let row = SavedRow {
        before: Database::provider_update_digest(previous)?,
        provider: Database::provider_update_value(provider)?,
        clear_model_preference: clear_model_preference && in_use.as_deref() == Some(&provider.id),
    };
    let target = PendingTarget {
        saved_row: Some(row),
        ..Default::default()
    };
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
    } else {
        write_direct(service, app, op::APPLY, Some(provider), &live, target)
    }
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
    if *app == AppType::Codex {
        return codex_direct::recover_locked(service);
    }
    let write = AppWrite::open_mode(service, app)?;
    if let Some(pending) = state::pending(&write.store, &write.vault, app.as_str())? {
        operation::verify_saved_row(
            service.database(),
            service.database().secret_session(),
            &write.vault,
            app,
            &pending.target,
        )?;
    }
    operation::recover(
        &write.store,
        &write.vault,
        &write.guard,
        &files(app)?,
        &|target| write.commit(target),
    )
}

/// Only persisted, admitted applications participate. Missing state is not an
/// instruction to infer old flags or create a new Direct/Proxy decision.
fn persisted_apps(service: &ProxyService) -> Result<Vec<AppType>, AppError> {
    let store = DeviceStore::for_device();
    if crate::live::engine::read_current(&store.state_path())?.is_none() {
        return Err(invalid());
    }
    let vault = service.database().secret_session().read()?;
    let live = state::load(&store, &vault)?;
    if !live.extra.is_empty() {
        return Err(invalid());
    }
    Ok(PROXY_APPS
        .into_iter()
        .filter(|app| live.apps.contains_key(app.as_str()))
        .collect())
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

/// The same admitted mode snapshot supplies both the UI hint and the locked
/// execution-time stop check. Compatibility flags are not routing authority.
pub(crate) fn needs_listener(service: &ProxyService) -> Result<bool, AppError> {
    let store = DeviceStore::for_device();
    if crate::live::engine::read_current(&store.state_path())?.is_none() {
        return Err(invalid());
    }
    let vault = service.database().secret_session().read()?;
    let live = state::load(&store, &vault)?;
    if !live.extra.is_empty() {
        return Err(invalid());
    }
    for app in PROXY_APPS {
        if let Some(entry) = live.apps.get(app.as_str()) {
            let mode = entry.mode_state();
            mode.validate_for_update().map_err(|_| invalid())?;
            if mode.mode.is_none() || entry.stack.enabled {
                return Err(invalid());
            }
        }
    }
    Ok(live
        .apps
        .values()
        .any(|entry| entry.attached || entry.pending.is_some()))
}

pub(crate) async fn stop_if_unused(service: &ProxyService) -> Result<(), String> {
    if !needs_listener(service).map_err(err)? && service.is_running().await {
        service.stop().await?;
    }
    Ok(())
}
