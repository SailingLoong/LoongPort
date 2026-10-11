//! Unlock owns runtime publication; credential consumers never start the lifecycle.

#[cfg(feature = "gui")]
use super::key_store::SystemKeyStore;
use super::session::SecretSession;
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
#[cfg(feature = "gui")]
use tauri::{Emitter, Manager};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Locked,
    UpgradeReview,
    Initializing,
    Ready,
    Failed,
    Recovered,
}

pub(crate) struct StartupCoordinator {
    root: PathBuf,
    inspection: Mutex<super::upgrade::UpgradeInspection>,
    recovery_token: OnceLock<String>,
    phase: Mutex<Phase>,
    upgrade_review: Mutex<Option<super::upgrade::AuthenticatedUpgrade>>,
}

impl StartupCoordinator {
    pub(crate) fn new(root: PathBuf, inspection: super::upgrade::UpgradeInspection) -> Self {
        let recovery_token = OnceLock::new();
        if let super::upgrade::UpgradeInspection::RecoveryRequired(evidence) = &inspection {
            let _ = recovery_token.set(evidence.token());
        }
        Self {
            root,
            recovery_token,
            inspection: Mutex::new(inspection),
            phase: Mutex::new(Phase::Locked),
            upgrade_review: Mutex::new(None),
        }
    }

    #[cfg(feature = "gui")]
    fn initialize(&self, app: &tauri::AppHandle, password: Option<&str>) -> Result<(), String> {
        self.initialize_with(password, &SystemKeyStore, |session| {
            crate::initialize_runtime(app, session).map_err(|error| error.to_string())
        })?;
        // Publish readiness only after the original coordinator has reached Ready.
        crate::init_status::clear_init_error();
        let _ = app.emit("runtime-ready", ());
        Ok(())
    }

    fn initialize_with<P>(
        &self,
        password: Option<&str>,
        store: &dyn super::key_store::KeyStore,
        prepare: P,
    ) -> Result<(), String>
    where
        P: FnOnce(std::sync::Arc<SecretSession>) -> Result<(), String>,
    {
        let resume = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .is_database_resume_candidate();
        if resume {
            let phase = *self
                .phase
                .lock()
                .map_err(|_| "secret.startup_unavailable")?;
            let view = match phase {
                Phase::Locked => self.authenticate_upgrade(password, store)?,
                Phase::UpgradeReview | Phase::Ready => self.upgrade_view()?,
                Phase::Initializing => return Err("secret.initializing".into()),
                Phase::Failed | Phase::Recovered => return Err("secret.restart_required".into()),
            };
            let token = view.review_token.ok_or("secret.locked")?;
            return self.run_upgrade_attempt(&token, prepare);
        }
        self.run_attempt(
            || {
                let inspection = self
                    .inspection
                    .lock()
                    .map_err(|_| "secret.startup_unavailable")?;
                inspection
                    .verify_unchanged(&self.root)
                    .map_err(super::error::public_code)?;
                if inspection.has_vault() {
                    let vault = super::session::authenticate_existing(&self.root, store, password)
                        .map_err(super::error::public_code)?;
                    inspection
                        .validate_device_state(&vault)
                        .map_err(super::error::public_code)?;
                    inspection
                        .validate_database(&self.root, &vault)
                        .map_err(super::error::public_code)?;
                    inspection
                        .verify_unchanged(&self.root)
                        .map_err(super::error::public_code)?;
                } else if inspection.has_device_files() {
                    return Err("secret.metadata_missing".into());
                }
                super::reset::recover(&self.root, password).map_err(super::error::public_code)?;
                super::bootstrap_restore::recover(&self.root, store, password)
                    .map_err(super::error::public_code)?;
                crate::settings::reload_settings().map_err(super::error::public_code)?;
                crate::settings::bootstrap_settings().map_err(super::error::public_code)?;
                crate::database::vault::preflight(&self.root.join(crate::config::DB_FILE_NAME))
                    .map_err(super::error::public_code)?;
                SecretSession::open(&self.root, store, password).map_err(super::error::public_code)
            },
            prepare,
        )
    }

