use std::{fs, future::Future, io::Write, path::Path, path::PathBuf, time::Duration};

use cc_switch_lib::{sync_session_usage, AppState, APP_DIR_NAME, DB_FILE_NAME};
use rusqlite::{Connection, OptionalExtension};
use tauri::Manager;

#[path = "support.rs"]
mod support;

const THREAD_ID: &str = "00000000-0000-4000-8000-000000000003";

#[derive(Clone, Copy, Debug)]
enum Client {
    Claude,
    Codex,
}

impl Client {
    fn header(self) -> String {
        match self {
            Self::Claude => String::new(),
            Self::Codex => format!(
                "{}\n{}\n",
                serde_json::json!({"type":"session_meta", "timestamp":"2026-10-25T02:00:00Z", "payload":{"id":THREAD_ID,"source":"cli"}}),
                serde_json::json!({"type":"turn_context", "timestamp":"2026-10-25T02:00:01Z", "payload":{"model":"gpt-5.6-sol"}}),
            ),
        }
    }

    fn usage(self, index: u32) -> String {
        match self {
            Self::Claude => serde_json::json!({
                "type":"assistant", "timestamp":"2026-10-25T02:00:02Z", "sessionId":"checkpoint-boundary",
                "message":{"id":format!("checkpoint-{index}"), "model":"claude-sonnet-4-5", "stop_reason":"end_turn",
                    "usage":{"input_tokens":index,"output_tokens":1}}
            }),
            Self::Codex => {
                let input = index * (index + 1) / 2;
                serde_json::json!({
                    "type":"event_msg", "timestamp":"2026-10-25T02:00:02Z",
                    "payload":{"type":"token_count", "info":{"total_token_usage":{
                        "input_tokens":input,"cached_input_tokens":0,"output_tokens":index,"total_tokens":input+index
                    }}}
                })
            }
        }.to_string()
    }

    fn first_cursor(self) -> i64 {
        match self {
            Self::Claude => 1,
            Self::Codex => 3,
        }
    }
}

// MockRuntime's core plugins retain AppHandle cycles, so dropping a raw App
// does not release managed AppState/SQLite. Keep ownership scoped to this fixture.
struct FixtureApp(tauri::App<tauri::test::MockRuntime>);

impl FixtureApp {
    fn state(&self) -> tauri::State<'_, AppState> {
        self.0.state::<AppState>()
    }
}

impl Drop for FixtureApp {
    #[allow(deprecated)]
    fn drop(&mut self) {
        // SAFETY: this private wrapper exposes only a borrow tied to &self, never
        // an AppHandle. Every sync future is awaited before teardown; no IPC
        // handler is installed. Thus no State reference survives this removal.
        // Tauri's unmanage is deprecated because callers must ensure this lifetime.
        drop(self.0.unmanage::<AppState>());
    }
}

fn fixture(client: Client) -> (FixtureApp, Connection, PathBuf) {
    support::reset_test_fs();
    let home = support::ensure_test_home();
    std::env::set_var("OPENCODE_DB", home.join("absent-opencode.db"));
    let pi = home.join("empty-pi-sessions");
    fs::create_dir_all(&pi).unwrap();
    std::env::set_var("PI_CODING_AGENT_SESSION_DIR", pi);
    let state = support::create_test_state().unwrap();
    let conn = Connection::open(home.join(APP_DIR_NAME).join(DB_FILE_NAME)).unwrap();
    let file = match client {
        Client::Claude => home.join(".claude/projects/checkpoint/session.jsonl"),
        Client::Codex => home.join(format!(
            ".codex/sessions/2026/10/25/rollout-2026-10-25T02-00-00-{THREAD_ID}.jsonl"
        )),
    };
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    // Only the Tauri State container is mocked; sync, parsing and SQLite are real.
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    (FixtureApp(app), conn, file)
}

async fn within_deadline<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("public session sync must not hang")
}

fn set_mtime(file: &Path, nanos: u32) {
    let time = std::time::SystemTime::UNIX_EPOCH + Duration::new(100, nanos);
    fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(time)
        .unwrap();
    assert_eq!(fs::metadata(file).unwrap().modified().unwrap(), time);
}

