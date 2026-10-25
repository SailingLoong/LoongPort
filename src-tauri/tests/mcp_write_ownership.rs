//! Linux-only deterministic regression probe. No production test hooks are used.
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
    sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock},
    task::{Context, Poll},
    thread,
    time::{Duration, Instant},
};
use tauri::Manager;

fn fixture() -> (MutexGuard<'static, ()>, AppState, PathBuf) {
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HOME.get_or_init(|| {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "mcp-write-ownership-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&home).unwrap();
        std::env::set_var("HOME", &home);
        std::env::set_var("CC_SWITCH_TEST_HOME", &home);
        home
    });
    fs::create_dir_all(home.join(".claude")).unwrap();
    let path = cc_switch_lib::get_claude_mcp_path();
    assert_eq!(path, home.join(".claude.json"));
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
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

fn read_live(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn app(state: AppState) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
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

fn paused_toggle(state: &AppState, path: &Path) -> PausedToggle {
    paused_toggle_for_app(state, path, AppType::Claude, br#"{"mcpServers":{}}"#)
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

#[test]
fn toggle_must_not_overwrite_same_server_edit() {
    let (_guard, state, path) = fixture();
    let app = app(state.clone());
    state
        .db
        .save_mcp_server(&server("target", "old-command", false))
        .unwrap();
    let mut toggle = paused_toggle(&state, &path);
    overlap_command(
        &mut toggle,
        cc_switch_lib::upsert_mcp_server(app.state(), server("target", "edited-command", true)),
    );
    let stored = state.db.get_all_mcp_servers().unwrap();
    let live = read_live(&path);
    eprintln!("edit: SQLite={}, live={}", stored["target"].server, live);
    assert_eq!(stored["target"].server["command"], "edited-command");
    assert_eq!(
        live["mcpServers"]["target"]["command"], "edited-command",
        "a completed edit must not be replaced by toggle's older server snapshot"
    );
}

#[test]
fn toggle_must_not_resurrect_deleted_server() {
    let (_guard, state, path) = fixture();
    let app = app(state.clone());
    state
        .db
        .save_mcp_server(&server("target", "old-command", false))
        .unwrap();
    let mut toggle = paused_toggle(&state, &path);
    assert!(overlap_command(
        &mut toggle,
        cc_switch_lib::delete_mcp_server(app.state(), "target".into())
    ));
    let stored = state.db.get_all_mcp_servers().unwrap();
    let live = read_live(&path);
    eprintln!(
        "delete: SQLite_has_target={}, live={}",
        stored.contains_key("target"),
        live
    );
    assert!(!stored.contains_key("target"));
    assert!(
        live["mcpServers"].get("target").is_none(),
        "deleted MCP server must not be resurrected by an in-flight toggle"
    );
}

#[test]
fn toggle_must_preserve_other_server_added_to_same_app() {
    let (_guard, state, path) = fixture();
    let app = app(state.clone());
    state
        .db
        .save_mcp_server(&server("target", "old-command", false))
        .unwrap();
    let mut toggle = paused_toggle(&state, &path);
    overlap_command(
        &mut toggle,
        cc_switch_lib::upsert_mcp_server(app.state(), server("other", "other-command", true)),
    );
    let stored = state.db.get_all_mcp_servers().unwrap();
    let live = read_live(&path);
    eprintln!(
        "other-server: SQLite_keys={:?}, live={}",
        stored.keys().collect::<Vec<_>>(),
        live
    );
    assert!(stored["other"].apps.claude);
    assert_eq!(
        live["mcpServers"]["other"]["command"], "other-command",
        "same-app whole-file writers must not discard a different server"
    );
}

#[test]
fn multi_app_writer_yields_on_current_thread_until_later_app_is_released() {
    let (_guard, state, claude_path) = fixture();
    let app = app(state.clone());
    let codex_path = cc_switch_lib::get_codex_config_path();
    fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    if codex_path.exists() {
        fs::remove_file(&codex_path).unwrap();
    }
    state
        .db
        .save_mcp_server(&server("target", "codex-command", false))
        .unwrap();
    let mut toggle = paused_toggle_for_app(&state, &codex_path, AppType::Codex, b"");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let writer =
            cc_switch_lib::upsert_mcp_server(app.state(), server("other", "claude-command", true));
        let release = async {
            // The writer is first polled by the biased join. It must own Claude
            // and yield while awaiting the already-held later Codex lock.
            let mut claude = Box::pin(McpService::toggle_app_locked(
                &state,
                "missing",
                AppType::Claude,
                false,
            ));
            let waker = futures::task::noop_waker();
            assert!(
                matches!(
                    claude.as_mut().poll(&mut Context::from_waker(&waker)),
                    Poll::Pending
                ),
                "multi-app writer must acquire in the existing global app order"
            );
            drop(claude);
            eprintln!("current-thread release task ran while the multi-app writer was waiting");
            toggle.release().unwrap();
        };
        let (result, ()) = tokio::join!(biased; writer, release);
        result.unwrap();
    });
    assert_eq!(
        read_live(&claude_path)["mcpServers"]["other"]["command"],
        "claude-command"
    );
    assert!(fs::read_to_string(codex_path)
        .unwrap()
        .contains("codex-command"));
}

#[test]
fn cancelling_waiting_multi_app_writer_releases_already_acquired_app() {
    let (_guard, state, _) = fixture();
    let codex_path = cc_switch_lib::get_codex_config_path();
    fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    if codex_path.exists() {
        fs::remove_file(&codex_path).unwrap();
    }
    state
        .db
        .save_mcp_server(&server("target", "codex-command", false))
        .unwrap();
    let mut toggle = paused_toggle_for_app(&state, &codex_path, AppType::Codex, b"");
    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    let mut writer = Box::pin(McpService::upsert_server(
        &state,
        server("other", "never-written", true),
    ));
    assert!(matches!(writer.as_mut().poll(&mut context), Poll::Pending));
    let mut claude = Box::pin(McpService::toggle_app_locked(
        &state,
        "missing",
        AppType::Claude,
        false,
    ));
    assert!(matches!(claude.as_mut().poll(&mut context), Poll::Pending));
    drop(writer);
    assert!(matches!(
        claude.as_mut().poll(&mut context),
        Poll::Ready(Ok(()))
    ));
    toggle.release().unwrap();
    assert!(!state
        .db
        .get_all_mcp_servers()
        .unwrap()
        .contains_key("other"));
}
