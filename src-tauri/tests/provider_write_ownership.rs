//! Linux-only provider projection regressions using real services and SQLite.
//! The FIFO pauses exactly one real live-config read after the real SQLite
//! toggle commit. It supplies the pre-write snapshot an ordinary opened file
//! would retain across another writer's atomic rename. No service/DAO is mocked.

#![cfg(target_os = "linux")]

use cc_switch_lib::{AppState, AppType, Database, McpApps, McpServer, McpService};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{mpsc, Arc, MutexGuard},
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};

#[path = "support.rs"]
mod support;

fn fixture() -> (MutexGuard<'static, ()>, AppState, PathBuf) {
    let guard = support::test_mutex()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    support::reset_test_fs();
    let home = support::ensure_test_home();
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    fs::create_dir_all(home.join(".claude")).unwrap();
    let path = cc_switch_lib::get_claude_mcp_path();
    assert_eq!(path, home.join(".claude.json"));
    let state = AppState::new(Arc::new(Database::memory().unwrap())).unwrap();
    (guard, state, path)
}

fn server(id: &str, command: &str, enabled: bool) -> McpServer {
    McpServer {
        id: id.into(),
        name: id.into(),
        server: json!({"command": command, "args": []}),
        apps: McpApps {
            claude: enabled,
            ..Default::default()
        },
        description: None,
        homepage: None,
        docs: None,
        tags: vec![],
    }
}

/// Current commands finish despite the held toggle lock. Correctly serialized
/// commands return Pending, and finish only after that toggle is released.
/// Polling the actual command future makes both schedules deterministic without
/// sleeps, time-based assertions, a mock DAO, or a production test hook.
fn overlap_command<T: std::fmt::Debug>(
    toggle: &mut PausedToggle,
    command: impl Future<Output = Result<T, String>>,
) -> T {
    let mut command = Box::pin(command);
    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    let first_poll = command.as_mut().poll(&mut context);
    eprintln!(
        "command_before_toggle_release={}",
        if first_poll.is_ready() {
            "Ready"
        } else {
            "Pending"
        }
    );
    toggle
        .release()
        .expect("toggle completes after FIFO release");
    match first_poll {
        Poll::Ready(result) => result.unwrap(),
        Poll::Pending => match command.as_mut().poll(&mut context) {
            Poll::Ready(result) => result.unwrap(),
            Poll::Pending => {
                panic!("command remains pending after its only competing writer completed")
            }
        },
    }
}

/// Return a writer to the FIFO once toggle has opened its first live read.
/// Keep this descriptor open without bytes so that read_to_string cannot finish.
struct PausedToggle {
    writer: Option<File>,
    result: mpsc::Receiver<Result<(), String>>,
    thread: Option<thread::JoinHandle<()>>,
    snapshot: &'static [u8],
}

impl PausedToggle {
    fn release(&mut self) -> Result<(), String> {
        if let Some(mut writer) = self.writer.take() {
            writer.write_all(self.snapshot).map_err(|e| e.to_string())?;
            drop(writer);
        }
        let result = self
            .result
            .recv_timeout(Duration::from_secs(10))
            .map_err(|e| format!("toggle failed to complete after releasing read: {e}"))?;
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| "toggle thread panicked")?;
        result
    }
}

impl Drop for PausedToggle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            // Release the gate even if an intermediate assertion fails.
            // An unexpected stuck service cannot hang the harness indefinitely.
            if let Err(error) = self.release() {
                eprintln!("repro cleanup failure: {error}");
            }
        }
    }
}