fn append(file: &Path, contents: &str, nanos: u32) {
    let mut handle = fs::OpenOptions::new().append(true).open(file).unwrap();
    handle.write_all(contents.as_bytes()).unwrap();
    handle.flush().unwrap();
    drop(handle);
    set_mtime(file, nanos);
}

fn cursor(conn: &Connection, file: &Path) -> Option<(i64, i64)> {
    conn.query_row(
        "SELECT last_modified,last_line_offset FROM session_log_sync WHERE file_path=?1",
        [file.to_string_lossy().as_ref()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .unwrap()
}

fn totals(conn: &Connection) -> (i64, i64, i64) {
    conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0) FROM proxy_request_logs",
        [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap()
}

async fn append_checkpoint(client: Client, same_mtime: bool) {
    let (app, conn, file) = fixture(client);
    fs::write(&file, format!("{}{}\n", client.header(), client.usage(1))).unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, 1);
    assert!(first.errors.is_empty());
    assert_eq!(
        cursor(&conn, &file),
        Some((100_100_000_000, client.first_cursor()))
    );
    let old_len = fs::metadata(&file).unwrap().len();
    let next_nanos = if same_mtime { 100_000_000 } else { 900_000_000 };
    append(&file, &format!("{}\n", client.usage(2)), next_nanos);
    assert!(fs::metadata(&file).unwrap().len() > old_len);
    let second = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    let after_second = totals(&conn);
    let after_cursor = cursor(&conn, &file);
    let repeat = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    if same_mtime {
        // This is delayed visibility, not permanent loss: a later mtime change
        // still exposes the unconsumed appended record on the current code.
        set_mtime(&file, 900_000_000);
        let recovered = within_deadline(sync_session_usage(app.state()))
            .await
            .unwrap();
        assert_eq!(
            recovered.imported,
            u32::from(second.imported == 0),
            "mtime advance should recover the delayed row: {recovered:?}"
        );
        assert!(recovered.errors.is_empty());
        assert_eq!(totals(&conn), (2, 3, 2));
    }
    assert_eq!(second.imported,1,
        "{client:?} must see appended bytes; same_mtime={same_mtime}, second={second:?}, repeat={repeat:?}, totals={after_second:?}, cursor={after_cursor:?}");
    assert!(second.errors.is_empty());
    assert_eq!(after_second, (2, 3, 2));
    assert_eq!(
        after_cursor,
        Some((
            100_000_000_000 + next_nanos as i64,
            client.first_cursor() + 1
        ))
    );
    assert_eq!(repeat.imported, 0);
    assert!(repeat.errors.is_empty());
    assert_eq!(totals(&conn), (2, 3, 2));
}

async fn incomplete_tail_checkpoint(client: Client, has_previous_complete: bool) {
    let (app, conn, file) = fixture(client);
    let index = if has_previous_complete { 2 } else { 1 };
    let second_line = client.usage(index);
    let valid_prefix = if has_previous_complete {
        format!("{}\n", client.usage(1))
    } else {
        String::new()
    };
    let complete_offset = client.first_cursor() - i64::from(!has_previous_complete);
    let expected_totals = if has_previous_complete {
        (2, 3, 2)
    } else {
        (1, 1, 1)
    };
    // An actual interrupted append, without newline. Completion appends bytes
    // rather than rewriting the file. With no previous complete token, Codex's
    // no-billable early checkpoint path must also preserve the partial tail.
    let cut = second_line.len() / 2;
    assert!(serde_json::from_str::<serde_json::Value>(&second_line[..cut]).is_err());
    fs::write(
        &file,
        format!("{}{valid_prefix}{}", client.header(), &second_line[..cut]),
    )
    .unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, u32::from(has_previous_complete));
    assert!(first.errors.is_empty());
    let premature_cursor = cursor(&conn, &file);
    let paused = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(paused.imported, 0);
    assert!(paused.errors.is_empty());
    append(&file, &format!("{}\n", &second_line[cut..]), 900_000_000);
    let finished = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    let finished_totals = totals(&conn);
    let repeated = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(finished.imported, 1,
        "{client:?} must recover completed tail; has_previous_complete={has_previous_complete}, first={first:?}, premature_cursor={premature_cursor:?}, finished={finished:?}, totals={finished_totals:?}, repeated={repeated:?}");
    assert!(finished.errors.is_empty());
    assert_eq!(premature_cursor, Some((100_100_000_000, complete_offset)));
    assert_eq!(finished_totals, expected_totals);
    assert_eq!(cursor(&conn, &file).unwrap().1, complete_offset + 1);
    assert_eq!(repeated.imported, 0);
    assert!(repeated.errors.is_empty());
}

