//! Real database restore and native projection result contract, without real accounts.
use cc_switch_lib::{
    create_db_backup, restore_db_backup, AppState, Provider, APP_DIR_NAME, DB_FILE_NAME,
};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};
use tauri::Manager;

#[allow(dead_code)]
#[path = "support.rs"]
mod support;

// MockRuntime plugins can retain AppHandle cycles. Release managed state explicitly.
struct Fixture(tauri::App<tauri::test::MockRuntime>);
impl Fixture {
    fn state(&self) -> tauri::State<'_, AppState> {
        self.0.state::<AppState>()
    }
    fn backup_count(&self) -> usize {
        fs::read_dir(self.backups())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "db"))
            .count()
    }
    fn backups(&self) -> PathBuf {
        support::ensure_test_home()
            .join(APP_DIR_NAME)
            .join("backups")
    }
}
impl Drop for Fixture {
    #[allow(deprecated)]
    fn drop(&mut self) {
        // No handle/IPC consumer escapes; all operations are awaited before teardown.
        drop(self.0.unmanage::<AppState>());
    }
}
fn fixture() -> Fixture {
    support::reset_test_fs();
    let state = support::create_test_state().unwrap();
    let provider = Provider::with_id(
        "restore-fixture".into(),
        "Source".into(),
        json!({"env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-restore-canary","ANTHROPIC_BASE_URL":"https://restore.example"}}),
        None,
    );
    state.db.save_provider("claude", &provider).unwrap();
    state
        .db
        .set_current_provider("claude", &provider.id)
        .unwrap();
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    Fixture(app)
}
async fn source_backup(app: &Fixture) -> String {
    let filename = create_db_backup(app.state()).await.unwrap();
    let mut changed = app
        .state()
        .db
        .get_provider_by_id("restore-fixture", "claude")
        .unwrap()
        .unwrap();
    changed.name = "Before restore".into();
    app.state().db.save_provider("claude", &changed).unwrap();
    filename
}
fn assert_applied_once(app: &Fixture, source: &str, result: &Value, before_count: usize) {
    assert_eq!(
        result["success"], true,
        "successful DB apply must retain success even with a projection warning"
    );
    let id = result["backupId"]
        .as_str()
        .expect("structured safety backup ID");
    assert!(!id.is_empty());
    assert!(app.backups().join(source).exists());
    let safety = Connection::open_with_flags(
        app.backups().join(format!("{id}.db")),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let name: String = safety
        .query_row(
            "SELECT name FROM providers WHERE id='restore-fixture'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        name, "Before restore",
        "safety backup is the state actually replaced"
    );
    assert_eq!(
        app.state()
            .db
            .get_provider_by_id("restore-fixture", "claude")
            .unwrap()
            .unwrap()
            .name,
        "Source"
    );
    drop(safety);
    assert_eq!(
        app.backup_count(),
        before_count + 1,
        "projection error must not retry DB restoration or create another safety copy"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn committed_restore_reports_projection_warning_and_retains_safety_backup() {
    let app = fixture();
    let source = source_backup(&app).await;
    let before_count = app.backup_count();
    // An actual impossible native write, not a mocked projection result.
    let live = cc_switch_lib::get_claude_settings_path();
    fs::create_dir_all(&live).unwrap();
    let result = serde_json::to_value(
        restore_db_backup(app.state(), source.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        result["warning"]
            .as_str()
            .is_some_and(|w| w.contains("live configuration")),
        "committed DB with failed native projection must expose its warning: {result}"
    );
    assert_applied_once(&app, &source, &result, before_count);
    assert!(
        live.is_dir(),
        "failed projection must not destroy the conflicting entry"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn complete_restore_returns_safety_id_without_warning() {
    let app = fixture();
    let source = source_backup(&app).await;
    let before_count = app.backup_count();
    let result = serde_json::to_value(
        restore_db_backup(app.state(), source.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_applied_once(&app, &source, &result, before_count);
    assert!(
        result.get("warning").is_none(),
        "complete projection must not manufacture a warning: {result}"
    );
    let live: Value =
        serde_json::from_slice(&fs::read(cc_switch_lib::get_claude_settings_path()).unwrap())
            .unwrap();
    assert_eq!(
        live["env"]["ANTHROPIC_AUTH_TOKEN"],
        "synthetic-restore-canary"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn rejected_backup_returns_error_without_projection_or_safety_copy() {
    let app = fixture();
    fs::create_dir_all(app.backups()).unwrap();
    fs::write(app.backups().join("corrupt.db"), b"not a database").unwrap();
    let root = support::ensure_test_home().join(APP_DIR_NAME);
    let before_db = fs::read(root.join(DB_FILE_NAME)).unwrap();
    let before_vault = fs::read(root.join("vault.json")).unwrap();
    let before_count = app.backup_count();
    let live = cc_switch_lib::get_claude_settings_path();
    fs::create_dir_all(live.parent().unwrap()).unwrap();
    fs::write(&live, b"native-content-before-rejected-restore").unwrap();
    assert!(restore_db_backup(app.state(), "corrupt.db".into())
        .await
        .is_err());
    assert_eq!(fs::read(root.join(DB_FILE_NAME)).unwrap(), before_db);
    assert_eq!(fs::read(root.join("vault.json")).unwrap(), before_vault);
    assert_eq!(app.backup_count(), before_count);
    assert_eq!(
        fs::read(live).unwrap(),
        b"native-content-before-rejected-restore"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn projection_warning_does_not_expose_restored_configuration_secrets() {
    let app = fixture();
    let canary = "synthetic-restored-config-secret-canary";
    let codex = Provider::with_id(
        "invalid-codex-fixture".into(),
        "Invalid config fixture".into(),
        json!({"auth":{"OPENAI_API_KEY":"synthetic-auth-key"},
            "config":format!("model_provider = \"{canary}\n")}),
        None,
    );
    app.state().db.save_provider("codex", &codex).unwrap();
    app.state()
        .db
        .set_current_provider("codex", &codex.id)
        .unwrap();
    let source = source_backup(&app).await;
    let before_count = app.backup_count();
    let result = serde_json::to_value(
        restore_db_backup(app.state(), source.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    let warning = result["warning"]
        .as_str()
        .expect("bad native configuration must produce a warning");
    assert!(
        !warning.contains(canary),
        "warning must not expose raw parser source lines: {warning}"
    );
    assert!(!warning.contains("synthetic-auth-key"));
    assert_applied_once(&app, &source, &result, before_count);
}