fn paused_toggle_for_app(
    state: &AppState,
    path: &Path,
    app: AppType,
    snapshot: &'static [u8],
) -> PausedToggle {
    assert!(Command::new("mkfifo").arg(path).status().unwrap().success());
    let state_for_toggle = state.clone();
    let toggle_app = app.clone();
    let (tx, rx) = mpsc::channel();
    let toggle = thread::spawn(move || {
        let result = futures::executor::block_on(McpService::toggle_app_locked(
            &state_for_toggle,
            "target",
            toggle_app,
            true,
        ))
        .map_err(|error| error.to_string());
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let writer = loop {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Ok(writer) => break writer,
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                if let Ok(result) = rx.try_recv() {
                    toggle.join().unwrap();
                    panic!("toggle returned before reaching the live-read gate: {result:?}");
                }
                if Instant::now() >= deadline {
                    // A nonblocking RDWR opener releases a late FIFO open/read.
                    let mut rescue = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(path)
                        .unwrap();
                    let _ = rescue.write_all(snapshot);
                    drop(rescue);
                    let _ = rx.recv_timeout(Duration::from_secs(1));
                    panic!("fixture timeout: toggle never reached its first live read");
                }
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("FIFO fixture failed: {error}"),
        }
    };
    let paused = PausedToggle {
        writer: Some(writer),
        result: rx,
        thread: Some(toggle),
        snapshot,
    };
    assert!(
        state.db.get_all_mcp_servers().unwrap()["target"]
            .apps
            .is_enabled_for(&app),
        "the real SQLite flag commit precedes this read"
    );
    let mut lock = Box::pin(McpService::toggle_app_locked(state, "missing", app, false));
    let waker = futures::task::noop_waker();
    assert!(
        matches!(
            lock.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ),
        "toggle still owns the real per-app switch lock"
    );
    drop(lock);
    fs::remove_file(path).unwrap();
    fs::write(path, snapshot).unwrap();
    paused
}

fn codex_fixture() -> (MutexGuard<'static, ()>, AppState, PathBuf) {
    let (guard, state, _) = fixture();
    let path = seed_codex(&state);
    (guard, state, path)
}

fn seed_codex(state: &AppState) -> PathBuf {
    let path = cc_switch_lib::get_codex_config_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    let mut provider = cc_switch_lib::Provider::with_id(
        "provider".into(),
        "Fixture Provider".into(),
        json!({"config": "model = \"new-model\"\n", "auth": {"OPENAI_API_KEY": "synthetic-fixture-key"}}),
        None,
    );
    provider.category = Some("official".into());
    state.db.save_provider("codex", &provider).unwrap();
    state.db.set_current_provider("codex", "provider").unwrap();
    let mut target = server("target", "fixture-mcp", false);
    target.apps.claude = false;
    state.db.save_mcp_server(&target).unwrap();
    path
}

fn assert_provider_projection(state: &AppState, path: &Path) {
    let stored = state
        .db
        .get_provider_by_id("provider", "codex")
        .unwrap()
        .unwrap();
    let live = fs::read_to_string(path).unwrap();
    eprintln!(
        "SQLite provider={}, SQLite MCP codex={}, live={}",
        stored.settings_config,
        state.db.get_all_mcp_servers().unwrap()["target"].apps.codex,
        live
    );
    assert!(stored.settings_config["config"]
        .as_str()
        .unwrap()
        .contains("new-model"));
    assert!(state.db.get_all_mcp_servers().unwrap()["target"].apps.codex);
    assert!(live.contains("fixture-mcp"));
    assert!(
        live.contains("new-model"),
        "completed provider projection must survive concurrent MCP toggle"
    );
}

#[derive(Clone, Copy, Debug)]
enum Refresh {
    Single,
    Official,
    All,
}
async fn refresh(state: &AppState, kind: Refresh) -> Result<(), String> {
    match kind {
        Refresh::Single => {
            cc_switch_lib::ProviderService::sync_current_provider_for_app(state, AppType::Codex)
                .await
        }
        Refresh::Official => cc_switch_lib::reapply_current_codex_official_live(state)
            .await
            .map(|_| ()),
        Refresh::All => cc_switch_lib::ProviderService::sync_current_to_live(state).await,
    }
    .map_err(|e| e.to_string())
}
fn preserves_projection(kind: Refresh) {
    let (_guard, state, path) = codex_fixture();
    let mut toggle =
        paused_toggle_for_app(&state, &path, AppType::Codex, b"model = \"old-model\"\n");
    overlap_command(&mut toggle, refresh(&state, kind));
    assert_provider_projection(&state, &path);
}
#[test]
fn single_provider_refresh_must_survive_mcp_toggle() {
    preserves_projection(Refresh::Single);
}
#[test]
fn official_reapply_must_survive_mcp_toggle() {
    preserves_projection(Refresh::Official);
}
#[test]
fn full_provider_refresh_must_survive_mcp_toggle() {
    preserves_projection(Refresh::All);
}

#[test]
fn every_refresh_yields_to_its_lock_releaser_on_current_thread() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for kind in [Refresh::Single, Refresh::Official, Refresh::All] {
        let (_guard, state, path) = codex_fixture();
        let mut toggle =
            paused_toggle_for_app(&state, &path, AppType::Codex, b"model = \"old-model\"\n");
        runtime.block_on(async {
            let mut writer = Box::pin(refresh(&state, kind));
            let waker = futures::task::noop_waker();
            assert!(
                matches!(
                    writer.as_mut().poll(&mut Context::from_waker(&waker)),
                    Poll::Pending
                ),
                "{kind:?} must wait before provider snapshot/native write"
            );
            let release = async {
                toggle.release().unwrap();
            };
            let (result, ()) = tokio::join!(biased; writer, release);
            result.unwrap();
        });
        assert_provider_projection(&state, &path);
    }
}

