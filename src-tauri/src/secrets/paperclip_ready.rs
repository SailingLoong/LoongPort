//! Private same-crate hook; StartupCoordinator remains the only Ready authority.
use super::{Phase, StartupCoordinator};
use crate::{
    app_config::AppType,
    error::AppError,
    paperclip::{Bridge, Owner},
    store::AppState,
};
use std::path::Path;
#[cfg(test)]
mod launch_tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn paperclip_native_launch_is_explicit_and_rejects_partial_or_invalid_configuration() {
        assert!(NativeLaunch::from_parts(None, None).unwrap().is_none());
        for (root, owner) in [
            (Some(OsString::from("/tmp")), None),
            (None, Some(OsString::from("owner-a"))),
            (
                Some(OsString::from("relative")),
                Some(OsString::from("owner-a")),
            ),
            (Some(OsString::from("/tmp")), Some(OsString::from(""))),
            (
                Some(OsString::from("/tmp")),
                Some(OsString::from_vec(vec![0xff])),
            ),
        ] {
            assert!(NativeLaunch::from_parts(root, owner).is_err());
        }
    }

    #[test]
    #[serial_test::serial]
    fn paperclip_native_launch_shares_ready_and_terminal_exit_authority() {
        use crate::{database::Database, live::engine::DeviceStore, secrets::upgrade};
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _entered = rt.enter();
        let root = crate::paperclip::tests::private_root();
        let path = std::fs::canonicalize(root.path()).unwrap();
        let c = StartupCoordinator::new(
            path.clone(),
            upgrade::inspect(&path, &DeviceStore::at(path.join("device"))).unwrap(),
        );
        let db = std::sync::Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone()).unwrap();
        let launch = || {
            NativeLaunch::from_parts(Some(path.clone().into_os_string()), Some("owner-a".into()))
                .unwrap()
        };
        c.paperclip_launch(&state, None).unwrap();
        assert!(!path.join("owner.sock").exists());
        assert!(c.paperclip_launch(&state, launch()).is_err());
        c.run_attempt(|| Ok(db.secrets.clone()), |_| Ok(()))
            .unwrap();
        c.paperclip_launch(&state, launch()).unwrap();
        c.paperclip_launch(&state, launch()).unwrap();
        assert!(path.join("owner.sock").exists());
        c.paperclip_stop_bridge().unwrap();
        assert!(!path.join("owner.sock").exists());
        assert!(c.paperclip_launch(&state, launch()).is_err());
    }
}

