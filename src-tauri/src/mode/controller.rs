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

fn claude_proxy_projection(route: &Provider, url: &str) -> ClaudeProjection {
    let auth = if route.uses_managed_account_auth() {
        ProxyAuth::Managed {
            auth_token: !route.is_github_copilot() || !route.claude_uses_api_key_field(),
        }
    } else {
        ProxyAuth::FollowRow
    };
    proxy_projection(
        &ClaudeProjection::of(&route.settings_config),
        url,
        auth,
        None,
    )
}

fn gemini_proxy_projection(route: &Provider, url: &str) -> GeminiProjection {
    GeminiProjection::proxy_contract(
        &GeminiProjection::of(&route.settings_config, false),
        url,
        PROXY_TOKEN_PLACEHOLDER,
    )
}

fn grok_proxy_projection(route: &Provider, url: &str) -> Result<GrokProjection, AppError> {
    GrokProjection::proxy_contract(
        &grok_direct::projection(route)?,
        &format!("{}/grokbuild/v1", url.trim_end_matches('/')),
        PROXY_TOKEN_PLACEHOLDER,
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
        AppType::Claude => contract::claude(&claude_proxy_projection(route, url)),
        AppType::Gemini => contract::gemini(&gemini_proxy_projection(route, url)),
        AppType::GrokBuild => contract::grok(&grok_proxy_projection(route, url)?),
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
    mut target: PendingTarget,
    url: &str,
) -> Result<bool, AppError> {
    let mut next = target.state.clone().ok_or_else(invalid)?;
    match app {
        AppType::Codex => {
            return codex_direct::apply_mode(
                service,
                &live.codex_owner(),
                codex_direct::Target::Proxy {
                    route,
                    base_url: &format!("{}/v1", url.trim_end_matches('/')),
                },
                operation,
                target,
            );
        }
        AppType::Claude => {
            let projection = claude_proxy_projection(route, url);
            next.contract = Some(contract::claude(&projection));
            target.state = Some(next);
            let patch = direct_patch(live.claude_owner().as_ref(), &projection);
            let write = AppWrite::begin_mode(service, app)?;
            claude_direct::run_with_write(&write, operation, Some(&patch), target)?;
        }
        AppType::Gemini => {
            let projection = gemini_proxy_projection(route, url);
            next.contract = Some(contract::gemini(&projection));
            target.state = Some(next);
            let write = AppWrite::begin_mode(service, app)?;
            gemini_direct::run_with_write(&write, operation, Some(&projection), target)?;
        }
        AppType::GrokBuild => {
            let projection = grok_proxy_projection(route, url)?;
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
    service.invalidate_app_requests(app.as_str()).map_err(err)?;
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
    .map(|_| ())
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
    let (before, in_use, target) =
        save_row_target(service, app, previous, provider, clear_model_preference)?;
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
    mut target: PendingTarget,
) -> Result<bool, AppError> {
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
            write_target_only(service, app, op::APPLY, target)?;
            return Ok(false);
        }
        let (url, _) =
            futures::executor::block_on(service.build_proxy_urls()).map_err(AppError::Message)?;
        write_proxy(service, app, op::APPLY, provider, &live, target, &url)
    } else {
        target.pointer = Some(provider.id.clone());
        write_direct(service, app, op::APPLY, Some(provider), &live, target)
    }
}

pub(crate) fn apply_order_locked(
    service: &ProxyService,
    app: &AppType,
    target: PendingTarget,
) -> Result<(), AppError> {
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
    if let Some(pending) = state::pending(&write.store, &write.vault, app.as_str())? {
        operation::verify_saved_row(
            service.database(),
            service.database().secret_session(),
            &write.vault,
            app,
            &pending.target,
        )?;
    }
    let verify = |pending: &state::Pending, live: Option<&state::LiveState>| {
        checks.map_or(Ok(()), |checks| {
            (checks.verify)(&write.vault, pending, live)
        })
    };
    let finished = |pending: &state::Pending, live: Option<&state::LiveState>| {
        write.verify_recovered_target(pending, live)
    };
    let admission = checks.map(|checks| operation::RecoveryAdmission {
        expected_pending: checks.pending,
        verify: &verify,
        finished: &finished,
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
    pub(crate) files: Vec<(LiveFile, crate::live::patch::Guarded)>,
    pub(crate) target: PendingTarget,
    pub(crate) revision: String,
    pub(crate) preserved_catalog: bool,
    pub(crate) generation: Option<String>,
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
) -> Result<SaveRowPlan, AppError> {
    let changed = crate::relay::provider_config::selected_model(app, &previous.settings_config)
        != crate::relay::provider_config::selected_model(app, &provider.settings_config);
    let (before, in_use, mut target) = save_row_target(service, app, previous, provider, changed)?;
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
                let projection = endpoint
                    .as_ref()
                    .map(|url| claude_proxy_projection(provider, url))
                    .unwrap_or_else(|| ClaudeProjection::of(&provider.settings_config));
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
                    Some(url) => gemini_proxy_projection(provider, url),
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
                    Some(url) => grok_proxy_projection(provider, url)?,
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
                let output = codex_direct::plan_editor_save(
                    service,
                    &live.codex_owner(),
                    desired,
                    target,
                    request_id,
                )?;
                files = output.0;
                target = output.1;
                preserved_catalog = output.2;
                codex_revision = Some(output.3);
                generation = Some(output.4);
            }
            _ => return Err(invalid()),
        }
    }
    // Hash only the original inputs actually used by this plan. No durable
    // before-image framework; confirmed files use the existing Guarded patch.
    let file_versions = files
        .iter()
        .map(|(file, patch)| (&file.path, file.private, &patch.expected_pre))
        .collect::<Vec<_>>();
    let source = serde_json::to_vec(&(
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
    .map_err(|source| AppError::JsonSerialize { source })?;
    Ok(SaveRowPlan {
        files,
        target,
        revision: crate::live::engine::sha256_hex(&source),
        preserved_catalog,
        generation,
    })
}

pub(crate) fn consume_row_save(
    service: &ProxyService,
    app: &AppType,
    plan: SaveRowPlan,
    request: state::SaveRequest,
) -> Result<(), AppError> {
    let mut target = plan.target;
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
