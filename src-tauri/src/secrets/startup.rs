//! Unlock owns runtime publication; credential consumers never start the lifecycle.

use super::{key_store::SystemKeyStore, session::SecretSession};
use std::{path::PathBuf, sync::Mutex};
use tauri::{Emitter, Manager};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Locked,
    Initializing,
    Ready,
    Failed,
}

pub(crate) struct StartupCoordinator {
    root: PathBuf,
    phase: Mutex<Phase>,
}

impl StartupCoordinator {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            phase: Mutex::new(Phase::Locked),
        }
    }

    fn initialize(&self, app: &tauri::AppHandle, password: Option<&str>) -> Result<(), String> {
        self.run_attempt(
            || {
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
            Phase::Initializing => return Err("secret.initializing".into()),
            Phase::Failed => return Err("secret.restart_required".into()),
        }
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

    fn restart_required(&self) -> bool {
        self.phase
            .lock()
            .map(|phase| *phase == Phase::Failed)
            .unwrap_or(true)
    }
}

fn prepare_runtime(
    app: &tauri::AppHandle,
    session: std::sync::Arc<SecretSession>,
) -> Result<(), String> {
    crate::initialize_runtime(app, session).map_err(|e| e.to_string())?;
    crate::init_status::clear_init_error();
    let _ = app.emit("runtime-ready", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn preview_startup_restore(
    app: tauri::AppHandle,
    source: super::bootstrap_restore::RestoreSource,
) -> Result<super::bootstrap_restore::RestorePreview, String> {
    {
        let coordinator = app.state::<StartupCoordinator>();
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

pub(crate) fn try_automatic_unlock(app: &tauri::AppHandle) {
    let coordinator = app.state::<StartupCoordinator>();
    if let Err(error) = coordinator.initialize(app, None) {
        present_error(app, &error);
    }
}

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

#[tauri::command]
pub(crate) async fn preview_secret_reset(
    app: tauri::AppHandle,
) -> Result<super::reset::ResetPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app.state::<StartupCoordinator>();
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

#[tauri::command]
pub(crate) async fn reset_secret_vault(
    app: tauri::AppHandle,
    fingerprint: String,
    password: String,
) -> Result<String, StartupError> {
    let password = zeroize::Zeroizing::new(password);
    tauri::async_runtime::spawn_blocking(move || {
        let coordinator = app.state::<StartupCoordinator>();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn failed_unlock_never_starts_runtime_and_success_is_published_once() {
        let coordinator = StartupCoordinator::new(PathBuf::new());
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

    #[test]
    fn failed_runtime_preparation_is_not_repeated_in_the_same_process() {
        let coordinator = StartupCoordinator::new(PathBuf::new());
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