/// Explicit launch configuration only; absence keeps the native bridge disabled.
struct NativeLaunch {
    root: std::path::PathBuf,
    owner: String,
}
impl NativeLaunch {
    fn from_environment() -> Result<Option<Self>, AppError> {
        Self::from_parts(
            std::env::var_os("PAPERCLIP_NATIVE_SOCKET_ROOT"),
            std::env::var_os("PAPERCLIP_NATIVE_OWNER_ID"),
        )
    }
    fn from_parts(
        root: Option<std::ffi::OsString>,
        owner: Option<std::ffi::OsString>,
    ) -> Result<Option<Self>, AppError> {
        let unavailable = || AppError::Config("paperclip.owner_unavailable".into());
        match (root, owner) {
            (None, None) => Ok(None),
            (Some(root), Some(owner)) => {
                let root = std::path::PathBuf::from(root);
                let owner = owner.into_string().map_err(|_| unavailable())?;
                if !root.is_absolute() || !crate::paperclip::bindings::id(&owner) {
                    return Err(unavailable());
                }
                Ok(Some(Self { root, owner }))
            }
            _ => Err(unavailable()),
        }
    }
}
impl StartupCoordinator {
    fn paperclip_launch(
        &self,
        state: &AppState,
        launch: Option<NativeLaunch>,
    ) -> Result<(), AppError> {
        if let Some(launch) = launch {
            self.register_paperclip_bridge(state, AppType::Codex, &launch.owner, &launch.root)?;
        }
        Ok(())
    }
    pub(crate) fn start_paperclip_bridge(
        &self,
        state: &AppState,
        app: AppType,
        owner: &str,
        root: &Path,
    ) -> Result<Bridge, AppError> {
        let phase = self
            .phase
            .lock()
            .map_err(|_| AppError::Config("paperclip.owner_unavailable".into()))?;
        if *phase != Phase::Ready {
            return Err(AppError::Config("paperclip.owner_unavailable".into()));
        }
        // A retained checkpoint can publish the original runtime while its
        // historical schema is still active. Only its owner may release it.
        super::super::upgrade::checkpoint::ensure_no_pending_checkpoint(
            &crate::live::engine::DeviceStore::for_device(),
        )?;
        let bridge_owner = Owner::new(state.clone(), app, owner.into())?;
        #[cfg(test)]
        let bridge_owner = if std::env::var("PC_NATIVE_FIXTURE_ENABLE").as_deref() == Ok("1")
            && std::env::var_os("PC_NATIVE_ROOT").is_some_and(|p| std::path::Path::new(&p) == root)
            && matches!(
                std::env::var("PC_NATIVE_FAULT").as_deref(),
                Ok("synthetic-responses"
                    | "synthetic-responses-pending"
                    | "synthetic-responses-crash")
            ) {
            bridge_owner.with_synthetic_responses_fixture()
        } else {
            bridge_owner
        };
        bridge_owner.listen(root)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::Database, live::engine::DeviceStore, secrets::upgrade};
    use std::sync::Arc;
    #[test]
    #[serial_test::serial]
    fn original_ready_authority_controls_native_listener_and_stop() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _entered = rt.enter();
        let root = crate::paperclip::tests::private_root();
        let device = DeviceStore::at(root.path().join("device"));
        let c = StartupCoordinator::new(
            root.path().into(),
            upgrade::inspect(root.path(), &device).unwrap(),
        );
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone()).unwrap();
        assert!(c
            .start_paperclip_bridge(&state, AppType::Codex, "owner-a", root.path())
            .is_err());
        c.run_attempt(|| Ok(db.secrets.clone()), |_| Ok(()))
            .unwrap();
        let bridge = c
            .start_paperclip_bridge(&state, AppType::Codex, "owner-a", root.path())
            .unwrap();
        assert!(root.path().join("owner.sock").exists());
        bridge.stop().unwrap();
        assert!(!root.path().join("owner.sock").exists());
        *c.phase.lock().unwrap() = Phase::Failed;
        assert!(c
            .start_paperclip_bridge(&state, AppType::Codex, "owner-a", root.path())
            .is_err());
    }
}