async fn valid_tail_without_newline(client: Client) {
    let (app, conn, file) = fixture(client);
    fs::write(&file, format!("{}{}", client.header(), client.usage(1))).unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, 1);
    assert!(first.errors.is_empty());
    assert_eq!(totals(&conn), (1, 1, 1));
    append(&file, "\n", 900_000_000);
    let repeat = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(repeat.imported, 0);
    assert!(repeat.errors.is_empty());
    assert_eq!(totals(&conn), (1, 1, 1));
    assert_eq!(cursor(&conn, &file).unwrap().1, client.first_cursor());
}

macro_rules! boundary_case {
    ($name:ident, $call:expr) => {
        #[allow(clippy::await_holding_lock)]
        #[tokio::test]
        async fn $name() {
            let _guard = support::test_mutex()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            $call.await;
        }
    };
}

boundary_case!(
    claude_same_second_different_nanos_append,
    append_checkpoint(Client::Claude, false)
);
boundary_case!(
    codex_same_second_different_nanos_append,
    append_checkpoint(Client::Codex, false)
);
boundary_case!(
    claude_equal_mtime_growing_file_append,
    append_checkpoint(Client::Claude, true)
);
boundary_case!(
    codex_equal_mtime_growing_file_append,
    append_checkpoint(Client::Codex, true)
);
boundary_case!(
    claude_incomplete_tail_is_retried_when_completed,
    incomplete_tail_checkpoint(Client::Claude, true)
);
boundary_case!(
    codex_incomplete_tail_is_retried_when_completed,
    incomplete_tail_checkpoint(Client::Codex, true)
);
boundary_case!(
    claude_complete_tail_without_newline_is_preserved,
    valid_tail_without_newline(Client::Claude)
);
boundary_case!(
    codex_complete_tail_without_newline_is_preserved,
    valid_tail_without_newline(Client::Codex)
);

boundary_case!(
    claude_incomplete_first_token_is_retried,
    incomplete_tail_checkpoint(Client::Claude, false)
);
boundary_case!(
    codex_incomplete_first_token_is_retried,
    incomplete_tail_checkpoint(Client::Codex, false)
);

fn enable_optional_byte_checkpoint(conn: &Connection) {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_log_sync') WHERE name='last_byte_offset')",
        [], |row| row.get(0),
    ).unwrap();
    assert!(
        !exists,
        "the default fixture must exercise the schema-17 fallback"
    );
    // Simulate an already upgraded SQLite database, not a production migration.
    conn.execute_batch("ALTER TABLE session_log_sync ADD COLUMN last_byte_offset INTEGER; ALTER TABLE session_log_sync ADD COLUMN last_tail_fingerprint INTEGER;")
        .unwrap();
}

fn byte_cursor(conn: &Connection, file: &Path) -> Option<i64> {
    conn.query_row(
        "SELECT last_byte_offset FROM session_log_sync WHERE file_path=?1",
        [file.to_string_lossy().as_ref()],
        |row| row.get(0),
    )
    .unwrap()
}