#[test]
fn every_refresh_preserves_takeover_and_updates_backup_without_reentry() {
    let (_guard, state, path) = codex_fixture();
    let taken_over = "model = \"proxy-model\"\n";
    fs::write(&path, taken_over).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for kind in [Refresh::Single, Refresh::Official, Refresh::All] {
            state.db.save_live_backup("codex", r#"{"config":"model = \"backup-model\"\n","auth":{"OPENAI_API_KEY":"synthetic-backup"}}"#).await.unwrap();
            let mut projection = Box::pin(refresh(&state, kind));
            let waker = futures::task::noop_waker();
            assert!(matches!(projection.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Ready(Ok(()))),
                "{kind:?} must finish the takeover projection in the same poll after ownership");
            assert_eq!(fs::read_to_string(&path).unwrap(), taken_over, "{kind:?} must not rewrite takeover-owned live");
            let backup = state.db.get_live_backup("codex").await.unwrap().unwrap();
            let backup: Value = serde_json::from_str(&backup.original_config).unwrap();
            assert!(backup["config"].as_str().unwrap().contains("new-model"));
        }
    });
}

fn additive_switch_preserves_mcp(app: AppType) {
    let (_guard, state, _) = fixture();
    let home = PathBuf::from(std::env::var_os("CC_SWITCH_TEST_HOME").unwrap());
    let (path, key, provider_config, live_seed) = match app {
        AppType::OpenCode => (
            home.join(".config/opencode/opencode.json"),
            "mcp",
            json!({"npm":"@ai-sdk/openai-compatible","name":"Native provider","options":{"baseURL":"https://fixture.invalid/v1","apiKey":"synthetic-fixture"},"models":{"fixture-model":{"name":"Fixture model"}}}),
            json!({"provider":{},"mcp":{"unmanaged":{"type":"local","command":["unmanaged-command"],"enabled":true}}}),
        ),
        AppType::Hermes => (
            cc_switch_lib::hermes_config::get_hermes_config_path(),
            "mcp_servers",
            json!({"api":"openai-chat","base_url":"https://fixture.invalid/v1","api_key":"synthetic-fixture","models":{"fixture-model":{"name":"Fixture model"}}}),
            json!({"custom_providers":[],"mcp_servers":{"unmanaged":{"command":"unmanaged-command","enabled":true}}}),
        ),
        _ => unreachable!(),
    };
    assert!(path.starts_with(&home));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Seed the native block-YAML form for the section-preserving Hermes writer.
    let seed = if app == AppType::Hermes {
        serde_yaml::to_string(&live_seed).unwrap()
    } else {
        serde_json::to_string_pretty(&live_seed).unwrap()
    };
    fs::write(&path, seed).unwrap();
    let mut target = server("target", "fixture-mcp", false);
    target.apps.set_enabled_for(&app, true);
    futures::executor::block_on(McpService::upsert_server(&state, target)).unwrap();
    let provider = cc_switch_lib::Provider::with_id(
        "native-provider".into(),
        "Native provider".into(),
        provider_config,
        None,
    );
    state.db.save_provider(app.as_str(), &provider).unwrap();
    for _ in 0..2 {
        cc_switch_lib::ProviderService::switch(&state, app.clone(), &provider.id).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let live: Value = serde_yaml::from_str(&text).unwrap();
        assert!(
            live[key].get("target").is_some(),
            "{app:?} switch preserves managed MCP"
        );
        assert!(
            live[key].get("unmanaged").is_some(),
            "{app:?} switch preserves unmanaged MCP"
        );
        assert!(text.contains("native-provider"));
    }
}