/// Listener lifecycle only. Original Phase remains the application Ready authority.
#[derive(Default)]
pub(super) struct ListenerSlot {
    bridge: Option<Bridge>,
    closed: bool,
    registration: Option<(AppType, String, std::path::PathBuf)>,
}
impl StartupCoordinator {
    fn register_paperclip_bridge(
        &self,
        state: &AppState,
        app: AppType,
        owner: &str,
        root: &Path,
    ) -> Result<(), AppError> {
        let mut slot = self
            .paperclip_bridge
            .lock()
            .map_err(|_| AppError::Config("paperclip.owner_unavailable".into()))?;
        if slot.closed {
            return Err(AppError::Config("paperclip.owner_unavailable".into()));
        }
        let registration = (app.clone(), owner.to_owned(), root.to_path_buf());
        if slot
            .registration
            .as_ref()
            .is_some_and(|old| old != &registration)
        {
            return Err(AppError::Config("paperclip.owner_unavailable".into()));
        }
        if slot.bridge.is_none() {
            slot.bridge = Some(self.start_paperclip_bridge(state, app, owner, root)?);
            slot.registration = Some(registration);
        }
        Ok(())
    }
    pub(crate) fn paperclip_stop_bridge(&self) -> Result<(), AppError> {
        // Keep the slot through join: every concurrent exit caller shares the barrier.
        // Worker never takes this slot or calls GUI code.
        let mut slot = self
            .paperclip_bridge
            .lock()
            .map_err(|_| AppError::Config("paperclip.owner_unavailable".into()))?;
        slot.closed = true;
        if let Some(bridge) = slot.bridge.take() {
            bridge.stop()?;
        }
        Ok(())
    }
    #[cfg(feature = "gui")]
    pub(super) fn paperclip_after_ready(&self, app: &tauri::AppHandle) -> Result<(), String> {
        use tauri::Manager;
        // Explicit private-launch configuration only. Never consult user files.
        let launch = NativeLaunch::from_environment().map_err(|_| "paperclip.owner_unavailable")?;
        if launch.is_none() {
            return Ok(());
        }
        match super::super::upgrade::checkpoint::ensure_no_pending_checkpoint(
            &crate::live::engine::DeviceStore::for_device(),
        ) {
            Ok(()) => {}
            Err(AppError::Config(code)) if code == "upgrade.checkpoint_pending" => {
                // Upgrade review intentionally runs with the old authenticated
                // database. Keep that successful Ready path without a bridge.
                return Ok(());
            }
            Err(_) => return Err("paperclip.owner_unavailable".into()),
        }
        let state = app
            .try_state::<AppState>()
            .ok_or("paperclip.owner_unavailable")?;
        self.paperclip_launch(&state, launch)
            .map_err(|_| "paperclip.owner_unavailable".into())
    }
}
#[cfg(feature = "gui")]
pub(crate) fn stop_for_app(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(c) = app.try_state::<StartupCoordinator>() {
        if let Err(error) = c.paperclip_stop_bridge() {
            log::error!("Paperclip listener stop failed: {error}");
        }
    }
}
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::{database::Database, live::engine::DeviceStore, secrets::upgrade};
    use std::sync::Arc;
    #[test]
    #[serial_test::serial]
    fn paperclip_exit_during_prepare_cannot_reopen_listener() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let root = crate::paperclip::tests::private_root();
        let path = std::fs::canonicalize(root.path()).unwrap();
        let c = StartupCoordinator::new(
            path.clone(),
            upgrade::inspect(&path, &DeviceStore::at(path.join("device"))).unwrap(),
        );
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone()).unwrap();
        c.run_attempt(
            || Ok(db.secrets.clone()),
            |_| c.paperclip_stop_bridge().map_err(|error| error.to_string()),
        )
        .unwrap();
        assert!(*c.phase.lock().unwrap() == Phase::Ready);
        assert!(c
            .register_paperclip_bridge(&state, AppType::Codex, "owner-a", &path)
            .is_err());
        assert!(!path.join("owner.sock").exists());
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_ready_registration_is_idempotent_and_exit_is_terminal() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _enter = rt.enter();
        let root = crate::paperclip::tests::private_root();
        let path = std::fs::canonicalize(root.path()).unwrap();
        let c = StartupCoordinator::new(
            path.clone(),
            upgrade::inspect(&path, &DeviceStore::at(path.join("device"))).unwrap(),
        );
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone()).unwrap();
        assert!(c
            .register_paperclip_bridge(&state, AppType::Codex, "owner-a", &path)
            .is_err());
        c.run_attempt(|| Ok(db.secrets.clone()), |_| Ok(()))
            .unwrap();
        c.register_paperclip_bridge(&state, AppType::Codex, "owner-a", &path)
            .unwrap();
        c.register_paperclip_bridge(&state, AppType::Codex, "owner-a", &path)
            .unwrap();
        assert!(c
            .register_paperclip_bridge(&state, AppType::Codex, "different-owner", &path)
            .is_err());
        c.paperclip_stop_bridge().unwrap();
        c.paperclip_stop_bridge().unwrap();
        assert!(!path.join("owner.sock").exists());
        assert!(c
            .register_paperclip_bridge(&state, AppType::Codex, "owner-a", &path)
            .is_err());
    }
    #[test]
    #[serial_test::serial]
    fn paperclip_failed_prepare_never_registers_listener() {
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _enter = rt.enter();
        let root = crate::paperclip::tests::private_root();
        let path = std::fs::canonicalize(root.path()).unwrap();
        let c = StartupCoordinator::new(
            path.clone(),
            upgrade::inspect(&path, &DeviceStore::at(path.join("device"))).unwrap(),
        );
        let db = Arc::new(Database::memory().unwrap());
        let state = AppState::new(db.clone()).unwrap();
        assert!(c
            .run_attempt(
                || Ok(db.secrets.clone()),
                |_| Err("synthetic failure".into())
            )
            .is_err());
        assert!(c
            .register_paperclip_bridge(&state, AppType::Codex, "owner-a", &path)
            .is_err());
        assert!(!path.join("owner.sock").exists());
    }
}

