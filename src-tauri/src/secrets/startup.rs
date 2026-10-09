//! Unlock owns runtime publication; credential consumers never start the lifecycle.

#[cfg(feature = "gui")]
use super::key_store::SystemKeyStore;
use super::session::SecretSession;
use std::{path::PathBuf, sync::Mutex};
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
    recovery_token: Option<String>,
    phase: Mutex<Phase>,
    upgrade_review: Mutex<Option<super::upgrade::AuthenticatedUpgrade>>,
}

impl StartupCoordinator {
    pub(crate) fn new(root: PathBuf, inspection: super::upgrade::UpgradeInspection) -> Self {
        Self {
            root,
            recovery_token: match &inspection {
                super::upgrade::UpgradeInspection::RecoveryRequired(evidence) => {
                    Some(evidence.token())
                }
                _ => None,
            },
            inspection: Mutex::new(inspection),
            phase: Mutex::new(Phase::Locked),
            upgrade_review: Mutex::new(None),
        }
    }

    #[cfg(feature = "gui")]
    fn initialize(&self, app: &tauri::AppHandle, password: Option<&str>) -> Result<(), String> {
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
                    let vault = super::session::authenticate_existing(
                        &self.root,
                        &SystemKeyStore,
                        password,
                    )
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
                super::bootstrap_restore::recover(&self.root, &SystemKeyStore, password)
                    .map_err(super::error::public_code)?;
                crate::settings::reload_settings().map_err(super::error::public_code)?;
                crate::settings::bootstrap_settings().map_err(super::error::public_code)?;
                crate::database::vault::preflight(&self.root.join(crate::config::DB_FILE_NAME))
                    .map_err(super::error::public_code)?;
                SecretSession::open(&self.root, &SystemKeyStore, password)
                    .map_err(super::error::public_code)
            },
            |session| prepare_runtime(app, session),
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

    pub(super) fn upgrade_view(&self) -> Result<super::upgrade::StartupUpgradeView, String> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        let blocked = match *phase {
            Phase::Ready => Some("runtime_active"),
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
        if *phase == Phase::UpgradeReview {
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

    pub(super) fn review_upgrade_app(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
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
            .review_app(&inspection, token, app)
            .map_err(super::error::public_code)
    }

    pub(super) fn recover_upgrade_app(
        &self,
        token: &str,
        app: &crate::app_config::AppType,
        revision: &str,
    ) -> Result<super::upgrade::UpgradeAppReview, String> {
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
            .recover_app(&inspection, token, app, revision)
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

    fn recovery_view(&self) -> Result<StartupRecoveryView, String> {
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
            .clone()
            .ok_or("secret.no_pending_operation")?;
        let (status, can_recover, restart_required) = match (&*inspection, *phase) {
            (super::upgrade::UpgradeInspection::RecoveryRequired(evidence), Phase::Locked)
                if evidence.verify_unchanged(&self.root, &token).is_ok() =>
            {
                ("pending", true, false)
            }
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

    fn recover_operation(
        &self,
        token: &str,
        password: &str,
        store: &dyn super::key_store::KeyStore,
    ) -> Result<StartupRecoveryView, String> {
        let mut phase = self
            .phase
            .lock()
            .map_err(|_| "secret.startup_unavailable")?;
        if *phase != Phase::Locked {
            return Err("secret.restart_required".into());
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
            .review_upgrade_app(&expected_review_token, &app_type)
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
        app.try_state::<StartupCoordinator>()
            .ok_or("secret.startup_unavailable")?
            .recover_upgrade_app(&expected_review_token, &app_type, &expected_app_revision)
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