async fn optional_byte_checkpoint(client: Client) {
    let (app, conn, file) = fixture(client);
    enable_optional_byte_checkpoint(&conn);
    let prefix = format!("{}{}\n", client.header(), client.usage(1));
    let pending = client.usage(2);
    let split = pending.len() / 2;
    fs::write(&file, format!("{prefix}{}", &pending[..split])).unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, 1);
    assert!(first.errors.is_empty());
    assert_eq!(
        byte_cursor(&conn, &file),
        Some(prefix.len() as i64),
        "checkpoint must not consume an incomplete tail"
    );
    assert_eq!(cursor(&conn, &file).unwrap().1, client.first_cursor());
    conn.execute(
        "UPDATE session_log_sync SET last_tail_fingerprint='synthetic-tail' WHERE file_path=?1",
        [file.to_string_lossy().as_ref()],
    )
    .unwrap();
    append(&file, &format!("{}\n", &pending[split..]), 100_000_000);
    let complete = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(complete.imported, 1);
    assert!(complete.errors.is_empty());
    assert_eq!(totals(&conn), (2, 3, 2));
    let fingerprint: String = conn
        .query_row(
            "SELECT last_tail_fingerprint FROM session_log_sync WHERE file_path=?1",
            [file.to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        fingerprint, "synthetic-tail",
        "byte checkpoint must preserve unrelated optional metadata"
    );
    let committed_byte = byte_cursor(&conn, &file);
    let committed_cursor = cursor(&conn, &file);
    assert_eq!(
        committed_byte,
        Some(fs::metadata(&file).unwrap().len() as i64)
    );
    conn.execute_batch("CREATE TRIGGER reject_checkpoint_write BEFORE INSERT ON session_log_sync BEGIN SELECT RAISE(FAIL, 'temporary byte checkpoint write failure'); END;").unwrap();
    let unchanged = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(unchanged.imported, 0);
    assert!(
        unchanged.errors.is_empty(),
        "known unchanged byte checkpoint must skip unnecessary writes: {unchanged:?}"
    );
    append(&file, &format!("{}\n", client.usage(3)), 100_000_000);
    let failed = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(failed.errors.len(), 1);
    assert!(failed.errors[0].contains("temporary byte checkpoint write failure"));
    assert_eq!(totals(&conn), (2, 3, 2));
    assert_eq!(byte_cursor(&conn, &file), committed_byte);
    assert_eq!(cursor(&conn, &file), committed_cursor);
    conn.execute_batch("DROP TRIGGER reject_checkpoint_write;")
        .unwrap();
    let retried = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(retried.imported, 1);
    assert!(retried.errors.is_empty());
    assert_eq!(totals(&conn), (3, 6, 3));
    assert_eq!(
        byte_cursor(&conn, &file),
        Some(fs::metadata(&file).unwrap().len() as i64)
    );
}

async fn malformed_lines_and_split_utf8_tail(client: Client) {
    let (app, conn, file) = fixture(client);
    let prefix = format!("{}not json\n{{}}\n{}\n", client.header(), client.usage(1));
    let mut pending: serde_json::Value = serde_json::from_str(&client.usage(2)).unwrap();
    pending["note"] = serde_json::json!("中文");
    let pending = pending.to_string();
    let split = pending.find('中').unwrap() + 1;
    assert!(std::str::from_utf8(&pending.as_bytes()[..split]).is_err());
    let mut initial = prefix.as_bytes().to_vec();
    initial.extend_from_slice(&pending.as_bytes()[..split]);
    fs::write(&file, &initial).unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, 1);
    assert!(first.errors.is_empty());
    let mut tail = fs::OpenOptions::new().append(true).open(&file).unwrap();
    tail.write_all(&pending.as_bytes()[split..]).unwrap();
    tail.write_all(b"\n").unwrap();
    drop(tail);
    set_mtime(&file, 900_000_000);
    let finished = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(
        finished.imported, 1,
        "completed UTF-8 tail must not have been consumed: {finished:?}"
    );
    assert!(finished.errors.is_empty());
    assert_eq!(totals(&conn), (2, 3, 2));
    assert_eq!(cursor(&conn, &file).unwrap().1, client.first_cursor() + 3);
}

async fn unknown_optional_byte_checkpoint(client: Client) {
    let (app, conn, file) = fixture(client);
    enable_optional_byte_checkpoint(&conn);
    fs::write(&file, format!("{}{}\n", client.header(), client.usage(1))).unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, 1);
    for unknown in [
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Text("legacy-unknown".to_owned()),
    ] {
        conn.execute(
            "UPDATE session_log_sync SET last_byte_offset=?1 WHERE file_path=?2",
            rusqlite::params![unknown, file.to_string_lossy().as_ref()],
        )
        .unwrap();
        let retried = within_deadline(sync_session_usage(app.state()))
            .await
            .unwrap();
        assert_eq!(retried.imported, 0);
        assert!(retried.errors.is_empty());
        assert_eq!(totals(&conn), (1, 1, 1));
        assert_eq!(
            byte_cursor(&conn, &file),
            Some(fs::metadata(&file).unwrap().len() as i64),
            "unknown byte cursor must not authorize the unchanged fast path"
        );
    }
}

