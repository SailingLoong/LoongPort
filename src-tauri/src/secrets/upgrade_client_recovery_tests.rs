//! Client admission must not rewrite an authenticated pre-client checkpoint.
use super::attached_recovery_tests::assert_no_recovery_writes;
use super::*;
use crate::proxy::client_access::managed;

fn source_version(f: &Fixture, version: i32) {
    if version == 24 {
        return;
    }
    let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
    crate::paperclip::bindings::ensure_schema(&conn).unwrap();
    crate::database::create_qoder_http_auth_column(&conn).unwrap();
    conn.execute_batch(include_str!(
        "test_fixtures/qoder_campaign_operations_v27.sql"
    ))
    .unwrap();
    if version == 28 {
        conn.execute_batch(include_str!("test_fixtures/client_credentials_v28.sql"))
            .unwrap();
    }
    conn.execute(
        "UPDATE loongport_schema_version SET version=?1 WHERE id=1",
        [version],
    )
    .unwrap();
}

#[test]
#[serial_test::serial]
fn retained_pre_client_managed_projection_is_authenticated_and_read_only() {
    for version in [24, 27] {
        let f = Fixture::new();
        source_version(&f, version);
        let (_, _, _, runtime) = super::native_recovery_tests::native_runtime_at_version(
            &f,
            &crate::app_config::AppType::Claude,
            version,
        );
        let before = snapshot(f.home.path());
        for app in managed::CLI_APPS {
            runtime.db.provision_managed_client_credential(app).unwrap();
            assert_eq!(managed::key(&runtime.db, app).unwrap(), managed::LEGACY_KEY);
            assert!(!runtime
                .db
                .owns_managed_client_key(app, "synthetic-foreign")
                .unwrap());
        }
        assert!(runtime
            .db
            .provision_managed_client_credential("unknown")
            .is_err());
        let reactor = tokio::runtime::Runtime::new().unwrap();
        let error = reactor
            .block_on(runtime.proxy_service.complete_client_access_migration())
            .unwrap_err();
        assert!(error.contains("upgrade.checkpoint_pending"), "{error}");
        assert_no_recovery_writes(
            &before,
            &snapshot(f.home.path()),
            &format!("v{version} must not issue grants, create a table or publish readiness"),
        );
    }
}

#[test]
#[serial_test::serial]
fn legacy_client_projection_rejects_unverified_or_inconsistent_retained_state() {
    for case in [
        "missing",
        "corrupt",
        "ready",
        "unknown_marker",
        "table",
        "blocked",
        "source",
    ] {
        let f = Fixture::new();
        publish_resume_fixture(&f);
        let path = f.device.root().join(checkpoint::FILE);
        match case {
            "missing" => std::fs::remove_file(&path).unwrap(),
            "corrupt" => std::fs::write(&path, b"synthetic-bad-checkpoint").unwrap(),
            _ => {}
        }
        let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        match case {
            "ready" | "unknown_marker" => {
                conn.execute(
                    "INSERT INTO settings(key,value) VALUES('client_access_migration_version',?1)",
                    [if case == "ready" { "1" } else { "future" }],
                )
                .unwrap();
            }
            "table" => conn
                .execute_batch("CREATE TABLE loongport_client_credentials(id TEXT)")
                .unwrap(),
            "source" => conn.pragma_update(None, "user_version", 17).unwrap(),
            _ => {}
        }
        let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
        let db = Database::from_connection(conn, session.clone());
        if case == "blocked" {
            session.set_blocked(true);
        }
        let before = snapshot(f.home.path());
        assert!(
            db.provision_managed_client_credential("claude").is_err(),
            "{case}"
        );
        assert!(managed::key(&db, "claude").is_err(), "{case}");
        assert!(
            db.owns_managed_client_key("claude", "synthetic-foreign")
                .is_err(),
            "{case}"
        );
        assert_no_recovery_writes(&before, &snapshot(f.home.path()), case);
    }
}

#[test]
#[serial_test::serial]
fn legacy_client_projection_rechecks_epoch_after_final_sql_reads() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let f = Fixture::new();
    publish_resume_fixture(&f);
    let session = session::SecretSession::from_context(f.root.clone(), f.vault.clone());
    let db = Database::from_connection(
        rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
        session.clone(),
    );
    let rotated = Arc::new(AtomicBool::new(false));
    let hook = rotated.clone();
    db.conn
        .lock()
        .unwrap()
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "settings",
                    ..
                }
            ) && !hook.swap(true, Ordering::SeqCst)
            {
                session.set_blocked(true);
                session.set_blocked(false);
            }
            Authorization::Allow
        }))
        .unwrap();
    assert!(db.retained_legacy_client_projection().is_err());
    assert!(
        rotated.load(Ordering::SeqCst),
        "rotation must occur inside the final recheck"
    );
}

#[test]
#[serial_test::serial]
fn current_client_schema_never_falls_back_when_missing_or_revoked() {
    let current_schema = database::loongport_schema::LOONGPORT_SCHEMA_VERSION;
    for version in [28, current_schema] {
        let f = Fixture::new();
        source_version(&f, 28);
        publish_resume_fixture(&f);
        let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        if version == current_schema {
            database::loongport_schema::apply(&conn).unwrap();
        }
        assert_eq!(
            database::loongport_schema::read_stored_version(&conn).unwrap(),
            version,
            "the missing-table negative must exercise the declared schema"
        );
        conn.execute_batch("DROP TABLE loongport_client_credentials")
            .unwrap();
        let db = Database::from_connection(
            conn,
            session::SecretSession::from_context(f.root.clone(), f.vault.clone()),
        );
        let before = snapshot(f.home.path());
        assert!(!db.retained_legacy_client_projection().unwrap());
        assert!(db.provision_managed_client_credential("claude").is_err());
        assert!(managed::key(&db, "claude").is_err());
        assert_no_recovery_writes(&before, &snapshot(f.home.path()), "current missing table");
    }
    let db = Database::memory().unwrap();
    db.provision_managed_client_credential("claude").unwrap();
    db.revoke_client_credential("builtin:managed-claude:claude")
        .unwrap();
    assert!(db.provision_managed_client_credential("claude").is_err());
    assert!(managed::key(&db, "claude").is_err());
}

#[test]
#[serial_test::serial]
fn native_runtime_preserves_explicit_historical_versions() {
    for version in [24, 27, 28] {
        let f = Fixture::new();
        source_version(&f, version);
        let conn = rusqlite::Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&conn).unwrap(),
            version
        );
        drop(conn);
        let (_, _, _, runtime) = super::native_recovery_tests::native_runtime_at_version(
            &f,
            &crate::app_config::AppType::Claude,
            version,
        );
        let conn = runtime.db.conn.lock().unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&conn).unwrap(),
            version
        );
        assert_eq!(
            Database::table_exists(&conn, "loongport_client_credentials").unwrap(),
            version >= 28
        );
        assert!(!conn
            .prepare("PRAGMA table_xinfo(providers)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .any(|name| name.unwrap().eq_ignore_ascii_case("credential_revision")));
    }
}
