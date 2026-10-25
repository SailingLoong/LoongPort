use std::{fs, future::Future, path::Path, path::PathBuf, sync::MutexGuard, time::Duration};

use cc_switch_lib::{sync_session_usage, AppState, APP_DIR_NAME, DB_FILE_NAME};
use rusqlite::{Connection, OptionalExtension};
use tauri::Manager;

#[path = "support.rs"]
mod support;

fn test_guard() -> MutexGuard<'static, ()> {
    // Keep independent fault cases runnable when an earlier RED assertion panics.
    support::test_mutex()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn fixture() -> (tauri::App<tauri::test::MockRuntime>, Connection, PathBuf) {
    support::reset_test_fs();
    let home = support::ensure_test_home();
    // The public command scans all clients. Environment overrides must also
    // remain inside the synthetic home, never point to real client data.
    std::env::set_var("OPENCODE_DB", home.join("absent-opencode.db"));
    let pi_sessions = home.join("empty-pi-sessions");
    fs::create_dir_all(&pi_sessions).unwrap();
    std::env::set_var("PI_CODING_AGENT_SESSION_DIR", pi_sessions);
    let state = support::create_test_state().unwrap();
    let conn = Connection::open(home.join(APP_DIR_NAME).join(DB_FILE_NAME)).unwrap();
    let project = home.join(".claude/projects/recovery-project");
    fs::create_dir_all(&project).unwrap();
    // MockRuntime supplies only the State container. The public command,
    // importer and on-disk SQLite database are the actual production code.
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (app, conn, project.join("session.jsonl"))
}

async fn within_deadline<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("public session sync must not deadlock")
}

fn usage_line(id: &str, input_tokens: u32) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "id": id,
            "model": "claude-sonnet-4-5",
            "usage": { "input_tokens": input_tokens, "output_tokens": 20 },
            "stop_reason": "end_turn"
        },
        "timestamp": "2026-10-20T12:00:00Z",
        "sessionId": "recovery-session"
    })
    .to_string()
}

fn write_log(path: &Path, contents: &str, modified_secs: u64) {
    fs::write(path, contents).unwrap();
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(modified_secs))
        .unwrap();
}

fn log_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn cursor(conn: &Connection, file: &Path) -> Option<(i64, i64)> {
    conn.query_row(
        "SELECT last_modified, last_line_offset FROM session_log_sync WHERE file_path = ?1",
        [file.to_string_lossy().as_ref()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .unwrap()
}

// The shared fixture serializes process-wide HOME/settings changes.
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn public_sync_retries_unchanged_claude_file_after_sqlite_failure() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    write_log(
        &file,
        &format!(
            "{}\n{}\n",
            usage_line("retry-a", 100),
            usage_line("retry-b", 200)
        ),
        100,
    );
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    // A regular trigger is intentional: the production importer has a separate
    // SQLite connection. Permit one insert before failing, regardless of order.
    conn.execute_batch(
        "CREATE TRIGGER temporary_usage_failure BEFORE INSERT ON proxy_request_logs
         WHEN (SELECT COUNT(*) FROM proxy_request_logs) > 0
         BEGIN SELECT RAISE(FAIL, 'temporary usage write failure'); END;",
    )
    .unwrap();
    let first = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    let failed_count = log_count(&conn);
    let failed_cursor = cursor(&conn, &file);
    conn.execute_batch("DROP TRIGGER temporary_usage_failure;")
        .unwrap();

    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), modified);
    assert_eq!(retry.imported, 2,
        "unchanged file must recover all usage after SQLite recovers; first={first:?}, retry={retry:?}, cursor={failed_cursor:?}, stored={failed_count}");
    assert_eq!(first.imported, 0);
    assert_eq!(first.errors.len(), 1);
    assert!(first.errors[0].contains("temporary usage write failure"));
    assert_eq!(failed_count, 0);
    assert_eq!(failed_cursor, None);
    assert!(retry.errors.is_empty());
    let repeat = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(repeat.imported, 0);
    assert!(repeat.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn incremental_failure_preserves_committed_rows_and_cursor() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    let committed = usage_line("committed", 50);
    write_log(&file, &format!("{committed}\n"), 100);
    let initial = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(initial.imported, 1);
    assert!(initial.errors.is_empty());
    let original_cursor = cursor(&conn, &file);
    write_log(
        &file,
        &format!("{committed}\n{}\n", usage_line("pending", 100)),
        101,
    );
    conn.execute_batch(
        "CREATE TRIGGER temporary_usage_failure BEFORE INSERT ON proxy_request_logs
         WHEN NEW.request_id = 'session:pending'
         BEGIN SELECT RAISE(FAIL, 'temporary incremental failure'); END;",
    )
    .unwrap();
    let failed = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(failed.errors.len(), 1);
    assert!(failed.errors[0].contains("temporary incremental failure"));
    assert_eq!(failed.imported, 0);
    assert_eq!(cursor(&conn, &file), original_cursor);
    assert_eq!(log_count(&conn), 1);
    conn.execute_batch("DROP TRIGGER temporary_usage_failure;")
        .unwrap();
    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(retry.imported, 1);
    assert!(retry.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
    assert_eq!(cursor(&conn, &file).unwrap().1, 2);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn failed_cursor_write_rolls_back_new_usage_and_can_retry() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    write_log(&file, &format!("{}\n", usage_line("checkpoint", 100)), 100);
    conn.execute_batch(
        "CREATE TRIGGER temporary_cursor_failure BEFORE INSERT ON session_log_sync
         BEGIN SELECT RAISE(FAIL, 'temporary cursor write failure'); END;",
    )
    .unwrap();
    let failed = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(failed.errors.len(), 1);
    assert!(failed.errors[0].contains("temporary cursor write failure"));
    assert_eq!(failed.imported, 0);
    assert_eq!(log_count(&conn), 0);
    assert_eq!(cursor(&conn, &file), None);
    conn.execute_batch("DROP TRIGGER temporary_cursor_failure;")
        .unwrap();
    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(retry.imported, 1);
    assert!(retry.errors.is_empty());
    let repeat = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(repeat.imported, 0);
    assert!(repeat.errors.is_empty());
    assert_eq!(log_count(&conn), 1);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn malformed_lines_and_duplicates_do_not_block_valid_usage() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    let valid = usage_line("valid", 100);
    let contents = format!("not json\n{{}}\n{{\"type\":\"assistant\"}}\n{valid}\n{valid}\n");
    write_log(&file, &contents, 100);
    let initial = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(initial.imported, 1);
    assert!(initial.errors.is_empty());
    assert_eq!(cursor(&conn, &file).unwrap().1, 5);
    assert_eq!(log_count(&conn), 1);
    conn.execute(
        "UPDATE proxy_request_logs SET provider_id = 'original-owner', total_cost_usd = '7.25'
         WHERE request_id = 'session:valid'",
        [],
    )
    .unwrap();
    write_log(&file, &format!("{contents}{valid}\n"), 101);
    let repeat = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!((repeat.imported, repeat.skipped), (0, 1));
    assert!(repeat.errors.is_empty());
    let stored: (String, String) = conn.query_row(
        "SELECT provider_id, total_cost_usd FROM proxy_request_logs WHERE request_id = 'session:valid'", [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(stored, ("original-owner".to_string(), "7.25".to_string()));
    assert_eq!(log_count(&conn), 1);
}
