//! User-choice IPC boundary; the original StartupCoordinator owns readiness.
use super::{Phase, StartupCoordinator};
use crate::secrets::key_store::{KeyStore, SystemKeyStore};
use crate::secrets::{protection, session, storage::StorageIdentity, VaultContext};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::Manager;
use tauri::{AppHandle, Runtime};
use zeroize::Zeroizing;

/// No Debug/Serialize: original input exists only for this invocation.
pub(crate) struct KeyText(Zeroizing<String>);
impl<'de> Deserialize<'de> for KeyText {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|value| Self(Zeroizing::new(value)))
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RequestedMode {
    Encrypted,
    Plaintext,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConfigureRequest {
    mode: RequestedMode,
    key_text: Option<KeyText>,
    preserve_existing: bool,
    plaintext_acknowledged: bool,
    review_token: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProtectionStatus {
    mode: &'static str,
    status: &'static str,
    should_prompt: bool,
    can_configure: bool,
    can_unlock: bool,
    can_switch_to_plaintext: bool,
    can_create_separate_configuration: bool,
    review_token: Option<String>,
    restart_required: bool,
    error_code: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
struct ReviewBinding {
    phase: u8,
    record: Option<Vec<u8>>,
    vault: Option<Vec<u8>>,
    runtime: Option<(StorageIdentity, u64, usize)>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletedAction {
    Unlock,
    PlaintextChoice,
    EncryptedChoice,
    SeparatePlaintext,
    SeparateEncrypted,
}
#[derive(Default)]
pub(super) struct ReviewSlot {
    current: Option<(ReviewBinding, String)>,
    completed: Option<(String, CompletedAction, ReviewBinding)>,
    cache_warning: Option<(StorageIdentity, u64, usize)>,
    restart_error: Option<String>,
}
struct Observation {
    binding: ReviewBinding,
    status: ProtectionStatus,
    saved: Option<session::LocalVault>,
}
fn unavailable() -> String {
    "protection.operation_unavailable".into()
}
fn changed() -> String {
    "protection.review_changed".into()
}
fn public(error: crate::error::AppError) -> String {
    crate::secrets::error::public_code(error)
}

fn observe<R: Runtime>(
    coordinator: &StartupCoordinator,
    app: &AppHandle<R>,
) -> Result<Observation, String> {
    crate::config_file_io::directory_exists_no_alias(&coordinator.root).map_err(public)?;
    let record = protection::read_with_bytes(&coordinator.root).map_err(public)?;
    let local = session::read_metadata_with_bytes(&coordinator.root).map_err(public)?;
    let (saved, vault_bytes) = match local {
        Some((saved, bytes)) => (Some(saved), Some(bytes)),
        None => (None, None),
    };
    if let Some(saved) = &saved {
        saved.metadata.validate().map_err(|e| e.code().to_owned())?;
    }
    if let Some((record, _)) = &record {
        record
            .validate_vault(saved.as_ref().map(|v| &v.metadata))
            .map_err(public)?;
    } else if saved
        .as_ref()
        .is_some_and(|v| v.metadata.user_text_key.is_some())
    {
        return Err("protection.record_missing".into());
    }
    let phase = *coordinator.phase.lock().map_err(|_| unavailable())?;
    let state = app.try_state::<crate::AppState>();
    let available = coordinator
        .runtime_availability(state.as_ref().map(|s| s.db.secret_session()))
        .credential_runtime_available;
    let mut runtime = None;
    if available {
        let state = state.as_ref().ok_or_else(unavailable)?;
        let session = state.db.secret_session();
        if session.root() != coordinator.root {
            return Err(changed());
        }
        let context = session.read().map_err(public)?;
        protection::validate_runtime_storage(&coordinator.root, &context).map_err(public)?;
        runtime = Some((
            context.identity(),
            session.lifecycle_epoch(),
            session as *const _ as usize,
        ));
    }
    let mode = match record.as_ref().map(|(r, _)| &r.storage) {
        Some(protection::ProtectionStorage::Plaintext { .. }) => "plaintext",
        Some(protection::ProtectionStorage::Encrypted { .. }) => "encrypted",
        None if saved.is_some() => "encrypted",
        None => "unconfigured",
    };
    let mut status = ProtectionStatus {
        mode,
        status: if available { "available" } else { "blocked" },
        should_prompt: false,
        can_configure: available,
        can_unlock: false,
        can_switch_to_plaintext: available,
        can_create_separate_configuration: false,
        review_token: None,
        restart_required: matches!(phase, Phase::Failed | Phase::Recovered),
        error_code: None,
    };
    if available {
        let service_complete = crate::settings::get_settings_for_frontend()
            .map_err(public)?
            .service_onboarding_completed;
        status.should_prompt = record
            .as_ref()
            .is_none_or(|(record, _)| record.should_prompt(service_complete));
    } else if phase == Phase::Locked {
        let inspection = coordinator.inspection.lock().map_err(|_| unavailable())?;
        let admission = inspection
            .ensure_runtime_admitted()
            .and_then(|()| inspection.verify_unchanged(&coordinator.root));
        let startup_error = crate::init_status::get_init_error();
        let failed_other_owner = startup_error
            .as_ref()
            .is_some_and(|e| e.kind.as_deref() != Some("secret_locked"));
        if admission.is_ok()
            && !inspection.is_recovery_required()
            && !inspection.is_database_resume_candidate()
            && inspection.future_version().is_none()
            && !failed_other_owner
            && saved
                .as_ref()
                .is_some_and(|v| v.metadata.user_text_key.is_some())
        {
            status.status = "key_required";
            status.can_unlock = true;
        } else {
            status.error_code = Some(match admission {
                Err(e) => public(e),
                Ok(()) if failed_other_owner => "protection.startup_blocked".into(),
                Ok(())
                    if saved
                        .as_ref()
                        .is_some_and(|v| v.metadata.user_text_key.is_none()) =>
                {
                    "protection.legacy_key_recovery_required".into()
                }
                _ => "protection.startup_blocked".into(),
            });
        }
    } else {
        status.error_code = Some("protection.startup_blocked".into());
    }
    if !available && phase == Phase::Locked {
        status.can_create_separate_configuration = coordinator.separate_source().is_ok();
    }
    Ok(Observation {
        binding: ReviewBinding {
            phase: phase as u8,
            record: record.map(|(_, bytes)| bytes),
            vault: vault_bytes,
            runtime,
        },
        status,
        saved,
    })
}
fn status<R: Runtime>(
    coordinator: &StartupCoordinator,
    app: &AppHandle<R>,
) -> Result<ProtectionStatus, String> {
    let mut observed = observe(coordinator, app)?;
    let mut review = coordinator
        .credential_review
        .lock()
        .map_err(|_| unavailable())?;
    if review
        .current
        .as_ref()
        .is_none_or(|(binding, _)| *binding != observed.binding)
    {
        review.current = Some((observed.binding.clone(), uuid::Uuid::new_v4().to_string()));
    }
    if observed.status.can_unlock
        || observed.status.can_create_separate_configuration
        || observed.status.status == "available"
    {
        observed.status.review_token = review.current.as_ref().map(|(_, token)| token.clone());
    }
    if review.cache_warning.is_some() && review.cache_warning == observed.binding.runtime {
        observed.status.error_code = Some("protection.key_cache_unavailable".into());
    }
    if observed.status.restart_required {
        if let Some(error) = &review.restart_error {
            observed.status.error_code = Some(error.clone());
        }
    }
    Ok(observed.status)
}
fn require_review(
    coordinator: &StartupCoordinator,
    observed: &Observation,
    token: &str,
    action: CompletedAction,
) -> Result<bool, String> {
    let review = coordinator
        .credential_review
        .lock()
        .map_err(|_| unavailable())?;
    if review
        .completed
        .as_ref()
        .is_some_and(|(completed, kind, binding)| {
            completed == token && *kind == action && *binding == observed.binding
        })
    {
        return Ok(true);
    }
    match &review.current {
        Some((binding, current)) if current == token && *binding == observed.binding => Ok(false),
        _ => Err(changed()),
    }
}
fn complete<R: Runtime>(
    coordinator: &StartupCoordinator,
    app: &AppHandle<R>,
    token: String,
    action: CompletedAction,
    cache_failed: bool,
) -> Result<ProtectionStatus, String> {
    let observed = observe(coordinator, app)?;
    let mut review = coordinator
        .credential_review
        .lock()
        .map_err(|_| unavailable())?;
    if cache_failed {
        review.cache_warning = observed.binding.runtime.clone();
    }
    review.completed = Some((token, action, observed.binding));
    drop(review);
    status(coordinator, app)
}

/// Dependency carrier only, never a lifecycle or credential-storage owner.
/// Production forwards to the original GUI initialization body; mock runtime
/// tests forward to the same native runtime core without opening desktop windows.
type InitializeRuntime<R> =
    fn(&StartupCoordinator, &AppHandle<R>, Zeroizing<Vec<u8>>) -> Result<(), String>;
pub(crate) struct CommandDependencies<R: Runtime> {
    store: Arc<dyn KeyStore>,
    initialize: InitializeRuntime<R>,
}
impl<R: Runtime> CommandDependencies<R> {
    pub(super) fn key_store(&self) -> &dyn KeyStore {
        &*self.store
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn recovery_test_store(store: Arc<dyn KeyStore>) -> Self {
        Self {
            store,
            initialize: |_, _, _| Err("unexpected runtime initialization".into()),
        }
    }
}
impl CommandDependencies<tauri::Wry> {
    pub(crate) fn production() -> Self {
        Self {
            store: Arc::new(SystemKeyStore),
            initialize: StartupCoordinator::initialize_with_master,
        }
    }
}
#[tauri::command]
pub(crate) async fn get_credential_protection_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ProtectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::app_store::initialize_active_scope().map_err(public)?;
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or_else(unavailable)?;
        let _operation = coordinator
            .credential_operation
            .lock()
            .map_err(|_| unavailable())?;
        status(&coordinator, &app)
    })
    .await
    .map_err(|_| unavailable())?
}
#[tauri::command]
pub(crate) async fn unlock_credential_runtime<R: Runtime>(
    app: AppHandle<R>,
    key_text: KeyText,
    review_token: String,
) -> Result<ProtectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::app_store::initialize_active_scope().map_err(public)?;
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or_else(unavailable)?;
        let _operation = coordinator
            .credential_operation
            .lock()
            .map_err(|_| unavailable())?;
        let observed = observe(&coordinator, &app)?;
        if require_review(
            &coordinator,
            &observed,
            &review_token,
            CompletedAction::Unlock,
        )? {
            return status(&coordinator, &app);
        }
        if !observed.status.can_unlock {
            return Err("protection.unlock_unavailable".into());
        }
        let saved = observed.saved.as_ref().ok_or_else(changed)?;
        let vault = VaultContext::from_user_text(saved.metadata.clone(), &key_text.0)
            .map_err(|e| e.code().to_owned())?;
        // Derivation is intentionally outside any secret-session lock. Recheck
        // the original metadata and reviewed state before original publication.
        if observe(&coordinator, &app)?.binding != observed.binding {
            return Err(changed());
        }
        let dependencies = app.state::<CommandDependencies<R>>();
        let result = (dependencies.initialize)(&coordinator, &app, vault.export_key());
        if let Err(error) = result {
            if !coordinator
                .runtime_availability(
                    app.try_state::<crate::AppState>()
                        .as_ref()
                        .map(|s| s.db.secret_session()),
                )
                .credential_runtime_available
            {
                return Err(
                    if error.starts_with("secret.")
                        || error.starts_with("protection.")
                        || error.starts_with("upgrade.")
                    {
                        error
                    } else {
                        "protection.startup_blocked".into()
                    },
                );
            }
        }
        // Cache denial cannot revoke successful authentication of existing data.
        // No alternative OS access, overwrite, deletion or plaintext fallback.
        let cache_failed = crate::secrets::key_store::save_user_text_verified(
            &*dependencies.store,
            &vault,
            &key_text.0,
        )
        .is_err();
        complete(
            &coordinator,
            &app,
            review_token,
            CompletedAction::Unlock,
            cache_failed,
        )
    })
    .await
    .map_err(|_| unavailable())?
}
#[tauri::command]
pub(crate) async fn configure_credential_protection<R: Runtime>(
    app: AppHandle<R>,
    request: ConfigureRequest,
) -> Result<ProtectionStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::app_store::initialize_active_scope().map_err(public)?;
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or_else(unavailable)?;
        let _operation = coordinator
            .credential_operation
            .lock()
            .map_err(|_| unavailable())?;
        match request.mode {
            RequestedMode::Plaintext
                if !request.plaintext_acknowledged || request.key_text.is_some() =>
            {
                return Err("protection.explicit_plaintext_required".into())
            }
            RequestedMode::Encrypted
                if request.key_text.as_ref().is_none_or(|key| key.0.is_empty()) =>
            {
                return Err("protection.key_input_required".into())
            }
            _ => {}
        }
        let action = match (request.preserve_existing, request.mode) {
            (true, RequestedMode::Plaintext) => CompletedAction::SeparatePlaintext,
            (true, RequestedMode::Encrypted) => CompletedAction::SeparateEncrypted,
            (false, RequestedMode::Plaintext) => CompletedAction::PlaintextChoice,
            (false, RequestedMode::Encrypted) => CompletedAction::EncryptedChoice,
        };
        let observed = observe(&coordinator, &app)?;
        if require_review(&coordinator, &observed, &request.review_token, action)? {
            return status(&coordinator, &app);
        }
        if request.preserve_existing {
            if !observed.status.can_create_separate_configuration {
                return Err("protection.separate_configuration_unavailable".into());
            }
        } else if match request.mode {
            RequestedMode::Plaintext => !observed.status.can_switch_to_plaintext,
            RequestedMode::Encrypted => !observed.status.can_configure,
        } {
            return Err("protection.conversion_unavailable".into());
        }
        if !request.preserve_existing
            && request.mode == RequestedMode::Plaintext
            && observed.status.mode == "plaintext"
        {
            let state = app.try_state::<crate::AppState>().ok_or_else(unavailable)?;
            protection::complete_plaintext_choice(
                &state.db,
                observed.binding.record.as_deref().ok_or_else(changed)?,
            )
            .map_err(public)?;
            return complete(&coordinator, &app, request.review_token, action, false);
        }
        let selection = protection::generation::capture_current_selection().map_err(public)?;
        let mode = match request.mode {
            RequestedMode::Plaintext => protection::generation::GenerationMode::Plaintext,
            RequestedMode::Encrypted => protection::generation::GenerationMode::Encrypted(
                request.key_text.ok_or_else(changed)?.0,
            ),
        };
        let dependencies = app.state::<CommandDependencies<R>>();
        let outcome = if request.preserve_existing {
            protection::generation::create_separate_at(
                &coordinator,
                selection,
                mode,
                &*dependencies.store,
            )
        } else {
            let state = app.try_state::<crate::AppState>().ok_or_else(unavailable)?;
            protection::generation::convert_at(
                &state.db,
                &crate::config::get_device_config_dir(),
                selection,
                mode,
                &*dependencies.store,
            )
        }
        .map_err(public)?;
        let failure = match outcome {
            crate::app_store::isolated_scope::PublicationOutcome::NotPublished(error) => {
                return Err(public(error))
            }
            crate::app_store::isolated_scope::PublicationOutcome::PossiblyPublished(error) => {
                Some(public(error))
            }
            crate::app_store::isolated_scope::PublicationOutcome::RestartRequired(scope) => {
                log::info!(
                    "Credential storage generation {} published; restart required",
                    scope.root_id()
                );
                None
            }
        };
        // Selector publication never hot-switches cached paths or reopens the old DB.
        // The original coordinator publishes only a restart-required shell now.
        *coordinator.phase.lock().map_err(|_| unavailable())? = Phase::Recovered;
        coordinator
            .credential_review
            .lock()
            .map_err(|_| unavailable())?
            .restart_error = failure;
        complete(&coordinator, &app, request.review_token, action, false)
    })
    .await
    .map_err(|_| unavailable())?
}
#[cfg(any(test, feature = "test-hooks"))]
#[path = "credential_protection_tests.rs"]
pub(crate) mod tests;