    fn run_attempt<U, P>(&self, unlock: U, prepare: P) -> Result<(), String>
    where
        U: FnOnce() -> Result<std::sync::Arc<SecretSession>, String>,
        P: FnOnce(std::sync::Arc<SecretSession>) -> Result<(), String>,
    {
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        match *phase {
            Phase::Ready => return Ok(()),
            Phase::Locked => {}
            Phase::Initializing | Phase::UpgradeReview => return Err("secret.initializing".into()),
            Phase::Failed | Phase::Recovered => return Err("secret.restart_required".into()),
        }
        self.inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .ensure_runtime_admitted()
            .map_err(super::error::public_code)?;
        let session = unlock()?;
        *phase = Phase::Initializing;
        drop(phase);
        let result = prepare(session);
        *self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")? = if result.is_ok() {
            Phase::Ready
        } else {
            Phase::Failed
        };
        result
    }

    fn run_upgrade_attempt<P>(&self, token: &str, prepare: P) -> Result<(), String>
    where
        P: FnOnce(std::sync::Arc<SecretSession>) -> Result<(), String>,
    {
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        match *phase {
            Phase::UpgradeReview | Phase::Ready => {}
            Phase::Initializing => return Err("secret.initializing".into()),
            Phase::Failed | Phase::Recovered => return Err("secret.restart_required".into()),
            Phase::Locked => return Err("secret.locked".into()),
        }
        let session = {
            let inspection = self
                .inspection
                .lock()
                .map_err(|_| "secret.startup_unavailable")?;
            self.upgrade_review
                .lock()
                .map_err(|_| "secret.startup_unavailable")?
                .as_ref()
                .ok_or("secret.locked")?
                .runtime_session(&inspection, token)
                .map_err(super::error::public_code)?
        };
        // A repeated explicit request verifies the retained evidence but does not
        // prepare the process twice. The existing phase owns publication.
        if *phase == Phase::Ready {
            return Ok(());
        }
        *phase = Phase::Initializing;
        drop(phase);
        let result = prepare(session);
        *self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")? = if result.is_ok() {
            Phase::Ready
        } else {
            Phase::Failed
        };
        result
    }

    pub(super) fn upgrade_view(&self) -> Result<super::upgrade::StartupUpgradeView, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let blocked = match *phase {
            Phase::Ready => {
                if self
                    .upgrade_review
                    .lock()
                    .map_err(|_| "secret.startup_unavailable")?
                    .is_none()
                {
                    Some("runtime_active")
                } else {
                    None
                }
            }
            Phase::Initializing => Some("busy"),
            Phase::Failed | Phase::Recovered => Some("restart_required"),
            Phase::Locked | Phase::UpgradeReview => None,
        };
        if let Some(status) = blocked {
            return Ok(super::upgrade::StartupUpgradeView::blocked(status));
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if matches!(*phase, Phase::UpgradeReview | Phase::Ready) {
            return self
                .upgrade_review
                .lock()
                .map_err(|_| "secret.startup_unavailable")?
                .as_ref()
                .ok_or("secret.startup_unavailable")?
                .view(&inspection)
                .map_err(super::error::public_code);
        }
        let mut view = inspection
            .upgrade_view(&self.root)
            .map_err(super::error::public_code)?;
        view.can_authenticate = matches!(
            view.status,
            "authentication_required" | "checkpoint_requires_verification"
        );
        Ok(view)
    }

    pub(super) fn authenticate_upgrade(
        &self,
        password: Option<&str>,
        store: &dyn super::key_store::KeyStore,
    ) -> Result<super::upgrade::StartupUpgradeView, String> {
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::Locked {
            return Err("secret.initializing".into());
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let review = super::upgrade::AuthenticatedUpgrade::authenticate(
            &self.root,
            &crate::live::engine::DeviceStore::for_device(),
            &inspection,
            store,
            password,
        )
        .map_err(super::error::public_code)?;
        let view = review
            .view(&inspection)
            .map_err(super::error::public_code)?;
        *self
            .upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")? = Some(review);
        *phase = Phase::UpgradeReview;
        Ok(view)
    }

    pub(super) fn prepare_upgrade_checkpoint(
        &self,
        token: &str,
    ) -> Result<super::upgrade::StartupUpgradeView, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::UpgradeReview {
            return Err("secret.locked".into());
        }
        let mut inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        self.upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .as_mut()
            .ok_or("secret.startup_unavailable")?
            .prepare_checkpoint(&mut inspection, token)
            .map_err(super::error::public_code)
    }

    pub(super) fn cancel_upgrade_checkpoint(
        &self,
        token: &str,
        id: &str,
    ) -> Result<super::upgrade::StartupUpgradeView, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::UpgradeReview {
            return Err("secret.locked".into());
        }
        let mut inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        self.upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .as_mut()
            .ok_or("secret.startup_unavailable")?
            .cancel_checkpoint(&mut inspection, token, id)
            .map_err(super::error::public_code)
    }

    pub(super) fn publish_upgrade_checkpoint(
        &self,
        token: &str,
        id: &str,
        store: &dyn super::key_store::KeyStore,
    ) -> Result<super::upgrade::StartupUpgradeView, String> {
        self.publish_upgrade_checkpoint_with_hook(token, id, store, None)
    }

    pub(super) fn publish_upgrade_checkpoint_with_hook(
        &self,
        token: &str,
        id: &str,
        store: &dyn super::key_store::KeyStore,
        hook: Option<
            &mut dyn FnMut(super::transition::Checkpoint) -> Result<(), crate::error::AppError>,
        >,
    ) -> Result<super::upgrade::StartupUpgradeView, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::UpgradeReview {
            return Err("secret.locked".into());
        }
        let mut inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let mut review = self
            .upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let review = review.as_mut().ok_or("secret.startup_unavailable")?;
        let result = match hook {
            Some(hook) => {
                review.publish_checkpoint_with_hook(&mut inspection, token, id, store, hook)
            }
            None => review.publish_checkpoint(&mut inspection, token, id, store),
        };
        if let super::upgrade::UpgradeInspection::RecoveryRequired(evidence) = &*inspection {
            // This evidence contains the actual intent retained by the original
            // publication owner, even when current journal bytes differ.
            let _ = self.recovery_token.set(evidence.token());
        }
        result.map_err(super::error::public_code)
    }

    pub(super) fn review_upgrade_ownership(
        &self,
        token: &str,
    ) -> Result<super::upgrade::StagedUpgradeReview, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::UpgradeReview {
            return Err("secret.locked".into());
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        self.upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .as_ref()
            .ok_or("secret.startup_unavailable")?
            .stage_review(&inspection, token)
            .map_err(super::error::public_code)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn review_upgrade_app(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        self.review_upgrade_app_with_state(token, app, None)
    }

    pub(super) fn review_upgrade_app_with_state(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        state: Option<&crate::store::AppState>,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if !matches!(*phase, Phase::UpgradeReview | Phase::Ready) {
            return Err("secret.locked".into());
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let review = self
            .upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let review = review.as_ref().ok_or("secret.startup_unavailable")?;
        let mut view = match state.filter(|_| *phase == Phase::Ready) {
            Some(state) => review.review_app_with_state(&inspection, token, app, state),
            None => review.review_app(&inspection, token, app),
        }
        .map_err(super::error::public_code)?;
        if *phase != Phase::Ready {
            view.can_choose_provider = false;
            view.can_choose_mode = false;
        }
        Ok(view)
    }

    pub(super) fn select_upgrade_provider(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        revision: &str,
        provider_id: &str,
        state: &crate::store::AppState,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::Ready {
            return Err("secret.locked".into());
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        self.upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .as_ref()
            .ok_or("secret.startup_unavailable")?
            .select_provider(&inspection, token, app, revision, provider_id, state)
            .map_err(super::error::public_code)
    }

    pub(super) fn select_upgrade_mode(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        revision: &str,
        choice: &super::upgrade::UpgradeModeChoice,
        state: &crate::store::AppState,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::Ready {
            return Err("secret.locked".into());
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        self.upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .as_ref()
            .ok_or("secret.startup_unavailable")?
            .select_mode(&inspection, token, app, revision, choice, state)
            .map_err(super::error::public_code)
    }

    pub(super) fn recover_upgrade_app(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        revision: &str,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        self.recover_upgrade_app_with_runtime(token, app, revision, None)
    }

    pub(super) fn recover_upgrade_app_with_state(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        revision: &str,
        state: &crate::store::AppState,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        self.recover_upgrade_app_with_runtime(token, app, revision, Some(state))
    }

    fn recover_upgrade_app_with_runtime(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        revision: &str,
        state: Option<&crate::store::AppState>,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if state.is_some() && *phase != Phase::Ready {
            return Err("secret.locked".into());
        }
        if !matches!(*phase, Phase::UpgradeReview | Phase::Ready) {
            return Err("secret.locked".into());
        }
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let review = self
            .upgrade_review
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let review = review.as_ref().ok_or("secret.startup_unavailable")?;
        match state {
            Some(state) => review.recover_app_with_state(&inspection, token, app, revision, state),
            None => review.recover_app(&inspection, token, app, revision),
        }
        .map_err(super::error::public_code)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(super) fn verify_runtime_admission_blocked(&self) -> bool {
        self.run_attempt(
            || panic!("review must prevent ordinary unlock"),
            |_| panic!("review must prevent runtime preparation"),
        )
        .is_err()
    }

    fn restart_required(&self) -> bool {
        self.phase
            .lock()
            .map(|phase| matches!(*phase, Phase::Failed | Phase::Recovered))
            .unwrap_or(true)
    }

    pub(super) fn recovery_view(&self) -> Result<StartupRecoveryView, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let token = self
            .recovery_token
            .get()
            .cloned()
            .ok_or("secret.no_pending_operation")?;
        let (status, can_recover, restart_required) = match (&*inspection, *phase) {
            (
                super::upgrade::UpgradeInspection::RecoveryRequired(evidence),
                Phase::Locked | Phase::UpgradeReview,
            ) if evidence.verify_unchanged(&self.root, &token).is_ok() => ("pending", true, false),
            (super::upgrade::UpgradeInspection::Stable(_), Phase::Recovered)
                if inspection.verify_unchanged(&self.root).is_ok() =>
            {
                ("completed", false, true)
            }
            _ => ("verification_required", false, true),
        };
        Ok(StartupRecoveryView {
            token,
            status,
            can_recover,
            restart_required,
        })
    }

    pub(super) fn recover_operation(
        &self,
        token: &str,
        password: &str,
        store: &dyn super::key_store::KeyStore,
    ) -> Result<StartupRecoveryView, String> {
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if !matches!(*phase, Phase::Locked | Phase::UpgradeReview) {
            return Err("secret.restart_required".into());
        }
        if self.recovery_token.get().map(String::as_str) != Some(token) {
            return Err("upgrade.source_changed".into());
        }
        let mut inspection = self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let super::upgrade::UpgradeInspection::RecoveryRequired(evidence) = &*inspection else {
            return Err("secret.no_pending_operation".into());
        };
        evidence
            .verify_unchanged(&self.root, token)
            .map_err(super::error::public_code)?;
        *phase = Phase::Initializing;
        match evidence.recover(&self.root, token, store, password) {
            Ok(verified) => {
                *inspection = verified;
                // Recovery can replace the root. Restart opens logging and runtime
                // handles only after the settled generation is inspected again.
                *phase = Phase::Recovered;
            }
            Err(error) => {
                *phase = if evidence.verify_unchanged(&self.root, token).is_ok() {
                    Phase::Locked
                } else {
                    Phase::Failed
                };
                return Err(super::error::public_code(error));
            }
        }
        drop(inspection);
        drop(phase);
        self.recovery_view()
    }

    fn ensure_no_pending_recovery(&self) -> Result<(), String> {
        if self
            .inspection
            .lock()
            .map_err(|_| "secret.startup_unavailable")?
            .is_recovery_required()
        {
            Err("secret.recovery_required".into())
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartupRecoveryView {
    token: String,
    status: &'static str,
    can_recover: bool,
    restart_required: bool,
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn get_startup_upgrade_review(
    app: tauri::AppHandle,
) -> Result<super::upgrade::StartupUpgradeView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(coordinator) = app.try_state::<StartupCoordinator>() {
            coordinator.upgrade_view()
        } else {
            let initialization = crate::init_status::get_init_error();
            Err(super::upgrade::startup_upgrade_unavailable(
                initialization
                    .as_ref()
                    .and_then(|error| error.kind.as_deref()),
            )
            .to_owned())
        }
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn authenticate_startup_upgrade(
    app: tauri::AppHandle,
    password: Option<String>,
) -> Result<super::upgrade::StartupUpgradeView, String> {
    let password = password.map(zeroize::Zeroizing::new);
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.authenticate_upgrade(
            password.as_ref().map(|value| value.as_str()),
            &SystemKeyStore,
        )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn prepare_startup_upgrade_checkpoint(
    app: tauri::AppHandle,
    expected_review_token: String,
) -> Result<super::upgrade::StartupUpgradeView, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.prepare_upgrade_checkpoint(&expected_review_token)
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn cancel_startup_upgrade_checkpoint(
    app: tauri::AppHandle,
    expected_review_token: String,
    expected_checkpoint_id: String,
) -> Result<super::upgrade::StartupUpgradeView, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.cancel_upgrade_checkpoint(&expected_review_token, &expected_checkpoint_id)
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn publish_startup_upgrade_checkpoint(
    app: tauri::AppHandle,
    expected_review_token: String,
    expected_checkpoint_id: String,
) -> Result<super::upgrade::StartupUpgradeView, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        app.try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?
            .publish_upgrade_checkpoint(
                &expected_review_token,
                &expected_checkpoint_id,
                &SystemKeyStore,
            )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn continue_startup_upgrade(
    app: tauri::AppHandle,
    expected_review_token: String,
) -> Result<(), String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.run_upgrade_attempt(&expected_review_token, |session| {
            crate::initialize_runtime(&app, session).map_err(|error| error.to_string())
        })?;
        // Publish GUI readiness only after the original phase has become Ready.
        crate::init_status::clear_init_error();
        let _ = app.emit("runtime-ready", ());
        Ok(())
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn review_startup_upgrade_ownership(
    app: tauri::AppHandle,
    expected_review_token: String,
) -> Result<super::upgrade::StagedUpgradeReview, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.review_upgrade_ownership(&expected_review_token)
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn review_startup_upgrade_app(
    app: tauri::AppHandle,
    expected_review_token: String,
    app_type: crate::app_config::AppType,
) -> Result<super::upgrade::UpgradeAppReview, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        app.try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?
            .review_upgrade_app_with_state(
                &expected_review_token,
                &app_type,
                app.try_state::<crate::store::AppState>().as_deref(),
            )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn select_startup_upgrade_provider(
    app: tauri::AppHandle,
    expected_review_token: String,
    app_type: crate::app_config::AppType,
    expected_app_revision: String,
    provider_id: String,
) -> Result<super::upgrade::UpgradeAppReview, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        let state = app
            .try_state::<crate::store::AppState>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.select_upgrade_provider(
            &expected_review_token,
            &app_type,
            &expected_app_revision,
            &provider_id,
            &state,
        )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn select_startup_upgrade_mode(
    app: tauri::AppHandle,
    expected_review_token: String,
    app_type: crate::app_config::AppType,
    expected_app_revision: String,
    choice: super::upgrade::UpgradeModeChoice,
) -> Result<super::upgrade::UpgradeAppReview, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        let state = app
            .try_state::<crate::store::AppState>()
            .ok_or("secret.startup_unavailable")?;
        coordinator.select_upgrade_mode(
            &expected_review_token,
            &app_type,
            &expected_app_revision,
            &choice,
            &state,
        )
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn recover_startup_upgrade_app(
    app: tauri::AppHandle,
    expected_review_token: String,
    app_type: crate::app_config::AppType,
    expected_app_revision: String,
) -> Result<super::upgrade::UpgradeAppReview, String> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app
            .try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?;
        if let Some(state) = app.try_state::<crate::store::AppState>() {
            coordinator.recover_upgrade_app_with_state(
                &expected_review_token,
                &app_type,
                &expected_app_revision,
                &state,
            )
        } else {
            coordinator.recover_upgrade_app(
                &expected_review_token,
                &app_type,
                &expected_app_revision,
            )
        }
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn get_startup_recovery(
    app: tauri::AppHandle,
) -> Result<StartupRecoveryView, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<StartupCoordinator>().recovery_view())
        .await
        .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn recover_startup_operation(
    app: tauri::AppHandle,
    token: String,
    password: String,
) -> Result<StartupRecoveryView, StartupError> {
    let password = zeroize::Zeroizing::new(password);
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app.state::<StartupCoordinator>();
        coordinator
            .recover_operation(&token, &password, &SystemKeyStore)
            .map_err(|code| StartupError {
                code,
                restart_required: coordinator.restart_required(),
            })
    })
    .await
    .map_err(|_| StartupError {
        code: "secret.operation_failed".into(),
        restart_required: true,
    })?
}

#[cfg(feature = "gui")]
fn prepare_runtime(
    app: &tauri::AppHandle,
    session: std::sync::Arc<SecretSession>,
) -> Result<(), String> {
    crate::initialize_runtime(app, session).map_err(|e| e.to_string())?;
    crate::init_status::clear_init_error();
    let _ = app.emit("runtime-ready", ());
    Ok(())
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn preview_startup_restore(
    app: tauri::AppHandle,
    source: super::bootstrap_restore::RestoreSource,
) -> Result<super::bootstrap_restore::RestorePreview, String> {
    {
        let coordinator = app.state::<StartupCoordinator>();
        coordinator.ensure_no_pending_recovery()?;
        let mut phase = coordinator
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable".to_owned())?;
        if *phase != Phase::Locked {
            return Err("secret.initializing".into());
        }
        *phase = Phase::Initializing;
    }
    let result = super::bootstrap_restore::preview(&source)
        .await
        .map_err(super::error::public_code);
    let coordinator = app.state::<StartupCoordinator>();
    *coordinator
        .phase
        .lock()
        .map_err(|_| "secret.startup_unavailable".to_owned())? = Phase::Locked;
    result
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn restore_startup_vault(
    app: tauri::AppHandle,
    source: super::bootstrap_restore::RestoreSource,
    password: String,
    expected_snapshot_id: String,
    automatic_unlock: bool,
) -> Result<(), StartupError> {
    let password = zeroize::Zeroizing::new(password);
    let root = {
        let coordinator = app.state::<StartupCoordinator>();
        coordinator
            .ensure_no_pending_recovery()
            .map_err(|code| StartupError {
                code,
                restart_required: false,
            })?;
        let mut phase = coordinator.phase.lock().map_err(|_| StartupError {
            code: "secret.startup_unavailable".into(),
            restart_required: true,
        })?;
        if *phase != Phase::Locked {
            return Err(StartupError {
                code: "secret.restart_required".into(),
                restart_required: *phase == Phase::Failed,
            });
        }
        *phase = Phase::Initializing;
        coordinator.root.clone()
    };
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    let result =
        match super::bootstrap_restore::prepare(source, &password, &expected_snapshot_id).await {
            Err(error) => Err(StartupError {
                code: super::error::public_code(error),
                restart_required: false,
            }),
            Ok(prepared) => {
                let runtime_app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let session = super::bootstrap_restore::finalize(
                        &root,
                        prepared,
                        &SystemKeyStore,
                        automatic_unlock,
                    )
                    .map_err(|error| StartupError {
                        code: super::error::public_code(error),
                        restart_required: super::bootstrap_restore::pending(&root).unwrap_or(true),
                    })?;
                    prepare_runtime(&runtime_app, session).map_err(|code| StartupError {
                        code,
                        restart_required: true,
                    })
                })
                .await
                .unwrap_or_else(|_| {
                    Err(StartupError {
                        code: "secret.operation_failed".into(),
                        restart_required: true,
                    })
                })
            }
        };
    let coordinator = app.state::<StartupCoordinator>();
    *coordinator.phase.lock().map_err(|_| StartupError {
        code: "secret.startup_unavailable".into(),
        restart_required: true,
    })? = match &result {
        Ok(()) => Phase::Ready,
        Err(error) if error.restart_required => Phase::Failed,
        Err(_) => Phase::Locked,
    };
    if let Err(error) = &result {
        present_error(&app, &error.code);
    }
    result
}

#[cfg(feature = "gui")]
fn present_error(app: &tauri::AppHandle, error: &str) {
    log::error!("启动解锁失败（secret startup failed）: {error}");
    crate::init_status::set_init_error(crate::init_status::InitErrorPayload {
        path: String::new(),
        error: error.to_owned(),
        kind: Some(
            if app.state::<StartupCoordinator>().restart_required() {
                "secret_initialization_failed"
            } else {
                "secret_locked"
            }
            .into(),
        ),
        db_version: None,
        supported_version: None,
    });
    if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(feature = "gui")]
pub(crate) fn try_automatic_unlock(app: &tauri::AppHandle) {
    let coordinator = app.state::<StartupCoordinator>();
    if let Err(error) = coordinator.initialize(app, None) {
        present_error(app, &error);
    }
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn unlock_secret_vault(
    app: tauri::AppHandle,
    password: Option<String>,
) -> Result<(), StartupError> {
    let password = password.map(zeroize::Zeroizing::new);
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app.state::<StartupCoordinator>();
        coordinator
            .initialize(&app, password.as_ref().map(|p| p.as_str()))
            .map_err(|code| {
                present_error(&app, &code);
                StartupError {
                    code,
                    restart_required: coordinator.restart_required(),
                }
            })
    })
    .await
    .map_err(|_| StartupError {
        code: "secret.operation_failed".into(),
        restart_required: true,
    })?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn preview_secret_reset(
    app: tauri::AppHandle,
) -> Result<super::reset::ResetPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app.state::<StartupCoordinator>();
        coordinator.ensure_no_pending_recovery()?;
        let phase = coordinator
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable".to_owned())?;
        if *phase != Phase::Locked {
            return Err("secret.restart_required".into());
        }
        super::reset::preview(&coordinator.root).map_err(super::error::public_code)
    })
    .await
    .map_err(|_| "secret.operation_failed".to_owned())?
}

#[cfg(feature = "gui")]
#[tauri::command]
pub(crate) async fn reset_secret_vault(
    app: tauri::AppHandle,
    fingerprint: String,
    password: String,
) -> Result<String, StartupError> {
    let password = zeroize::Zeroizing::new(password);
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app.state::<StartupCoordinator>();
        coordinator
            .ensure_no_pending_recovery()
            .map_err(|code| StartupError {
                code,
                restart_required: false,
            })?;
        let mut phase = coordinator.phase.lock().map_err(|_| StartupError {
            code: "secret.startup_unavailable".into(),
            restart_required: true,
        })?;
        if *phase != Phase::Locked {
            return Err(StartupError {
                code: "secret.restart_required".into(),
                restart_required: true,
            });
        }
        let result = super::reset::reset(&coordinator.root, &fingerprint, &password);
        match result {
            Ok(archive) => {
                // Reset replaces the entire owned tree; reopen process-owned
                // logging handles on restart before publishing normal services.
                *phase = Phase::Failed;
                Ok(archive.to_string_lossy().into_owned())
            }
            Err(error) => {
                let interrupted = super::reset::pending(&coordinator.root).unwrap_or(true);
                if interrupted {
                    *phase = Phase::Failed;
                }
                Err(StartupError {
                    code: super::error::public_code(error),
                    restart_required: interrupted,
                })
            }
        }
    })
    .await
    .map_err(|_| StartupError {
        code: "secret.operation_failed".into(),
        restart_required: true,
    })?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartupError {
    code: String,
    restart_required: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
mod tests {
    use super::*;

    #[cfg_attr(test, test)]
    pub(super) fn upgrade_query_blocks_busy_runtime_and_restart_phases_without_reinspection() {
        let temporary = super::super::testing::tempdir().unwrap();
        let root = temporary.path().join("source");
        crate::config_file_io::ensure_private_directory(&root).unwrap();
        let device = crate::live::engine::DeviceStore::at(temporary.path().join("device"));
        let inspection = super::super::upgrade::inspect(&root, &device).unwrap();
        let coordinator = StartupCoordinator::new(root.clone(), inspection);
        std::fs::write(
            root.join(crate::config::DB_FILE_NAME),
            b"runtime-owned-source",
        )
        .unwrap();
        for (phase, expected) in [
            (Phase::Ready, "runtime_active"),
            (Phase::Initializing, "busy"),
            (Phase::Failed, "restart_required"),
            (Phase::Recovered, "restart_required"),
        ] {
            *coordinator.phase.lock().unwrap() = phase;
            let view = coordinator.upgrade_view().unwrap();
            assert_eq!(view.status, expected);
            assert!(view.source_versions.is_none());
            assert!(
                !view.can_authenticate && !view.can_check_and_backup && !view.can_start_upgrade
            );
            assert_eq!(
                std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap(),
                b"runtime-owned-source"
            );
        }
    }

    use std::cell::Cell;

    #[cfg_attr(test, test)]
    pub(super) fn recovery_query_does_not_authenticate_or_mutate_and_stale_action_is_refused() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("source");
        crate::config_file_io::ensure_private_directory(&root).unwrap();
        let marker = root.join(super::super::transition::INTENT);
        std::fs::write(&marker, b"unparsed generation fixture").unwrap();
        let device = crate::live::engine::DeviceStore::at(dir.path().join("device"));
        let inspected = super::super::upgrade::inspect(&root, &device).unwrap();
        let coordinator = StartupCoordinator::new(root, inspected);
        let before = std::fs::read(&marker).unwrap();
        let view = coordinator.recovery_view().unwrap();
        assert_eq!(view.status, "pending");
        assert!(view.can_recover);
        assert_eq!(std::fs::read(&marker).unwrap(), before);
        assert!(coordinator.ensure_no_pending_recovery().is_err());
        assert_eq!(
            coordinator
                .recover_operation(
                    "stale token",
                    "test recovery password",
                    &super::super::testing::MemoryKeyStore::default()
                )
                .unwrap_err(),
            "upgrade.source_changed"
        );
        assert_eq!(std::fs::read(&marker).unwrap(), before);
        std::fs::write(&marker, b"another generation").unwrap();
        let changed = coordinator.recovery_view().unwrap();
        assert_eq!(changed.status, "verification_required");
        assert!(!changed.can_recover);
        assert!(changed.restart_required);
        assert!(coordinator
            .recover_operation(
                &view.token,
                "test recovery password",
                &super::super::testing::MemoryKeyStore::default()
            )
            .is_err());
        assert_eq!(std::fs::read(&marker).unwrap(), b"another generation");
    }

    #[cfg_attr(test, test)]
    pub(super) fn failed_unlock_never_starts_runtime_and_success_is_published_once() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("source");
        let device = crate::live::engine::DeviceStore::at(dir.path().join("device"));
        let inspection = super::super::upgrade::inspect(&root, &device).unwrap();
        let coordinator = StartupCoordinator::new(root, inspection);
        let calls = Cell::new(0);
        assert!(coordinator
            .run_attempt(
                || Err("secret.password_rejected".into()),
                |_| {
                    calls.set(calls.get() + 1);
                    Ok(())
                }
            )
            .is_err());
        assert_eq!(calls.get(), 0);
        coordinator
            .run_attempt(
                || Ok(SecretSession::ephemeral().unwrap()),
                |_| {
                    calls.set(calls.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
        coordinator
            .run_attempt(
                || panic!("already unlocked"),
                |_| panic!("already published"),
            )
            .unwrap();
        assert_eq!(calls.get(), 1);
    }

    #[cfg_attr(test, test)]
    pub(super) fn failed_runtime_preparation_is_not_repeated_in_the_same_process() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("source");
        let device = crate::live::engine::DeviceStore::at(dir.path().join("device"));
        let inspection = super::super::upgrade::inspect(&root, &device).unwrap();
        let coordinator = StartupCoordinator::new(root, inspection);
        assert!(coordinator
            .run_attempt(
                || Ok(SecretSession::ephemeral().unwrap()),
                |_| Err("fixture failure".into())
            )
            .is_err());
        assert!(coordinator.restart_required());
        assert!(coordinator
            .run_attempt(|| panic!("requires restart"), |_| Ok(()))
            .is_err());
    }
}

#[cfg(any(test, feature = "test-hooks"))]
mod checkpoint_admission_tests {
    use super::*;
    #[cfg_attr(test, test)]
    pub(super) fn pending_upgrade_stops_unlock_before_any_runtime_callback() {
        let home = crate::secrets::testing::tempdir().unwrap();
        let root = home.path().join("data");
        let device = crate::live::engine::DeviceStore::at(home.path().join("device"));
        crate::config_file_io::ensure_private_directory(device.root()).unwrap();
        std::fs::write(
            device.root().join(super::super::upgrade::checkpoint::FILE),
            b"pending",
        )
        .unwrap();
        let inspected = super::super::upgrade::inspect(&root, &device).unwrap();
        let coordinator = StartupCoordinator::new(root, inspected);
        let unlocked = std::cell::Cell::new(false);
        let result = coordinator.run_attempt(
            || {
                unlocked.set(true);
                Err("unlock should not run".into())
            },
            |_| Ok(()),
        );
        assert_eq!(result, Err("upgrade.sync_paused".into()));
        assert!(!unlocked.get());
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_original_startup_admission() {
    tests::upgrade_query_blocks_busy_runtime_and_restart_phases_without_reinspection();
    tests::recovery_query_does_not_authenticate_or_mutate_and_stale_action_is_refused();
    tests::failed_unlock_never_starts_runtime_and_success_is_published_once();
    tests::failed_runtime_preparation_is_not_repeated_in_the_same_process();
    checkpoint_admission_tests::pending_upgrade_stops_unlock_before_any_runtime_callback();
}

#[cfg(test)]
mod upgrade_handoff_tests {
    use super::*;
    use crate::app_config::AppType;
    use crate::database::{self, Database};
    use crate::live::engine::DeviceStore;
    use crate::secrets::testing::{MemoryKeyStore, TestHome};
    use crate::secrets::upgrade::{checkpoint, review_tests::snapshot};
    use std::{cell::Cell, sync::Arc};

    struct Fixture {
        coordinator: StartupCoordinator,
        token: String,
        device: DeviceStore,
        native: PathBuf,
        home: TestHome,
        store: MemoryKeyStore,
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_password(None)
        }
        fn with_password(password: Option<&str>) -> Self {
            let home = TestHome::new().unwrap();
            crate::settings::reload_settings().unwrap();
            let root = crate::config::get_app_config_dir();
            let store = MemoryKeyStore::default();
            let session = SecretSession::open(&root, &store, password).unwrap();
            let conn = rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
            Database::create_tables_on_conn(&conn).unwrap();
            Database::apply_schema_migrations_on_conn(&conn).unwrap();
            database::loongport_schema::apply(&conn).unwrap();
            database::vault::stamp(&conn, &session.read().unwrap()).unwrap();
            session.complete_migration().unwrap();
            let db = Database::from_connection(conn, session.clone());
            let config = serde_json::json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-credential","ANTHROPIC_BASE_URL":"https://synthetic.example.invalid/v1","ANTHROPIC_MODEL":"synthetic-model"}});
            db.save_provider(
                "claude",
                &crate::provider::Provider::with_id(
                    "a".into(),
                    "Synthetic".into(),
                    config.clone(),
                    None,
                ),
            )
            .unwrap();
            db.set_current_provider("claude", "a").unwrap();
            drop(db);
            let settings = crate::settings::AppSettings {
                current_provider_claude: Some("a".into()),
                ..Default::default()
            };
            let bytes =
                crate::settings::encode_settings_with_vault(&settings, &session.read().unwrap())
                    .unwrap();
            crate::config_file_io::write_durable(&crate::settings::settings_path(), &bytes)
                .unwrap();
            let native = crate::config::get_claude_settings_path();
            crate::config_file_io::ensure_private_directory(native.parent().unwrap()).unwrap();
            let mut config = config;
            config["unowned"] = serde_json::json!({"keep":true});
            crate::config_file_io::write_durable(&native, &serde_json::to_vec(&config).unwrap())
                .unwrap();
            let device = DeviceStore::for_device();
            let file = crate::secrets::owned_file::DeviceFile::registered(
                crate::secrets::owned_file::DEVICE_STATE_FILE,
            )
            .unwrap();
            device.write_device(&session.read().unwrap(),&file,br#"{"version":1,"apps":{"claude":{"mode":"proxy","attached":false,"proxy_route":"a"},"codex":{"mode":"future-mode","opaque":900719925474099312345}}}"#).unwrap();
            let inspected = crate::secrets::upgrade::inspect(&root, &device).unwrap();
            let coordinator = StartupCoordinator::new(root, inspected);
            let token = coordinator
                .authenticate_upgrade(password, &store)
                .unwrap()
                .review_token
                .unwrap();
            let id = coordinator
                .prepare_upgrade_checkpoint(&token)
                .unwrap()
                .checkpoint_id
                .unwrap();
            let token = coordinator
                .publish_upgrade_checkpoint(&token, &id, &store)
                .unwrap()
                .review_token
                .unwrap();
            Self {
                coordinator,
                token,
                device,
                native,
                home,
                store,
            }
        }
        fn session(&self) -> Arc<SecretSession> {
            self.coordinator
                .upgrade_review
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .runtime_session(&self.coordinator.inspection.lock().unwrap(), &self.token)
                .unwrap()
        }
    }

    #[test]
    #[serial_test::serial]
    fn u03_restart_enters_controlled_runtime_after_original_authentication() {
        let f = Fixture::new();
        let root = f.coordinator.root.clone();
        let restarted = StartupCoordinator::new(
            root.clone(),
            crate::secrets::upgrade::inspect(&root, &f.device).unwrap(),
        );
        let vault_before = std::fs::read(root.join("vault.json")).unwrap();
        let checkpoint_before = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        let native_before = std::fs::read(&f.native).unwrap();
        let calls = Cell::new(0);
        let result = restarted.initialize_with(None, &f.store, |session| {
            calls.set(calls.get() + 1);
            assert!(matches!(
                *restarted.phase.lock().unwrap(),
                Phase::Initializing
            ));
            let legacy = session.migration_pending().map_err(|e| e.to_string())?;
            crate::secrets::migration::prepare_files(&session, legacy)
                .map_err(|e| e.to_string())?;
            crate::settings::unlock_settings(session.clone()).map_err(|e| e.to_string())?;
            assert!(!session.legacy_json_pending().unwrap());
            let db =
                Arc::new(Database::init_with_secrets(session.clone()).map_err(|e| e.to_string())?);
            let copilot = Arc::new(tokio::sync::RwLock::new(
                crate::proxy::providers::copilot_auth::CopilotAuthManager::new(session.clone())
                    .map_err(|e| e.to_string())?,
            ));
            let xai = Arc::new(tokio::sync::RwLock::new(
                crate::proxy::providers::xai_oauth_auth::XaiOAuthManager::new(session.clone())
                    .map_err(|e| e.to_string())?,
            ));
            let state = crate::store::AppState::new(db).map_err(|e| e.to_string())?;
            state
                .proxy_service
                .set_managed_auth(copilot, xai)
                .map_err(|e| e.to_string())?;
            session.complete_migration().map_err(|e| e.to_string())?;
            assert!(checkpoint::ensure_no_pending_checkpoint(&f.device).is_err());
            Ok(())
        });
        assert_eq!(
            result,
            Ok(()),
            "verified restart should use the original controlled handoff"
        );
        assert_eq!(calls.get(), 1);
        assert!(matches!(*restarted.phase.lock().unwrap(), Phase::Ready));
        restarted
            .initialize_with(None, &f.store, |_| panic!("already prepared"))
            .unwrap();
        let view = restarted.upgrade_view().unwrap();
        assert_eq!(view.status, "database_verified");
        let token = view.review_token.unwrap();
        assert!(
            restarted
                .review_upgrade_app(&token, &AppType::Claude)
                .unwrap()
                .can_complete_app
        );
        assert!(
            !restarted
                .review_upgrade_app(&token, &AppType::Codex)
                .unwrap()
                .can_complete_app
        );
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
        assert!(std::fs::read(root.join("vault.json")).unwrap() == vault_before);
        assert!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap() == checkpoint_before
        );
        assert!(std::fs::read(&f.native).unwrap() == native_before);
    }

    #[test]
    #[serial_test::serial]
    fn u03_restart_preserves_disabled_automatic_unlock_password_path() {
        let password = "synthetic-restart-password";
        let f = Fixture::with_password(Some(password));
        let root = f.coordinator.root.clone();
        let restarted = StartupCoordinator::new(
            root.clone(),
            crate::secrets::upgrade::inspect(&root, &f.device).unwrap(),
        );
        let before = snapshot(f.home.path());
        assert_eq!(
            restarted.initialize_with(None, &f.store, |_| panic!("password required")),
            Err("secret.locked".into())
        );
        assert!(restarted
            .initialize_with(Some("synthetic-wrong"), &f.store, |_| panic!(
                "wrong password"
            ))
            .is_err());
        assert!(matches!(*restarted.phase.lock().unwrap(), Phase::Locked));
        assert_eq!(snapshot(f.home.path()), before);
        let calls = Cell::new(0);
        restarted
            .initialize_with(Some(password), &f.store, |session| {
                calls.set(calls.get() + 1);
                assert!(!session.migration_pending().unwrap());
                assert!(checkpoint::verified_database_id(
                    &root,
                    &f.device,
                    &session.read().unwrap()
                )
                .unwrap()
                .is_some());
                Ok(())
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(snapshot(f.home.path()), before);
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn u03_restart_refuses_unverified_checkpoint_database_and_generation() {
        for case in [
            "checkpoint",
            "unpublished",
            "generation",
            "identity",
            "schema",
        ] {
            let f = Fixture::new();
            let root = f.coordinator.root.clone();
            match case {
                "checkpoint" => std::fs::write(
                    f.device.root().join(checkpoint::FILE),
                    b"synthetic-corrupt-proof",
                )
                .unwrap(),
                "unpublished" => {
                    let file = crate::secrets::owned_file::DeviceFile::registered(checkpoint::FILE)
                        .unwrap();
                    let session = f.session();
                    let vault = session.read().unwrap();
                    let bytes = f.device.read_device(&vault, &file).unwrap().unwrap();
                    let mut proof: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    proof.as_object_mut().unwrap().remove("published_database");
                    f.device
                        .write_device(&vault, &file, &serde_json::to_vec(&proof).unwrap())
                        .unwrap();
                }
                "generation" => std::fs::write(
                    root.join(crate::secrets::transition::INTENT),
                    b"synthetic-pending",
                )
                .unwrap(),
                "identity" => {
                    let conn =
                        rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
                    database::vault::stamp(
                        &conn,
                        &crate::secrets::VaultContext::generate().unwrap(),
                    )
                    .unwrap();
                }
                "schema" => {
                    let conn =
                        rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
                    conn.pragma_update(
                        None,
                        "user_version",
                        database::UPSTREAM4_SCHEMA_VERSION + 1,
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
            let before = snapshot(f.home.path());
            let inspected = match crate::secrets::upgrade::inspect(&root, &f.device) {
                Ok(inspected) => inspected,
                Err(error) => {
                    assert_eq!(case, "identity");
                    assert_eq!(
                        crate::secrets::error::public_code(error),
                        "secret.identity_mismatch"
                    );
                    assert_eq!(snapshot(f.home.path()), before, "{case}");
                    continue;
                }
            };
            let restarted = StartupCoordinator::new(root, inspected);
            assert!(
                restarted
                    .initialize_with(None, &f.store, |_| panic!(
                        "unverified runtime must not start"
                    ))
                    .is_err(),
                "{case}"
            );
            assert_eq!(snapshot(f.home.path()), before, "{case}");
            assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
        }
    }

    #[test]
    #[serial_test::serial]
    fn u03_restart_keeps_busy_failed_and_changed_handoffs_controlled() {
        let f = Fixture::new();
        let root = f.coordinator.root.clone();
        let restarted = StartupCoordinator::new(
            root.clone(),
            crate::secrets::upgrade::inspect(&root, &f.device).unwrap(),
        );
        let before = snapshot(f.home.path());
        *restarted.phase.lock().unwrap() = Phase::Initializing;
        assert_eq!(
            restarted.initialize_with(None, &f.store, |_| panic!("busy")),
            Err("secret.initializing".into())
        );
        *restarted.phase.lock().unwrap() = Phase::Locked;
        assert_eq!(
            restarted.initialize_with(None, &f.store, |_| Err("synthetic-runtime-failure".into())),
            Err("synthetic-runtime-failure".into())
        );
        assert_eq!(
            restarted.initialize_with(None, &f.store, |_| panic!("failed runtime")),
            Err("secret.restart_required".into())
        );
        assert_eq!(snapshot(f.home.path()), before);
        let restarted = StartupCoordinator::new(
            root.clone(),
            crate::secrets::upgrade::inspect(&root, &f.device).unwrap(),
        );
        restarted
            .initialize_with(None, &f.store, |_| Ok(()))
            .unwrap();
        std::fs::write(
            f.device.root().join(checkpoint::FILE),
            b"synthetic-checkpoint-drift",
        )
        .unwrap();
        let changed = snapshot(f.home.path());
        assert!(restarted
            .initialize_with(None, &f.store, |_| panic!("changed checkpoint"))
            .is_err());
        assert_eq!(snapshot(f.home.path()), changed);
    }

    #[test]
    #[serial_test::serial]
    fn u03_verified_handoff_isolates_conflicting_peer_provider_settings() {
        let f = Fixture::new();
        let path = crate::settings::settings_path();
        let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let peer = r#"{"future":"pointer", "opaque":900719925474099312345}"#;
        fields.insert(
            "currentProviderCodex".into(),
            serde_json::value::RawValue::from_string(peer.into()).unwrap(),
        );
        std::fs::write(&path, serde_json::to_vec(&fields).unwrap()).unwrap();
        let native_before = std::fs::read(&f.native).unwrap();
        let settings_before = std::fs::read(&path).unwrap();
        f.coordinator
            .run_upgrade_attempt(&f.token, |session| {
                let legacy_allowed = session.migration_pending().map_err(|e| e.to_string())?;
                crate::secrets::migration::prepare_files(&session, legacy_allowed)
                    .map_err(|e| e.to_string())?;
                assert!(std::fs::read(&path).unwrap() == settings_before);
                crate::settings::unlock_settings(session.clone()).map_err(|e| e.to_string())?;
                Database::init_with_secrets(session).map_err(|e| e.to_string())?;
                Ok(())
            })
            .unwrap();
        assert!(
            f.coordinator
                .review_upgrade_app(&f.token, &AppType::Claude)
                .unwrap()
                .can_complete_app
        );
        assert!(f
            .coordinator
            .review_upgrade_app(&f.token, &AppType::Codex)
            .is_err());
        crate::settings::reload_settings().unwrap();
        assert_eq!(
            crate::settings::get_current_provider_ready(&AppType::Claude)
                .unwrap()
                .as_deref(),
            Some("a")
        );
        assert!(crate::settings::get_current_provider_ready(&AppType::Codex).is_err());
        let session = f.session();
        let db = Database::init_with_secrets(session.clone()).unwrap();
        assert!(crate::settings::get_effective_current_provider(&db, &AppType::Codex).is_err());
        assert!(
            crate::settings::get_effective_current_provider_readonly(&db, &AppType::Codex).is_err()
        );
        let before = snapshot(f.home.path());
        assert!(crate::settings::set_current_provider(&AppType::Codex, None).is_err());
        let mut dto = crate::settings::get_settings();
        dto.current_provider_codex = Some("synthetic-new-selection".into());
        assert!(crate::settings::update_settings(dto).is_err());
        {
            let vault = session.read().unwrap();
            assert!(crate::settings::set_current_provider_with_vault(
                &AppType::Codex,
                Some("synthetic-new-selection"),
                &session,
                &vault
            )
            .is_err());
        }
        assert_eq!(snapshot(f.home.path()), before);
        crate::settings::update_settings(crate::settings::get_settings()).unwrap();
        {
            let vault = session.read().unwrap();
            crate::settings::set_current_provider_with_vault(
                &AppType::Claude,
                Some("a"),
                &session,
                &vault,
            )
            .unwrap();
        }
        crate::settings::mutate_settings(|settings| settings.language = Some("zh".into())).unwrap();
        let after: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after["currentProviderCodex"].get(), peer);
        assert_eq!(std::fs::read(&f.native).unwrap(), native_before);
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn u03_tray_section_provider_conflicts_keep_reliable_peer_and_shared_refusal() {
        let f = Fixture::new();
        let session = f.session();
        let path = crate::settings::settings_path();
        let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        fields.insert(
            "currentProviderCodex".into(),
            serde_json::value::RawValue::from_string(r#"{"future":true}"#.into()).unwrap(),
        );
        std::fs::write(&path, serde_json::to_vec(&fields).unwrap()).unwrap();
        crate::settings::unlock_settings(session.clone()).unwrap();
        let db = Database::init_with_secrets(session.clone()).unwrap();
        let before = snapshot(f.home.path());
        assert_eq!(
            crate::tray::tray_provider_selection(&db, &AppType::Codex).unwrap(),
            crate::tray::TrayProviderSelection::RequiresReview
        );
        assert_eq!(
            crate::tray::tray_provider_selection(&db, &AppType::Claude).unwrap(),
            crate::tray::TrayProviderSelection::Ready(Some("a".into()))
        );
        assert_eq!(
            crate::tray::tray_provider_selection(&db, &AppType::Gemini).unwrap(),
            crate::tray::TrayProviderSelection::Ready(None)
        );
        session.set_blocked(true);
        assert!(crate::tray::tray_provider_selection(&db, &AppType::Claude).is_err());
        session.set_blocked(false);
        assert_eq!(
            crate::tray::tray_provider_selection(&db, &AppType::Claude).unwrap(),
            crate::tray::TrayProviderSelection::Ready(Some("a".into()))
        );
        assert!(snapshot(f.home.path()) == before);
    }

    #[test]
    #[serial_test::serial]
    fn u03_pending_review_effective_query_preserves_conflicting_provider_choice() {
        let f = Fixture::new();
        let session = f.session();
        let path = crate::settings::settings_path();
        let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        fields.insert(
            "currentProviderClaude".into(),
            serde_json::value::RawValue::from_string(r#""synthetic-unresolved-provider""#.into())
                .unwrap(),
        );
        std::fs::write(&path, serde_json::to_vec(&fields).unwrap()).unwrap();
        crate::settings::unlock_settings(session.clone()).unwrap();
        let db = Database::init_with_secrets(session).unwrap();
        let before = snapshot(f.home.path());
        let result = crate::settings::get_effective_current_provider(&db, &AppType::Claude);
        assert!(
            snapshot(f.home.path()) == before,
            "effective query must not clear an unresolved review choice"
        );
        assert!(result.is_err());
        assert_eq!(
            crate::settings::get_current_provider_ready(&AppType::Claude)
                .unwrap()
                .as_deref(),
            Some("synthetic-unresolved-provider")
        );
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn u03_settings_conflict_projection_requires_original_published_checkpoint() {
        for case in ["absent", "damaged", "protected"] {
            let f = Fixture::new();
            let session = f.session();
            let path = crate::settings::settings_path();
            let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            fields.insert(
                "currentProviderCodex".into(),
                serde_json::value::RawValue::from_string(r#"{"future":true}"#.into()).unwrap(),
            );
            match case {
                "absent" => std::fs::remove_file(f.device.root().join(checkpoint::FILE)).unwrap(),
                "damaged" => std::fs::write(
                    f.device.root().join(checkpoint::FILE),
                    b"synthetic-invalid-checkpoint",
                )
                .unwrap(),
                "protected" => {
                    fields.insert(
                        "webdavSync".into(),
                        serde_json::value::RawValue::from_string(
                            r#"{"password":"synthetic-plaintext"}"#.into(),
                        )
                        .unwrap(),
                    );
                }
                _ => unreachable!(),
            }
            let bytes = serde_json::to_vec(&fields).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            let before = snapshot(f.home.path());
            assert!(
                crate::settings::decode_settings_with_vault(&bytes, &session.read().unwrap())
                    .is_err(),
                "{case}"
            );
            assert!(
                crate::secrets::migration::prepare_files(&session, false).is_err(),
                "{case}"
            );
            assert!(crate::settings::unlock_settings(session).is_err(), "{case}");
            assert_eq!(snapshot(f.home.path()), before, "{case}");
        }
    }

    #[test]
    #[serial_test::serial]
    fn u03_settings_projection_retains_each_conflicting_app_pointer() {
        for (field, app) in [
            ("currentProviderClaude", AppType::Claude),
            ("currentProviderCodex", AppType::Codex),
            ("currentProviderGemini", AppType::Gemini),
            ("currentProviderGrokbuild", AppType::GrokBuild),
        ] {
            let f = Fixture::new();
            let path = crate::settings::settings_path();
            let mut fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let peer = r#"{"future":"pointer", "opaque":900719925474099312345}"#;
            fields.insert(
                field.into(),
                serde_json::value::RawValue::from_string(peer.into()).unwrap(),
            );
            std::fs::write(&path, serde_json::to_vec(&fields).unwrap()).unwrap();
            let session = f.session();
            crate::secrets::migration::prepare_files(&session, false).unwrap();
            crate::settings::unlock_settings(session).unwrap();
            crate::settings::reload_settings().unwrap();
            assert!(
                crate::settings::get_current_provider_ready(&app).is_err(),
                "{field}"
            );
            crate::settings::mutate_settings(|settings| settings.language = Some("zh".into()))
                .unwrap();
            let after: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(after[field].get(), peer, "{field}");
        }
    }

    #[test]
    #[serial_test::serial]
    fn u03_preflight_preserves_original_legacy_settings_migration() {
        let _home = TestHome::new().unwrap();
        let root = crate::config::get_app_config_dir();
        let store = MemoryKeyStore::default();
        let session = SecretSession::open(&root, &store, None).unwrap();
        let settings = crate::settings::AppSettings {
            webdav_sync: Some(crate::settings::WebDavSyncSettings {
                base_url: "https://synthetic.example.invalid".into(),
                password: "synthetic-legacy-credential".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let path = crate::settings::settings_path();
        let plaintext = serde_json::to_vec(&settings).unwrap();
        std::fs::write(&path, &plaintext).unwrap();
        assert!(
            crate::settings::decode_settings_with_vault(&plaintext, &session.read().unwrap())
                .is_err()
        );
        assert!(crate::secrets::migration::prepare_files(&session, false).is_err());
        assert!(std::fs::read(&path).unwrap() == plaintext);
        assert!(session.migration_pending().unwrap());
        crate::secrets::migration::prepare_files(&session, true).unwrap();
        let encrypted = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&encrypted).contains("synthetic-legacy-credential"));
        let restored =
            crate::settings::decode_settings_with_vault(&encrypted, &session.read().unwrap())
                .unwrap();
        assert!(restored.webdav_sync.unwrap().password == "synthetic-legacy-credential");
        crate::secrets::migration::prepare_files(&session, true).unwrap();
        assert!(std::fs::read(&path).unwrap() == encrypted);
    }

    #[test]
    #[serial_test::serial]
    fn u03_runtime_migration_completion_preserves_verified_checkpoint() {
        let f = Fixture::new();
        let vault_path = f.coordinator.root.join("vault.json");
        let vault_before = std::fs::read(&vault_path).unwrap();
        let checkpoint_before = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        f.coordinator
            .run_upgrade_attempt(&f.token, |session| {
                assert!(!session.migration_pending().unwrap());
                crate::secrets::migration::prepare_files(&session, false).unwrap();
                crate::settings::unlock_settings(session.clone()).unwrap();
                let _db = Database::init_with_secrets(session.clone()).unwrap();
                // The real initialize_runtime calls this after opening its owners,
                // including when the original migration was already complete.
                session.complete_migration().unwrap();
                Ok(())
            })
            .unwrap();
        let vault_unchanged = std::fs::read(&vault_path).unwrap() == vault_before;
        let checkpoint_verified = f.coordinator.upgrade_view().is_ok();
        assert!(
            vault_unchanged && checkpoint_verified,
            "already-complete runtime migration must preserve vault and checkpoint: vault_unchanged={vault_unchanged}, checkpoint_verified={checkpoint_verified}"
        );
        assert!(
            f.coordinator
                .review_upgrade_app(&f.token, &AppType::Claude)
                .unwrap()
                .can_complete_app
        );
        assert!(
            !f.coordinator
                .review_upgrade_app(&f.token, &AppType::Codex)
                .unwrap()
                .can_complete_app
        );
        let session = f.session();
        let checkpoint_id = checkpoint::verified_database_id(
            &f.coordinator.root,
            &f.device,
            &session.read().unwrap(),
        )
        .unwrap()
        .unwrap();
        let restarted = StartupCoordinator::new(
            f.coordinator.root.clone(),
            crate::secrets::upgrade::inspect(&f.coordinator.root, &f.device).unwrap(),
        );
        let authenticated = restarted.authenticate_upgrade(None, &f.store).unwrap();
        assert_eq!(authenticated.status, "database_verified");
        assert_eq!(
            authenticated.checkpoint_id.as_deref(),
            Some(checkpoint_id.as_str())
        );
        assert!(restarted.verify_runtime_admission_blocked());
        assert!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap() == checkpoint_before
        );
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn u03_explicit_handoff_reuses_session_once_and_keeps_original_review() {
        let f = Fixture::new();
        let expected = f.session();
        let calls = Cell::new(0);
        f.coordinator
            .run_upgrade_attempt(&f.token, |session| {
                calls.set(calls.get() + 1);
                assert!(Arc::ptr_eq(&expected, &session));
                crate::settings::unlock_settings(session.clone()).unwrap();
                let db = Database::init_with_secrets(session).unwrap();
                assert!(Arc::ptr_eq(&db.secrets, &expected));
                Ok(())
            })
            .unwrap();
        f.coordinator
            .run_upgrade_attempt(&f.token, |_| panic!("handoff must not prepare twice"))
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(
            f.coordinator.upgrade_view().unwrap().status,
            "database_verified"
        );
        assert!(
            f.coordinator
                .review_upgrade_app(&f.token, &AppType::Claude)
                .unwrap()
                .can_complete_app
        );
        assert!(
            !f.coordinator
                .review_upgrade_app(&f.token, &AppType::Codex)
                .unwrap()
                .can_complete_app
        );
        assert!(checkpoint::ensure_sync_admitted(&f.device).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn u03_explicit_handoff_refuses_stale_or_unverified_facts_before_callback() {
        for case in [
            "stale_token",
            "checkpoint_replaced",
            "generation_pending",
            "busy",
        ] {
            let f = Fixture::new();
            let mut token = f.token.clone();
            match case {
                "stale_token" => token = "synthetic-stale-token".into(),
                "checkpoint_replaced" => std::fs::write(
                    f.device.root().join(checkpoint::FILE),
                    b"synthetic-replaced-checkpoint",
                )
                .unwrap(),
                "generation_pending" => std::fs::write(
                    f.coordinator.root.join(crate::secrets::transition::INTENT),
                    b"synthetic-generation-intent",
                )
                .unwrap(),
                "busy" => *f.coordinator.phase.lock().unwrap() = Phase::Initializing,
                _ => unreachable!(),
            }
            let before = snapshot(f.home.path());
            let called = Cell::new(false);
            assert!(
                f.coordinator
                    .run_upgrade_attempt(&token, |_| {
                        called.set(true);
                        Ok(())
                    })
                    .is_err(),
                "{case}"
            );
            assert!(!called.get(), "{case}");
            assert_eq!(snapshot(f.home.path()), before, "{case}");
        }
    }

    #[test]
    #[serial_test::serial]
    fn u03_explicit_handoff_failure_preserves_checkpoint_and_requires_restart() {
        let f = Fixture::new();
        let before = snapshot(f.home.path());
        let result = f
            .coordinator
            .run_upgrade_attempt(&f.token, |_| Err("synthetic-runtime-failure".into()));
        assert_eq!(result, Err("synthetic-runtime-failure".into()));
        assert!(f.coordinator.restart_required());
        assert!(f
            .coordinator
            .run_upgrade_attempt(&f.token, |_| panic!("failed runtime cannot prepare again"))
            .is_err());
        assert_eq!(snapshot(f.home.path()), before);
        assert_eq!(
            f.coordinator.upgrade_view().unwrap().status,
            "restart_required"
        );
    }

    #[test]
    #[serial_test::serial]
    fn u03_default_handoff_never_automatically_reattaches_saved_proxy() {
        let f = Fixture::new();
        let session = f.session();
        crate::settings::unlock_settings(session.clone()).unwrap();
        let db = Arc::new(Database::init_with_secrets(session).unwrap());
        let state = crate::store::AppState::new(db).unwrap();
        let native = std::fs::read(&f.native).unwrap();
        let checkpoint_bytes = std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap();
        let peer = std::fs::read(f.device.state_path()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(runtime
            .block_on(state.proxy_service.recover_from_crash())
            .is_err());
        assert_eq!(
            std::fs::read(&f.native).unwrap(),
            native,
            "default handoff keeps original native files"
        );
        let vault = state.db.secret_session().read().unwrap();
        let mode = crate::mode::state::mode_state(&f.device, &vault, "claude").unwrap();
        assert_eq!(mode.mode, Some(crate::mode::state::Mode::Proxy));
        assert_eq!(mode.proxy_route.as_deref(), Some("a"));
        assert!(!mode.attached);
        assert_eq!(std::fs::read(f.device.state_path()).unwrap(), peer);
        assert_eq!(
            std::fs::read(f.device.root().join(checkpoint::FILE)).unwrap(),
            checkpoint_bytes
        );
        assert!(!runtime.block_on(state.proxy_service.is_running()));
    }
    #[test]
    #[serial_test::serial]
    fn u03_retained_checkpoint_pauses_automatic_history_mcp_and_catalog_owners() {
        for case in [
            "history",
            "templates",
            "official_history",
            "imagegen_mcp",
            "catalog",
        ] {
            let f = Fixture::new();
            let session = f.session();
            crate::settings::unlock_settings(session.clone()).unwrap();
            let db = Arc::new(Database::init_with_secrets(session).unwrap());
            db.save_mcp_server(&crate::app_config::McpServer {
                id: crate::relay::imagegen_mcp::IMAGEGEN_MCP_ID.into(),
                name: "Synthetic retained MCP".into(),
                server: serde_json::json!({"command":"synthetic-command"}),
                apps: crate::app_config::McpApps {
                    claude: true,
                    ..Default::default()
                },
                description: None,
                homepage: None,
                docs: None,
                tags: Vec::new(),
            })
            .unwrap();
            let state = crate::store::AppState::new(db).unwrap();
            let before = snapshot(f.home.path());
            let result = match case {
                "history" => crate::codex_history_migration::maybe_migrate_codex_third_party_history_provider_bucket(&state.db).map(|_| ()),
                "templates" => crate::codex_history_migration::maybe_migrate_codex_provider_template_bucket(&state.db).map(|_| ()),
                "official_history" => crate::codex_history_migration::maybe_migrate_codex_official_history_to_unified_bucket().map(|_| ()),
                "imagegen_mcp" => crate::relay::imagegen_mcp::sync_registration(&state),
                "catalog" => crate::services::provider::refresh_current_codex_catalog_projection(&state).map(|_| ()),
                _ => unreachable!(),
            }.map_err(super::super::error::public_code);
            assert_eq!(result, Err("upgrade.checkpoint_pending".into()), "{case}");
            assert_eq!(snapshot(f.home.path()), before, "{case}");
            assert!(
                state
                    .db
                    .get_all_mcp_servers()
                    .unwrap()
                    .contains_key(crate::relay::imagegen_mcp::IMAGEGEN_MCP_ID),
                "{case}"
            );
        }
    }
}