#[cfg(test)]
mod wiring_tests {
    fn section<'a>(source: &'a str, begin: &str, end: &str) -> &'a str {
        source
            .split_once(begin)
            .unwrap()
            .1
            .split_once(end)
            .unwrap()
            .0
    }

    #[test]
    fn paperclip_four_ready_paths_use_original_phase_before_shared_launch() {
        let startup = include_str!("startup.rs");
        for (begin, end, authority) in [
            (
                "fn initialize(",
                "fn initialize_with",
                "self.initialize_with(",
            ),
            (
                "async fn continue_startup_upgrade(",
                "async fn complete_startup_upgrade(",
                "coordinator.run_upgrade_attempt(",
            ),
            (
                "async fn complete_startup_upgrade(",
                "pub(crate) enum UpgradeSyncChoice",
                "coordinator.complete_upgrade_checkpoint(",
            ),
            (
                "async fn restore_startup_vault(",
                "fn present_error(",
                "Ok(()) => Phase::Ready",
            ),
        ] {
            let path = section(startup, begin, end);
            assert_eq!(path.matches("paperclip_after_ready(").count(), 1, "{begin}");
            assert!(
                path.find(authority).unwrap() < path.find("paperclip_after_ready(").unwrap(),
                "{begin}"
            );
        }
        let prepare = section(
            startup,
            "fn prepare_runtime(",
            "async fn preview_startup_restore(",
        );
        assert!(prepare.contains("runtime-ready"));
        assert!(!prepare.contains("paperclip_after_ready("));
    }

    #[test]
    fn paperclip_real_exit_paths_join_before_early_returns_and_tray_keeps_listener() {
        let lib = include_str!("../lib.rs");
        let stop = "paperclip_ready::stop_for_app(";
        let tray = section(
            lib,
            "ExitRequestAction::StayInTray => {",
            "ExitRequestAction::DeferToTauriRestart => {",
        );
        assert!(!tray.contains(stop));
        let restart_event = section(
            lib,
            "ExitRequestAction::DeferToTauriRestart => {",
            "ExitRequestAction::CleanupAndExit => {}",
        );
        assert!(restart_event.find(stop).unwrap() < restart_event.find("return;").unwrap());
        let cleanup = section(
            lib,
            "pub async fn cleanup_before_exit(",
            "/// 主动释放 single-instance 锁",
        );
        assert!(cleanup.find(stop).unwrap() < cleanup.find("return;").unwrap());
        let restart = section(
            lib,
            "pub fn restart_process(",
            "/// macOS 经 LaunchServices",
        );
        assert!(
            restart.find(stop).unwrap() < restart.find("remove_tray_icon_before_exit(").unwrap()
        );
    }
}