#[test]
fn opencode_switch_retains_native_merge_and_mcp_semantics() {
    additive_switch_preserves_mcp(AppType::OpenCode);
}

#[test]
fn hermes_switch_retains_native_merge_and_mcp_semantics() {
    additive_switch_preserves_mcp(AppType::Hermes);
}

#[test]
fn cancelling_full_refresh_after_codex_keeps_its_mcp_projection() {
    let (_guard, state, codex_path) = codex_fixture();
    fs::write(&codex_path, "model = \"old-model\"\n").unwrap();
    futures::executor::block_on(McpService::toggle_app_locked(
        &state,
        "target",
        AppType::Codex,
        true,
    ))
    .unwrap();
    let home = PathBuf::from(std::env::var_os("CC_SWITCH_TEST_HOME").unwrap());
    let gemini_path = home.join(".gemini/settings.json");
    fs::create_dir_all(gemini_path.parent().unwrap()).unwrap();
    if gemini_path.exists() {
        fs::remove_file(&gemini_path).unwrap();
    }
    let mut toggle = paused_toggle_for_app(&state, &gemini_path, AppType::Gemini, b"{}");
    let mut writer = Box::pin(cc_switch_lib::ProviderService::sync_current_to_live(&state));
    let waker = futures::task::noop_waker();
    assert!(matches!(
        writer.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    let before_cancel = fs::read_to_string(&codex_path).unwrap();
    assert!(
        before_cancel.contains("new-model"),
        "full refresh has completed the earlier Codex provider write"
    );
    drop(writer);
    toggle.release().unwrap();
    let live = fs::read_to_string(&codex_path).unwrap();
    assert!(state.db.get_all_mcp_servers().unwrap()["target"].apps.codex);
    assert!(live.contains("fixture-mcp"), "cancelling while waiting for a later app must not leave Codex without its enabled MCP: {live}");
}

#[test]
fn full_refresh_projects_mcp_even_without_a_current_provider() {
    let (_guard, state, path) = codex_fixture();
    state.db.delete_provider("codex", "provider").unwrap();
    futures::executor::block_on(McpService::toggle_app_locked(
        &state,
        "target",
        AppType::Codex,
        true,
    ))
    .unwrap();
    fs::remove_file(&path).unwrap();
    futures::executor::block_on(cc_switch_lib::ProviderService::sync_current_to_live(&state))
        .unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("fixture-mcp"));
    assert!(state.db.get_all_providers("codex").unwrap().is_empty());
}

#[test]
fn full_refresh_aggregates_broken_app_but_finishes_other_app_projection() {
    let (_guard, state, path) = codex_fixture();
    fs::write(&path, "model = \"old-model\"\n").unwrap();
    futures::executor::block_on(McpService::toggle_app_locked(
        &state,
        "target",
        AppType::Codex,
        true,
    ))
    .unwrap();
    fs::write(cc_switch_lib::get_claude_mcp_path(), "{ broken JSON").unwrap();
    let error =
        futures::executor::block_on(cc_switch_lib::ProviderService::sync_current_to_live(&state))
            .unwrap_err();
    assert!(
        error.to_string().contains("claude"),
        "the broken app must remain visible: {error}"
    );
    assert_provider_projection(&state, &path);
}

#[test]
fn cancelled_imagegen_toggle_does_not_commit_setting_before_ownership() {
    use tauri::Manager;
    let (_guard, state, path) = codex_fixture();
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    futures::executor::block_on(cc_switch_lib::relay_set_imagegen_mcp_enabled(
        app.state(),
        true,
    ))
    .unwrap();
    let mut toggle =
        paused_toggle_for_app(&state, &path, AppType::Codex, b"model = \"old-model\"\n");
    let mut command = Box::pin(cc_switch_lib::relay_set_imagegen_mcp_enabled(
        app.state(),
        false,
    ));
    let waker = futures::task::noop_waker();
    assert!(matches!(
        command.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    let settings = futures::executor::block_on(cc_switch_lib::get_settings()).unwrap();
    assert_eq!(
        settings.imagegen_mcp_enabled,
        Some(true),
        "a command waiting for MCP ownership must not persist its setting yet"
    );
    drop(command);
    toggle.release().unwrap();
    assert_eq!(
        futures::executor::block_on(cc_switch_lib::get_settings())
            .unwrap()
            .imagegen_mcp_enabled,
        Some(true)
    );
}

#[test]
fn cancelled_settings_save_does_not_commit_before_ownership() {
    use tauri::Manager;
    let (_guard, state, path) = codex_fixture();
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let previous = futures::executor::block_on(cc_switch_lib::get_settings()).unwrap();
    let mut incoming = previous.clone();
    incoming.unify_codex_session_history = !previous.unify_codex_session_history;
    incoming.unify_codex_migrate_existing = Some(false);
    let mut toggle =
        paused_toggle_for_app(&state, &path, AppType::Codex, b"model = \"old-model\"\n");
    let mut command = Box::pin(cc_switch_lib::save_settings(app.state(), incoming));
    let waker = futures::task::noop_waker();
    assert!(matches!(
        command.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    assert_eq!(
        futures::executor::block_on(cc_switch_lib::get_settings())
            .unwrap()
            .unify_codex_session_history,
        previous.unify_codex_session_history,
        "waiting must not persist the Codex setting"
    );
    drop(command);
    toggle.release().unwrap();
    assert_eq!(
        futures::executor::block_on(cc_switch_lib::get_settings())
            .unwrap()
            .unify_codex_session_history,
        previous.unify_codex_session_history
    );
}

#[test]
fn cancelled_snippet_write_does_not_commit_before_ownership() {
    use tauri::Manager;
    let (_guard, unused, _) = fixture();
    drop(unused);
    let state = support::create_source17_test_state().unwrap();
    let path = seed_codex(&state);
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let previous = state.db.get_config_snippet("codex").unwrap();
    let mut toggle =
        paused_toggle_for_app(&state, &path, AppType::Codex, b"model = \"old-model\"\n");
    let mut command = Box::pin(cc_switch_lib::set_common_config_snippet(
        "codex".into(),
        "model_reasoning_effort = \"high\"\n".into(),
        app.state(),
    ));
    let waker = futures::task::noop_waker();
    assert!(matches!(
        command.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Pending
    ));
    assert_eq!(
        state.db.get_config_snippet("codex").unwrap(),
        previous,
        "waiting must not commit the common snippet"
    );
    drop(command);
    toggle.release().unwrap();
    assert_eq!(state.db.get_config_snippet("codex").unwrap(), previous);
}

#[test]
fn uncontended_local_refresh_completes_in_one_poll() {
    let (_guard, state, path) = codex_fixture();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for kind in [Refresh::Single, Refresh::Official, Refresh::All] {
            let mut projection = Box::pin(refresh(&state, kind));
            let waker = futures::task::noop_waker();
            assert!(
                matches!(
                    projection.as_mut().poll(&mut Context::from_waker(&waker)),
                    Poll::Ready(Ok(()))
                ),
                "{kind:?} must not yield between local configuration commit and projection"
            );
            assert!(fs::read_to_string(&path).unwrap().contains("new-model"));
        }
    });
}

#[test]
fn snippet_schema_admission_is_checked_under_app_ownership() {
    use tauri::Manager;
    let (_guard, unused, _) = fixture();
    drop(unused);
    let state = support::create_direct_test_state(AppType::Codex).unwrap();
    let path = seed_codex(&state);
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let previous = state.db.get_config_snippet("codex").unwrap();
    let mut toggle =
        paused_toggle_for_app(&state, &path, AppType::Codex, b"model = \"old-model\"\n");
    let mut command = Box::pin(cc_switch_lib::set_common_config_snippet(
        "codex".into(),
        "model_reasoning_effort = \"high\"\n".into(),
        app.state(),
    ));
    let waker = futures::task::noop_waker();
    assert!(
        matches!(
            command.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Pending
        ),
        "schema admission must be sampled only after ownership, not before a possible upgrade wait"
    );
    toggle.release().unwrap();
    assert!(
        matches!(command.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Err(ref error)) if error == "mode.legacy_common_config_frozen")
    );
    assert_eq!(state.db.get_config_snippet("codex").unwrap(), previous);
}

#[cfg(feature = "test-hooks")]
#[test]
fn real_schema_upgrade_rechecks_queued_snippet_and_preserves_freeze_contracts() {
    let _guard = support::test_mutex()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cc_switch_lib::ProviderService::verify_legacy_snippet_freeze().unwrap();
}