boundary_case!(
    claude_optional_byte_checkpoint_is_atomic,
    optional_byte_checkpoint(Client::Claude)
);
boundary_case!(
    codex_optional_byte_checkpoint_is_atomic,
    optional_byte_checkpoint(Client::Codex)
);
boundary_case!(
    claude_complete_bad_lines_and_split_utf8_tail,
    malformed_lines_and_split_utf8_tail(Client::Claude)
);
boundary_case!(
    codex_complete_bad_lines_and_split_utf8_tail,
    malformed_lines_and_split_utf8_tail(Client::Codex)
);
boundary_case!(
    claude_unknown_byte_checkpoint_is_not_trusted,
    unknown_optional_byte_checkpoint(Client::Claude)
);
boundary_case!(
    codex_unknown_byte_checkpoint_is_not_trusted,
    unknown_optional_byte_checkpoint(Client::Codex)
);

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn codex_archived_growth_inherits_only_consumed_bytes() {
    let _guard = support::test_mutex()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (app, conn, file) = fixture(Client::Codex);
    enable_optional_byte_checkpoint(&conn);
    fs::write(
        &file,
        format!("{}{}\n", Client::Codex.header(), Client::Codex.usage(1)),
    )
    .unwrap();
    set_mtime(&file, 100_000_000);
    let first = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(first.imported, 1);
    let archived_dir = support::ensure_test_home().join(".codex/archived_sessions");
    fs::create_dir_all(&archived_dir).unwrap();
    let archived = archived_dir.join(file.file_name().unwrap());
    fs::rename(&file, &archived).unwrap();
    append(
        &archived,
        &format!("{}\n", Client::Codex.usage(2)),
        100_000_000,
    );
    let second = within_deadline(sync_session_usage(app.state()))
        .await
        .unwrap();
    assert_eq!(
        second.imported, 1,
        "archived append needs inherited boundary, not just its equal mtime: {second:?}"
    );
    assert!(second.errors.is_empty());
    assert_eq!(totals(&conn), (2, 3, 2));
    assert_eq!(
        byte_cursor(&conn, &archived),
        Some(fs::metadata(&archived).unwrap().len() as i64)
    );
}

async fn truncation_keeps_existing_checkpoint(client: Client) {
    for optional_columns in [false, true] {
        let (app, conn, file) = fixture(client);
        if optional_columns {
            enable_optional_byte_checkpoint(&conn);
        }
        fs::write(
            &file,
            format!(
                "{}{}\n{}\n",
                client.header(),
                client.usage(1),
                client.usage(2)
            ),
        )
        .unwrap();
        set_mtime(&file, 100_000_000);
        let first = within_deadline(sync_session_usage(app.state()))
            .await
            .unwrap();
        assert_eq!(
            first.imported, 2,
            "{client:?}, optional_columns={optional_columns}: {first:?}"
        );
        assert!(first.errors.is_empty());
        let original = cursor(&conn, &file);
        let original_byte = if optional_columns {
            byte_cursor(&conn, &file)
        } else {
            None
        };
        fs::write(&file, format!("{}{}\n", client.header(), client.usage(1))).unwrap();
        set_mtime(&file, 900_000_000);
        let shortened = within_deadline(sync_session_usage(app.state()))
            .await
            .unwrap();
        assert_eq!(
            shortened.errors.len(),
            1,
            "truncation must not silently reset a committed checkpoint: {shortened:?}"
        );
        assert_eq!(shortened.imported, 0);
        assert_eq!(cursor(&conn, &file), original);
        if optional_columns {
            assert_eq!(byte_cursor(&conn, &file), original_byte);
        }
        assert_eq!(totals(&conn), (2, 3, 2));
    }
}
boundary_case!(
    claude_truncation_does_not_reset_checkpoint,
    truncation_keeps_existing_checkpoint(Client::Claude)
);
boundary_case!(
    codex_truncation_does_not_reset_checkpoint,
    truncation_keeps_existing_checkpoint(Client::Codex)
);

#[tokio::test]
async fn fixture_releases_database_before_next_schema() {
    let _guard = support::test_mutex()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for client in [Client::Claude, Client::Codex] {
        let (app, conn, _file) = fixture(client);
        let db = std::sync::Arc::downgrade(&app.state().db);
        drop(conn);
        drop(app);
        assert!(
            db.upgrade().is_none(),
            "{client:?} fixture must release SQLite before the next schema resets its files"
        );
    }
}
