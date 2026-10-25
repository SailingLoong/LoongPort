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
    let project = home.join(".codex/sessions/2026/10/25");
    fs::create_dir_all(&project).unwrap();
    // MockRuntime supplies only the State container. The public command,
    // importer and on-disk SQLite database are the actual production code.
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (
        app,
        conn,
        project.join(format!("rollout-2026-10-25T02-00-00-{THREAD_ID}.jsonl")),
    )
}

async fn within_deadline<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("public session sync must not deadlock")
}

const THREAD_ID: &str = "00000000-0000-4000-8000-000000000001";

fn request_id(index: usize) -> String {
    format!("codex_session:thread-v1:{THREAD_ID}:{index}")
}

fn rollout(count: usize) -> String {
    let mut lines = vec![
        serde_json::json!({
            "type": "session_meta", "timestamp": "2026-10-25T02:00:00Z",
            "payload": { "id": THREAD_ID, "source": "cli" }
        })
        .to_string(),
        serde_json::json!({
            "type": "turn_context", "timestamp": "2026-10-25T02:00:01Z",
            "payload": { "model": "gpt-5.6-sol" }
        })
        .to_string(),
    ];
    for index in 1..=count {
        let input = index * (index + 1) / 2;
        lines.push(
            serde_json::json!({
                "type": "event_msg", "timestamp": "2026-10-25T02:00:02Z",
                "payload": { "type": "token_count", "info": { "total_token_usage": {
                    "input_tokens": input, "cached_input_tokens": 0,
                    "output_tokens": index, "reasoning_output_tokens": 0,
                    "total_tokens": input + index
                }}}
            })
            .to_string(),
        );
    }
    lines.join("\n") + "\n"
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

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn unchanged_rollout_recovers_after_in_batch_write_failure() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    write_log(&file, &rollout(2), 100);
    conn.execute_batch(
        "CREATE TRIGGER temporary_codex_failure BEFORE INSERT ON proxy_request_logs
         WHEN NEW.data_source = 'codex_session' AND NEW.input_tokens = 2
         BEGIN SELECT RAISE(FAIL, 'temporary Codex write failure'); END;",
    )
    .unwrap();
    let first = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    let failed_rows = log_count(&conn);
    let failed_cursor = cursor(&conn, &file);
    conn.execute_batch("DROP TRIGGER temporary_codex_failure;")
        .unwrap();
    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(retry.imported, 2,
        "unchanged rollout must retry failed writes; first={first:?}, retry={retry:?}, rows={failed_rows}, cursor={failed_cursor:?}");
    assert_eq!(first.errors.len(), 1);
    assert!(first.errors[0].contains("temporary Codex write failure"));
    assert_eq!(failed_rows, 0);
    assert_eq!(failed_cursor, None);
    assert!(retry.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
    let repeat = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(repeat.imported, 0);
    assert!(repeat.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn later_batch_failure_preserves_committed_batch_and_recovers_tail() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    // Exceed the production 1000-row transaction boundary. The second batch
    // writes one row before its next row fails; the first batch must survive.
    write_log(&file, &rollout(1002), 100);
    conn.execute_batch(
        "CREATE TRIGGER temporary_codex_failure BEFORE INSERT ON proxy_request_logs
         WHEN NEW.data_source = 'codex_session' AND NEW.input_tokens = 1002
         BEGIN SELECT RAISE(FAIL, 'temporary later batch failure'); END;",
    )
    .unwrap();
    let failed = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    let failed_rows = log_count(&conn);
    let failed_cursor = cursor(&conn, &file);
    conn.execute(
        "UPDATE proxy_request_logs SET provider_id = 'original-owner', total_cost_usd = '7.25'
         WHERE request_id = ?1",
        [request_id(1)],
    )
    .unwrap();
    conn.execute_batch("DROP TRIGGER temporary_codex_failure;")
        .unwrap();
    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!((retry.imported, retry.skipped), (2, 1000),
        "unchanged rollout must recover the failed batch; first={failed:?}, retry={retry:?}, rows={failed_rows}, cursor={failed_cursor:?}");
    assert_eq!(failed.errors.len(), 1);
    assert!(failed.errors[0].contains("temporary later batch failure"));
    assert_eq!(failed_rows, 1000);
    assert_eq!(failed_cursor, None);
    assert!(retry.errors.is_empty());
    let totals: (i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), SUM(input_tokens), SUM(output_tokens) FROM proxy_request_logs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(totals, (1002, 502503, 1002));
    let owner: (String, String) = conn
        .query_row(
            "SELECT provider_id, total_cost_usd FROM proxy_request_logs WHERE request_id = ?1",
            [request_id(1)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner, ("original-owner".to_string(), "7.25".to_string()));
    assert_eq!(cursor(&conn, &file).unwrap().1, 1004);
    let repeat = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(repeat.imported, 0);
    assert!(repeat.errors.is_empty());
    assert_eq!(log_count(&conn), 1002);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn incremental_failure_keeps_previous_cursor_and_usage() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    write_log(&file, &rollout(1), 100);
    let initial = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(initial.imported, 1);
    assert!(initial.errors.is_empty());
    let original_cursor = cursor(&conn, &file);
    write_log(&file, &rollout(2), 101);
    conn.execute_batch(
        "CREATE TRIGGER temporary_codex_failure BEFORE INSERT ON proxy_request_logs
         WHEN NEW.data_source = 'codex_session' AND NEW.input_tokens = 2
         BEGIN SELECT RAISE(FAIL, 'temporary incremental Codex failure'); END;",
    )
    .unwrap();
    let failed = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    let failed_cursor = cursor(&conn, &file);
    let failed_rows = log_count(&conn);
    conn.execute_batch("DROP TRIGGER temporary_codex_failure;")
        .unwrap();
    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(retry.imported, 1,
        "unchanged incremental rollout must recover; first={failed:?}, retry={retry:?}, cursor={failed_cursor:?}");
    assert_eq!(failed.errors.len(), 1);
    assert!(failed.errors[0].contains("temporary incremental Codex failure"));
    assert_eq!(failed_cursor, original_cursor);
    assert_eq!(failed_rows, 1);
    assert!(retry.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
    assert_eq!(cursor(&conn, &file).unwrap().1, 4);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn malformed_lines_and_replayed_snapshots_preserve_usage_owner() {
    let _guard = test_guard();
    let (app, conn, file) = fixture();
    let valid = rollout(2);
    let repeated = valid.lines().last().unwrap();
    let contents = format!("{valid}not json\n{repeated}\n");
    write_log(&file, &contents, 100);
    let first = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(first.imported, 2);
    assert!(first.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
    assert_eq!(cursor(&conn, &file).unwrap().1, 6);
    conn.execute(
        "UPDATE proxy_request_logs SET provider_id = 'original-owner', total_cost_usd = '7.25'
         WHERE request_id = ?1",
        [request_id(1)],
    )
    .unwrap();
    write_log(&file, &format!("{contents}{repeated}\n"), 101);
    let retry = within_deadline(sync_session_usage(app.state::<AppState>()))
        .await
        .unwrap();
    assert_eq!(retry.imported, 0);
    assert!(retry.errors.is_empty());
    assert_eq!(log_count(&conn), 2);
    assert_eq!(cursor(&conn, &file).unwrap().1, 7);
    let owner: (String, String) = conn
        .query_row(
            "SELECT provider_id, total_cost_usd FROM proxy_request_logs WHERE request_id = ?1",
            [request_id(1)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner, ("original-owner".to_string(), "7.25".to_string()));
}
