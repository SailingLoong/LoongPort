//! Actual encrypted sync-join owner with synthetic, versioned private databases.
use super::*;
use crate::secrets::{session::SecretSession, testing::TestHome};

fn database(vault: &VaultContext, root: &Path, version: i32) -> Database {
    let conn = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    Database::apply_schema_migrations_on_conn(&conn).unwrap();
    crate::database::loongport_schema::apply(&conn).unwrap();
    if version == 20 {
        Database::apply_upstream4_migrations_on_conn(&conn).unwrap();
    }
    crate::database::vault::stamp(&conn, vault).unwrap();
    Database::from_connection(
        conn,
        SecretSession::from_context(root.to_path_buf(), vault.clone()),
    )
}

fn join_preserves_local_facts(version: i32) {
    let home = TestHome::new().unwrap();
    let current = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-current-password")
        .unwrap();
    let next = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-next-password")
        .unwrap();
    let local = database(&current, &home.path().join("current"), version);
    let incoming = database(&next, &home.path().join("incoming"), 17);
    {
        let conn = local.conn.lock().unwrap();
        conn.execute("INSERT INTO session_log_sync(file_path,last_modified,last_line_offset,last_synced_at) VALUES('synthetic-session',11,22,33)", []).unwrap();
        if version == 20 {
            conn.execute("UPDATE session_log_sync SET last_byte_offset=1234,last_tail_fingerprint='synthetic-tail'", []).unwrap();
        }
        conn.execute("INSERT INTO session_usage_dedup(data_source,request_id,semantic_id,has_entry_id) VALUES('synthetic','request-1','semantic-1',1)", []).unwrap();
        conn.execute("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,total_cost_usd,latency_ms,status_code,created_at) VALUES('request-1','synthetic-provider','claude','synthetic-model','0.125',5,200,44)", []).unwrap();
    }
    local.conn.lock().unwrap().execute("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,total_cost_usd) VALUES('2026-01-01','claude','synthetic-provider','synthetic-model',2,'0.250')", []).unwrap();
    crate::rt::block_on(local.save_live_backup("claude", "synthetic-local-secret-canary")).unwrap();
    incoming
        .set_setting("synthetic-synced-setting", "incoming-value")
        .unwrap();
    let (sql, metadata) = incoming.export_sync_snapshot().unwrap();
    let staged = Database::validate_sync_snapshot(&sql, &metadata, &next).unwrap();
    // The ordinary snapshot admission remains source17-only. Explicitly stage
    // the admitted private test copy through the real upgrade owner; do not
    // weaken normal startup/import version guards to exercise this join seam.
    if version == 20 {
        Database::apply_upstream4_migrations_on_conn(&staged.connection).unwrap();
    }
    let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    let joined = Database::prepare_sync_join(staged, &local.conn.lock().unwrap(), &current, &next)
        .unwrap_or_else(|error| panic!("source{version} encrypted sync join: {error}"));
    assert_eq!(
        Database::content_digest(&local.conn.lock().unwrap()).unwrap(),
        before
    );
    assert_eq!(Database::get_user_version(&joined).unwrap(), version);
    assert_eq!(
        crate::database::loongport_schema::read_stored_version(&joined).unwrap(),
        crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION
    );
    let cursor: (i64, i64, i64) = joined.query_row("SELECT last_modified,last_line_offset,last_synced_at FROM session_log_sync WHERE file_path='synthetic-session'", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(cursor, (11, 22, 33));
    if version == 20 {
        let tail: (i64,String) = joined.query_row("SELECT last_byte_offset,last_tail_fingerprint FROM session_log_sync WHERE file_path='synthetic-session'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(tail, (1234, "synthetic-tail".into()));
    }
    assert_eq!(joined.query_row("SELECT count(*) FROM session_usage_dedup WHERE request_id='request-1' AND semantic_id='semantic-1' AND has_entry_id=1", [], |row| row.get::<_,i64>(0)).unwrap(), 1);
    assert_eq!(
        joined
            .query_row(
                "SELECT total_cost_usd FROM proxy_request_logs WHERE request_id='request-1'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "0.125"
    );
    let rollup: (i64, String) = joined
        .query_row(
            "SELECT request_count,total_cost_usd FROM usage_daily_rollups WHERE date='2026-01-01'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rollup, (2, "0.250".into()));
    let raw: String = joined
        .query_row(
            "SELECT original_config FROM proxy_live_backup WHERE app_type='claude'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!raw.contains("synthetic-local-secret-canary"));
    let joined = Database::from_connection(
        joined,
        SecretSession::from_context(home.path().join("joined"), next),
    );
    assert_eq!(
        crate::rt::block_on(joined.get_live_backup("claude"))
            .unwrap()
            .unwrap()
            .original_config,
        "synthetic-local-secret-canary"
    );
    assert_eq!(
        joined
            .get_setting("synthetic-synced-setting")
            .unwrap()
            .as_deref(),
        Some("incoming-value")
    );
    println!("PASS source{version} encrypted sync join preserves cursor/dedup/cost and rewraps local backup");
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn source17_sync_join_preserves_local_facts() {
    join_preserves_local_facts(17);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn source20_sync_join_preserves_local_facts() {
    join_preserves_local_facts(20);
}

#[cfg(feature = "test-hooks")]
pub(super) fn verify() {
    sync_join_cannot_change_the_current_schema_version();
    sync_join_rejects_unknown_schema_and_normal_import_still_refuses20();
    source17_sync_join_preserves_local_facts();
    source20_sync_join_preserves_local_facts();
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn sync_join_cannot_change_the_current_schema_version() {
    for (current_version, incoming_version) in [(17, 20), (20, 17)] {
        let home = TestHome::new().unwrap();
        let vault = VaultContext::generate()
            .unwrap()
            .with_password("synthetic-schema-password")
            .unwrap();
        let local = database(&vault, &home.path().join("current"), current_version);
        let incoming = database(&vault, &home.path().join("incoming"), 17);
        let (sql, metadata) = incoming.export_sync_snapshot().unwrap();
        let staged = Database::validate_sync_snapshot(&sql, &metadata, &vault).unwrap();
        if incoming_version == 20 {
            Database::apply_upstream4_migrations_on_conn(&staged.connection).unwrap();
        }
        let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
        let result =
            Database::prepare_sync_join(staged, &local.conn.lock().unwrap(), &vault, &vault);
        assert!(
            result.is_err(),
            "sync join must not change schema {current_version} to {incoming_version}"
        );
        assert_eq!(
            Database::content_digest(&local.conn.lock().unwrap()).unwrap(),
            before
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn sync_join_rejects_unknown_schema_and_normal_import_still_refuses20() {
    for future in ["upstream", "loongport"] {
        let home = TestHome::new().unwrap();
        let vault = VaultContext::generate()
            .unwrap()
            .with_password("synthetic-future-password")
            .unwrap();
        let local = database(&vault, &home.path().join("current"), 17);
        let incoming = database(&vault, &home.path().join("incoming"), 17);
        let (sql, metadata) = incoming.export_sync_snapshot().unwrap();
        let staged = Database::validate_sync_snapshot(&sql, &metadata, &vault).unwrap();
        if future == "upstream" {
            local
                .conn
                .lock()
                .unwrap()
                .pragma_update(None, "user_version", 21)
                .unwrap();
            staged
                .connection
                .pragma_update(None, "user_version", 21)
                .unwrap();
        } else {
            local
                .conn
                .lock()
                .unwrap()
                .execute("UPDATE loongport_schema_version SET version=25", [])
                .unwrap();
            staged
                .connection
                .execute("UPDATE loongport_schema_version SET version=25", [])
                .unwrap();
        }
        let before = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
        assert!(
            Database::prepare_sync_join(staged, &local.conn.lock().unwrap(), &vault, &vault)
                .is_err()
        );
        assert_eq!(
            Database::content_digest(&local.conn.lock().unwrap()).unwrap(),
            before
        );
    }
    let home = TestHome::new().unwrap();
    let vault = VaultContext::generate()
        .unwrap()
        .with_password("synthetic-ordinary-password")
        .unwrap();
    let staged20 = database(&vault, &home.path().join("staged20"), 20);
    let (sql, metadata) = staged20.export_sync_snapshot().unwrap();
    assert!(
        matches!(Database::validate_sync_snapshot(&sql, &metadata, &vault), Err(AppError::Config(code)) if code == "secret.database_version_too_new")
    );
}