#[cfg(test)]
mod concurrent_stop_tests {
    use super::*;
    #[test]
    #[serial_test::serial]
    fn paperclip_concurrent_exit_callers_wait_for_same_worker_completion() {
        let root = crate::paperclip::tests::private_root();
        let path = std::fs::canonicalize(root.path()).unwrap();
        let c = std::sync::Arc::new(StartupCoordinator::new(
            path.clone(),
            crate::secrets::upgrade::inspect(
                &path,
                &crate::live::engine::DeviceStore::at(path.join("device")),
            )
            .unwrap(),
        ));
        let (bridge, release, stopping) = crate::paperclip::tests::gated_shutdown_fixture(&path);
        c.paperclip_bridge.lock().unwrap().bridge = Some(bridge);
        let first = {
            let c = c.clone();
            std::thread::spawn(move || c.paperclip_stop_bridge())
        };
        stopping
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (done, observe) = std::sync::mpsc::channel();
        let second = {
            let c = c.clone();
            std::thread::spawn(move || {
                let result = c.paperclip_stop_bridge();
                done.send(()).unwrap();
                result
            })
        };
        assert!(matches!(
            observe.recv_timeout(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        release.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert!(!path.join("owner.sock").exists());
    }
}

#[cfg(test)]
mod process_fixture {
    use super::*;
    use crate::{
        database::Database, live::engine::DeviceStore, provider::Provider, secrets::upgrade,
    };
    use std::sync::Arc;
    /// Child entry only, not an acceptance test. Parent invokes this exact ignored
    /// fixture to exercise original owners via an independent real UDS process.
    /// Original ephemeral test sessions are used; production Vault auth is NOT tested.
    #[test]
    #[serial_test::serial]
    #[ignore = "explicit synthetic child process fixture"]
    fn paperclip_native_process() {
        assert_eq!(std::env::var("PC_NATIVE_FIXTURE_ENABLE").unwrap(), "1");
        let root = std::path::PathBuf::from(std::env::var_os("PC_NATIVE_ROOT").unwrap());
        assert_eq!(std::fs::canonicalize(&root).unwrap(), root);
        let launch = NativeLaunch::from_environment()
            .unwrap()
            .expect("explicit native launch required");
        assert_eq!(launch.root, root);
        assert_eq!(launch.owner, "owner-a");
        let _home = crate::secrets::testing::TestHome::new().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _entered = rt.enter();
        let seed = Database::memory().unwrap();
        let file = root.join("synthetic.sqlite");
        let first = !file.exists();
        if !first {
            assert!(!std::fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink());
        }
        let mut disk = rusqlite::Connection::open(&file).unwrap();
        let fixture_mode = std::env::var("PC_NATIVE_FAULT").unwrap_or_default();
        let synthetic = matches!(
            fixture_mode.as_str(),
            "synthetic-responses" | "synthetic-responses-pending" | "synthetic-responses-crash"
        );
        let synthetic_url = if synthetic {
            rt.block_on(async {
                use axum::{routing::post, Router, Json};
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let address = listener.local_addr().unwrap();
                let hits = root.join("upstream-hits.json");
                let pending = fixture_mode == "synthetic-responses-pending";
                let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let app = Router::new().route("/v1/responses", post(move |Json(body): Json<serde_json::Value>| {
                    let hits=hits.clone();let count=count.clone();async move {
                        assert_eq!(body["model"], "synthetic-model");assert!(body.get("tools").is_none());
                        let n=count.fetch_add(1,std::sync::atomic::Ordering::SeqCst)+1;
                        std::fs::write(hits,serde_json::to_vec(&serde_json::json!({"requests":n,"syntheticOnly":true})).unwrap()).unwrap();
                        if pending {tokio::time::sleep(std::time::Duration::from_secs(20)).await;}
                        Json(serde_json::json!({"id":"synthetic-response","object":"response","status":"completed","model":"synthetic-model","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}))
                    }
                }));
                tokio::spawn(async move {axum::serve(listener,app).await.unwrap()});
                format!("http://{address}/v1")
            })
        } else {
            String::new()
        };
        if first {
            seed.save_provider(
                "codex",
                &Provider::with_id(
                    "synthetic".into(),
                    "Synthetic".into(),
                    if synthetic {serde_json::json!({"base_url":synthetic_url,"auth":{"OPENAI_API_KEY":"SYNTHETIC_NOT_REAL"}})} else {serde_json::json!({})},
                    None,
                ),
            )
            .unwrap();
            let source = seed.conn.lock().unwrap();
            let backup = rusqlite::backup::Backup::new(&source, &mut disk).unwrap();
            backup
                .run_to_completion(32, std::time::Duration::from_millis(1), None)
                .unwrap();
        }
        let db = Arc::new(Database::from_connection(disk, seed.secrets.clone()));
        let state = AppState::new(db.clone()).unwrap();
        if synthetic {
            db.save_provider("codex", &Provider::with_id("synthetic".into(),"Synthetic".into(),serde_json::json!({"base_url":synthetic_url,"auth":{"OPENAI_API_KEY":"SYNTHETIC_NOT_REAL"}}),None)).unwrap();
            db.set_current_provider("codex", "synthetic").unwrap();
            rt.block_on(
                state
                    .proxy_service
                    .paperclip_install_original_test_server(true),
            );
        }
        let c = StartupCoordinator::new(
            root.clone(),
            upgrade::inspect(&root, &DeviceStore::at(root.join("device"))).unwrap(),
        );
        c.run_attempt(|| Ok(db.secrets.clone()), |_| Ok(()))
            .unwrap();
        c.paperclip_launch(&state, Some(launch)).unwrap();
        println!("PC_NATIVE_LAUNCH=shared-ready-entry");
        println!("PC_NATIVE_READY");
        let mut command = String::new();
        std::io::stdin().read_line(&mut command).unwrap();
        assert_eq!(command.trim(), "STOP");
        c.paperclip_stop_bridge().unwrap();
        println!("PC_NATIVE_STOPPED");
    }
}
