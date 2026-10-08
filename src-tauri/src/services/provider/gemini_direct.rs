//! Gemini's upstream direct writer, adapted to LoongPort's authenticated mode
//! transaction and current-pointer owners. Ordinary schema17 startup remains
//! unchanged; schema20 publication/admission is owned by the upgrade lifecycle.

use crate::app_config::AppType;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::gemini_config::{
    get_gemini_env_path, get_gemini_settings_path, validate_gemini_settings,
    validate_gemini_settings_strict,
};
use crate::live::engine::{lock_app, DeviceStore, LiveFile};
use crate::live::project::gemini::GeminiProjection;
use crate::mode::operation::{self, FileChange, OperationReport, RecoveryOutcome};
use crate::mode::state::{self, op, Mode, PendingTarget};
use crate::provider::Provider;
use crate::secrets::{session::SecretSession, VaultContext};
use crate::store::AppState;
use std::sync::RwLockReadGuard;

use super::gemini_auth::is_google_official_gemini;

fn app() -> &'static str {
    AppType::Gemini.as_str()
}

/// The persisted schema is the existing migration fact, not a second feature flag.
pub(crate) fn uses_upstream4_schema(db: &Database) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);
    let version = Database::get_user_version(&conn)?;
    let modern = uses_upstream4_version(version, crate::database::SCHEMA_VERSION)?;
    if modern
        && crate::database::loongport_schema::read_stored_version(&conn)?
            != crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
    {
        return Err(AppError::Config("upgrade.future_version".into()));
    }
    Ok(modern)
}

// Pure comparison keeps the active startup ceiling distinct from the target
// migration version so the transition can be tested before ordinary activation.
pub(super) fn uses_upstream4_version(version: i32, supported: i32) -> Result<bool, AppError> {
    if version == crate::database::UPSTREAM4_SCHEMA_VERSION {
        return Ok(true);
    }
    if version <= supported && version < crate::database::UPSTREAM4_SCHEMA_VERSION {
        return Ok(false);
    }
    Err(AppError::Config("upgrade.future_version".into()))
}

pub(crate) fn env_file() -> LiveFile {
    LiveFile::private(get_gemini_env_path())
}

pub(crate) fn settings_file() -> LiveFile {
    LiveFile::shared(get_gemini_settings_path())
}

pub(crate) fn is_official(provider: &Provider) -> bool {
    provider.category.as_deref() == Some("official") || is_google_official_gemini(provider)
}

pub(crate) fn projection(provider: &Provider) -> Result<GeminiProjection, AppError> {
    validate_gemini_settings(&provider.settings_config)?;
    let official = is_official(provider);
    if !official {
        validate_gemini_settings_strict(&provider.settings_config)?;
    }
    Ok(GeminiProjection::of(&provider.settings_config, official))
}

fn validate_direct_mode(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
) -> Result<(), AppError> {
    let mut entry = state::load(store, vault)?
        .apps
        .get(app())
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

fn ensure_no_legacy_takeover(state: &AppState) -> Result<(), AppError> {
    // Admission must preserve authentication/read failures. The presentation
    // helper intentionally projects errors to false and cannot authorize writes.
    // This DAO acquires its vault guard, so run it before pinning our operation's
    // guard; otherwise a queued key writer could deadlock a recursive read.
    let backup = futures::executor::block_on(state.db.get_live_backup(app()))?;
    if backup.is_some()
        || state
            .proxy_service
            .detect_takeover_in_live_config_for_app(&AppType::Gemini)
    {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    Ok(())
}

fn commit_target(
    db: &Database,
    session: &SecretSession,
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    target: &PendingTarget,
) -> Result<(), AppError> {
    // This direct writer owns only the pointer. Mode transitions and Stack are
    // handled by their own approved controller paths, never inferred here.
    if target.state.is_some()
        || target.written.is_some()
        || target.stack.is_some()
        || !target.extra.is_empty()
    {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    validate_direct_mode(store, vault)?;
    if let Some(id) = target.pointer.as_deref() {
        let exists: bool = lock_conn!(db.conn).query_row(
            "SELECT EXISTS(SELECT 1 FROM providers WHERE app_type=?1 AND id=?2)",
            rusqlite::params![app(), id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AppError::Config("mode.verification_required".into()));
        }
        crate::settings::set_current_provider_with_vault(
            &AppType::Gemini,
            Some(id),
            session,
            vault,
        )?;
        db.set_current_provider(app(), id)?;
        crate::proxy::auto_strategy::set_model_pref(db, app(), None)?;
        if db.get_current_provider(app())?.as_deref() != Some(id)
            || crate::settings::get_current_provider(&AppType::Gemini).as_deref() != Some(id)
        {
            return Err(AppError::Config("mode.verification_required".into()));
        }
    }
    validate_direct_mode(store, vault)
}

/// Caller owns the existing application switch lock. Reuse the upstream
/// projectors and two-file operation; no common-fragment merge or live backfill.
pub(crate) fn switch_to(state: &AppState, target: &Provider) -> Result<OperationReport, AppError> {
    let projection = projection(target)?;
    ensure_no_legacy_takeover(state)?;
    let store = DeviceStore::for_device();
    let guard = lock_app(app());
    let session = state.db.secret_session();
    let vault = session.read()?;
    crate::secrets::upgrade::checkpoint::ensure_sync_admitted(&store)?;
    validate_direct_mode(&store, &vault)?;
    // An unresolved operation needs explicit recovery/query, not another switch.
    if state::pending(&store, &vault, app())?.is_some() {
        return Err(AppError::Config("mode.verification_required".into()));
    }
    let env = projection.env_patch();
    let settings = projection.settings_patch();
    operation::run(
        &store,
        &vault,
        &guard,
        op::SWITCH,
        &[
            FileChange {
                file: env_file(),
                patch: &env,
            },
            FileChange {
                file: settings_file(),
                patch: &settings,
            },
        ],
        PendingTarget::pointer(Some(target.id.clone())),
        &|target| commit_target(&state.db, session, &store, &vault, target),
    )
}

/// Explicit recovery of the original operation. It is not invoked by reads or
/// registered with startup/GUI until complete upgrade admission is enabled.
#[allow(dead_code)] // Remove when the controlled startup/GUI recovery caller is connected.
pub(crate) fn recover_pending(state: &AppState) -> Result<Option<RecoveryOutcome>, AppError> {
    if !uses_upstream4_schema(&state.db)? {
        return Err(AppError::Config("upgrade.migration_required".into()));
    }
    let _switch = futures::executor::block_on(state.proxy_service.lock_switch_for_app(app()));
    ensure_no_legacy_takeover(state)?;
    let store = DeviceStore::for_device();
    let guard = lock_app(app());
    let session = state.db.secret_session();
    let vault = session.read()?;
    crate::secrets::upgrade::checkpoint::ensure_sync_admitted(&store)?;
    validate_direct_mode(&store, &vault)?;
    operation::recover(
        &store,
        &vault,
        &guard,
        &[env_file(), settings_file()],
        &|target| commit_target(&state.db, session, &store, &vault, target),
    )
}
