//! Full original Database lifecycle regressions; not a standalone SQL fixture.
use super::loongport_schema;
use crate::database::{dao::relay_setup_operations as ledger, Database};

#[test]
fn new_database_installs_and_validates_the_setup_journal() {
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    assert_eq!(
        loongport_schema::read_stored_version(&conn).unwrap(),
        loongport_schema::LOONGPORT_SCHEMA_VERSION
    );
    ledger::validate_schema(&conn).unwrap();
    assert!(ledger::read_rows(&conn).unwrap().is_empty());
}

#[test]
fn prior_schema_migrates_without_changing_upstream_version() {
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    let upstream = Database::get_user_version(&conn).unwrap();
    conn.execute_batch("DROP TABLE relay_setup_operations; UPDATE loongport_schema_version SET version=31 WHERE id=1").unwrap();
    loongport_schema::apply(&conn).unwrap();
    ledger::validate_schema(&conn).unwrap();
    assert_eq!(Database::get_user_version(&conn).unwrap(), upstream);
    assert_eq!(
        loongport_schema::read_stored_version(&conn).unwrap(),
        loongport_schema::LOONGPORT_SCHEMA_VERSION
    );
    loongport_schema::apply(&conn).unwrap();
}

#[test]
fn installed_journal_loss_is_rejected_not_recreated() {
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("DROP TABLE relay_setup_operations")
        .unwrap();
    assert!(Database::create_tables_on_conn(&conn).is_err());
    assert!(loongport_schema::apply(&conn).is_err());
    assert!(super::vault::preflight_structure(&conn).is_err());
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='relay_setup_operations')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        !exists,
        "installed missing ledger is evidence loss, not a fresh database"
    );
}

#[test]
fn malformed_prior_journal_does_not_advance_schema_version() {
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("DROP TABLE relay_setup_operations; CREATE TABLE relay_setup_operations(operation_id TEXT); UPDATE loongport_schema_version SET version=31 WHERE id=1").unwrap();
    assert!(loongport_schema::apply(&conn).is_err());
    assert_eq!(loongport_schema::read_stored_version(&conn).unwrap(), 31);
    let columns: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('relay_setup_operations')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        columns, 1,
        "failed migration must not rewrite malformed prior evidence"
    );
}

#[test]
fn vendor_operation_schema_migrates_from_32_and_installed_loss_is_rejected() {
    let db = Database::memory().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute_batch("DROP TABLE vendor_connection_operations; UPDATE loongport_schema_version SET version=32 WHERE id=1").unwrap();
    loongport_schema::apply(&conn).unwrap();
    assert_eq!(loongport_schema::read_stored_version(&conn).unwrap(), 33);
    conn.execute_batch("DROP TABLE vendor_connection_operations")
        .unwrap();
    assert!(Database::create_tables_on_conn(&conn).is_err());
    assert!(super::vault::preflight_structure(&conn).is_err());
}
