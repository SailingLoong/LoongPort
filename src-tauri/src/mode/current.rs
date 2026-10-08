//! Upstream current-provider owner: direct pointer and in-use route are distinct.
//! LoongPort reads are fallible and never infer direct mode from missing/corrupt
//! state, repair a pointer, or recover an operation as a side effect.
use super::state::{self, Mode, ModeState};
use crate::app_config::AppType;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::live::engine::DeviceStore;
use crate::provider::Provider;
use crate::secrets::VaultContext;
use std::sync::RwLockReadGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Direct,
    InUse,
}

pub(crate) fn mode_state(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &AppType,
) -> Result<ModeState, AppError> {
    state::mode_state(store, vault, app.as_str())
}

pub(crate) fn validate_known_mode(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &AppType,
) -> Result<ModeState, AppError> {
    let live = state::load(store, vault)?;
    known_mode_from_state(&live, app)
}

pub(crate) fn known_mode_from_state(
    live: &state::LiveState,
    app: &AppType,
) -> Result<ModeState, AppError> {
    let entry = live
        .apps
        .get(app.as_str())
        .ok_or_else(|| AppError::Config("mode.verification_required".into()))?;
    let mode = entry.mode_state();
    mode.validate_for_update()
        .map_err(|_| AppError::Config("mode.verification_required".into()))?;
    if mode.mode.is_none() || entry.stack.enabled || !live.extra.is_empty() {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    Ok(mode)
}

pub(crate) fn validate_direct_mode(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    app: &AppType,
) -> Result<(), AppError> {
    let mut entry = state::load(store, vault)?
        .apps
        .get(app.as_str())
        .cloned()
        .ok_or_else(|| AppError::Config("mode.verification_required".into()))?;
    let mode = entry.mode_state();
    if mode.mode != Some(Mode::Direct) || mode.attached {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    entry
        .set_mode_state(mode)
        .map_err(|_| AppError::Config("mode.verification_required".into()))
}

pub(crate) fn provider_exists(db: &Database, app: &AppType, id: &str) -> Result<bool, AppError> {
    // Identity-only admission needs no decrypted row and must not recursively
    // acquire a SecretSession read lock while a key writer may be waiting.
    Ok(lock_conn!(db.conn).query_row(
        "SELECT EXISTS(SELECT 1 FROM providers WHERE app_type=?1 AND id=?2)",
        rusqlite::params![app.as_str(), id],
        |row| row.get(0),
    )?)
}

pub(crate) fn provider_for(
    db: &Database,
    app: &AppType,
    purpose: Purpose,
) -> Result<Option<String>, AppError> {
    if purpose == Purpose::InUse && app.supports_local_proxy() {
        let vault = db.secret_session().read()?;
        let store = DeviceStore::for_device();
        let mode = validate_known_mode(&store, &vault, app)?;
        if state::pending(&store, &vault, app.as_str())?.is_some() {
            return Err(AppError::Config("mode.verification_required".into()));
        }
        match mode.mode {
            Some(Mode::Proxy) => {
                let id = mode
                    .proxy_route
                    .ok_or_else(|| AppError::Config("mode.verification_required".into()))?;
                if !provider_exists(db, app, &id)? {
                    return Err(AppError::Config("mode.verification_required".into()));
                }
                return Ok(Some(id));
            }
            Some(Mode::Direct) => {}
            None => return Err(AppError::Config("mode.verification_required".into())),
        }
    }
    if let Some(id) = local_direct_pointer(app)? {
        if provider_exists(db, app, &id)? {
            return Ok(Some(id));
        }
    }
    db.get_current_provider(app.as_str())
}

/// Resolve/decrypt before pinning the operation's session guard.
pub(crate) fn direct_provider(db: &Database, app: &AppType) -> Result<Option<Provider>, AppError> {
    match provider_for(db, app, Purpose::Direct)? {
        Some(id) => db.get_provider_by_id(&id, app.as_str()),
        None => Ok(None),
    }
}

pub(crate) fn provider_in_use(db: &Database, app: &AppType) -> Result<Option<Provider>, AppError> {
    match provider_for(db, app, Purpose::InUse)? {
        Some(id) => db.get_provider_by_id(&id, app.as_str()),
        None => Ok(None),
    }
}

pub(crate) fn local_direct_pointer(app: &AppType) -> Result<Option<String>, AppError> {
    crate::settings::get_current_provider_ready(app)
}

pub(crate) fn verify_direct_pointer(
    db: &Database,
    app: &AppType,
    id: &str,
) -> Result<(), AppError> {
    if db.get_current_provider(app.as_str())?.as_deref() != Some(id)
        || local_direct_pointer(app)?.as_deref() != Some(id)
    {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    Ok(())
}

/// Read-only application facts, not a serialized journal or a recovery command.
/// `publication_started` describes the marker only; it does not claim every file
/// was written, or that an unpublished target-only operation had no side effects.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingReadView {
    pub status: &'static str,
    pub mode: Option<Mode>,
    pub attached: Option<bool>,
    pub current_provider_id: Option<String>,
    pub direct_provider_id: Option<String>,
    pub publication_started: Option<bool>,
    pub can_write: bool,
    pub can_recheck: bool,
    pub legacy_common_config_writable: bool,
}

impl RoutingReadView {
    fn blocked(status: &'static str, publication_started: Option<bool>) -> Self {
        Self {
            status,
            mode: None,
            attached: None,
            current_provider_id: None,
            direct_provider_id: None,
            publication_started,
            can_write: false,
            can_recheck: true,
            legacy_common_config_writable: false,
        }
    }
}

/// Caller holds the original service app lock for a coherent page snapshot.
/// Legacy/independent apps retain their existing read contract. All unavailable
/// modern facts remain explicitly unknown; no read opens a recovery transaction.
pub(crate) fn read_view(
    service: &crate::services::ProxyService,
    app: &AppType,
) -> Option<RoutingReadView> {
    if !app.supports_local_proxy() {
        return None;
    }
    let db = service.database();
    match super::operation::uses_upstream4_schema(db) {
        Ok(false) => return None,
        Err(_) => return Some(RoutingReadView::blocked("unknown", None)),
        Ok(true) => {}
    }
    let read = || -> Result<RoutingReadView, AppError> {
        let mode = {
            let write = super::operation::AppWrite::open_mode(service, app)?;
            let mode = validate_known_mode(&write.store, &write.vault, app)?;
            if let Some(pending) = state::pending(&write.store, &write.vault, app.as_str())? {
                return Ok(RoutingReadView::blocked(
                    "pending",
                    pending.extra.is_empty().then_some(pending.published),
                ));
            }
            mode
        };
        crate::proxy::application_routing::blocked_tier_ids_checked(db, app.as_str())?;
        let direct_provider_id = provider_for(db, app, Purpose::Direct)?;
        let current_provider_id = provider_for(db, app, Purpose::InUse)?;
        Ok(RoutingReadView {
            status: "ready",
            mode: mode.mode,
            attached: Some(mode.attached),
            current_provider_id,
            direct_provider_id,
            publication_started: None,
            can_write: true,
            can_recheck: true,
            legacy_common_config_writable: false,
        })
    };
    Some(read().unwrap_or_else(|_| RoutingReadView::blocked("unknown", None)))
}
