use super::{storage, Database};
use crate::secrets::testing::TestHome;
use crate::secrets::{
    inventory,
    storage::{PlaintextIdentity, StorageContext},
    VaultContext,
};
use rusqlite::Connection;
pub(super) const VALUE: &str = "synthetic-generation-provider-secret";
pub(super) const DERIVED: &str = "synthetic-generation-derived-secret";

pub(super) fn plaintext_successor(current: &StorageContext) -> StorageContext {
    let identity = current.identity();
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: identity.store_id().into(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: identity.revision() + 1,
    })
}
pub(super) fn source(context: &StorageContext) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    Database::create_tables_on_conn(&conn).unwrap();
    Database::apply_upgrade_source_migrations_on_conn(&conn).unwrap();
    super::loongport_schema::apply(&conn).unwrap();
    storage::stamp(&conn, context).unwrap();
    let value = inventory::seal_db(
        context,
        "providers",
        "settings_config",
        &["generation-provider", "codex"],
        VALUE,
    )
    .unwrap();
    let meta = inventory::seal_db(
        context,
        "providers",
        "meta",
        &["generation-provider", "codex"],
        "{}",
    )
    .unwrap();
    let derived = inventory::seal_db(
        context,
        "providers",
        "qoder_http_secret",
        &["generation-provider", "codex"],
        DERIVED,
    )
    .unwrap();
    conn.execute("INSERT INTO providers(id,app_type,name,settings_config,meta,qoder_http_secret) VALUES('generation-provider','codex','Synthetic',?1,?2,?3)",rusqlite::params![value,meta,derived]).unwrap();
    conn.execute_batch("CREATE TABLE future_unknown_data(id INTEGER PRIMARY KEY,value BLOB,absent TEXT); INSERT INTO future_unknown_data VALUES(7,X'00FF12',NULL);").unwrap();
    conn
}
pub(super) fn assert_values(conn: &Connection, context: &StorageContext) {
    storage::check_identity(conn, context).unwrap();
    for (column, expected) in [
        ("settings_config", VALUE),
        ("meta", "{}"),
        ("qoder_http_secret", DERIVED),
    ] {
        let raw: String = conn
            .query_row(
                &format!("SELECT {column} FROM providers WHERE id='generation-provider'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            inventory::open_db(
                context,
                "providers",
                column,
                &["generation-provider", "codex"],
                &raw
            )
            .unwrap(),
            expected
        );
        if context.is_plaintext() {
            assert_eq!(raw, expected);
        } else {
            assert!(raw.starts_with("lpenc"));
        }
    }
    let row: (i64, Vec<u8>, Option<String>) = conn
        .query_row("SELECT id,value,absent FROM future_unknown_data", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(row, (7, vec![0, 255, 18], None));
}
pub(super) fn assert_unchanged(conn: &Connection, before: &str) {
    assert_eq!(Database::content_digest(conn).unwrap(), before);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn encrypted_to_plaintext_keeps_derived_and_unknown_rows_without_mutating_source() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = plaintext_successor(&current);
    let conn = source(&current);
    let before = Database::content_digest(&conn).unwrap();
    let output = storage::prepare_storage_generation(&conn, &current, &next).unwrap();
    assert_values(&output, &next);
    assert_unchanged(&conn, &before);
    assert_values(&conn, &current);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn plaintext_to_encrypted_keeps_derived_and_unknown_rows_without_mutating_source() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    });
    let next = StorageContext::from(
        VaultContext::create_generation_with_user_key(
            current.identity().store_id().to_owned(),
            2,
            zeroize::Zeroizing::new(vec![7; 32]),
        )
        .unwrap(),
    );
    let conn = source(&current);
    let before = Database::content_digest(&conn).unwrap();
    let output = storage::prepare_storage_generation(&conn, &current, &next).unwrap();
    assert_values(&output, &next);
    assert_unchanged(&conn, &before);
    assert_values(&conn, &current);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn explicit_plaintext_generation_change_keeps_values_and_rebinds_identity() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    });
    let next = plaintext_successor(&current);
    let conn = source(&current);
    let before = Database::content_digest(&conn).unwrap();
    let output = storage::prepare_storage_generation(&conn, &current, &next).unwrap();
    assert_values(&output, &next);
    assert_unchanged(&conn, &before);
    assert!(storage::check_identity(&output, &current).is_err());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_conversion_rejects_owner_revision_and_generation_changes_outside_contract() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let conn = source(&current);
    let before = Database::content_digest(&conn).unwrap();
    let StorageContext::Plaintext(valid) = plaintext_successor(&current) else {
        unreachable!()
    };
    let mut wrong_owner = valid.clone();
    wrong_owner.store_id = uuid::Uuid::new_v4().to_string();
    let mut repeated = valid.clone();
    repeated.generation_id = current.identity().generation_id().into();
    let mut skipped = valid.clone();
    skipped.revision += 1;
    let mut older = valid;
    older.revision = current.identity().revision();
    for invalid in [wrong_owner, repeated, skipped, older] {
        let result = storage::prepare_storage_generation(
            &conn,
            &current,
            &StorageContext::Plaintext(invalid),
        );
        assert!(
            matches!(result,Err(crate::error::AppError::Config(ref code)) if code=="protection.invalid_generation")
        );
        assert_unchanged(&conn, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_conversion_does_not_accept_corrupt_encrypted_values() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = plaintext_successor(&current);
    let conn = source(&current);
    conn.execute("UPDATE providers SET qoder_http_secret='lpenc:corrupt'", [])
        .unwrap();
    let before = Database::content_digest(&conn).unwrap();
    let result = storage::prepare_storage_generation(&conn, &current, &next);
    assert!(matches!(
        result,
        Err(crate::error::AppError::Config(ref code)) if code == "secret.unsupported_format"
    ));
    assert_unchanged(&conn, &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_conversion_preserves_unknown_rows_and_restores_audit_trigger() {
    use rusqlite::config::DbConfig;
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = plaintext_successor(&current);
    let conn = source(&current);
    conn.execute_batch("CREATE TRIGGER synthetic_conversion_audit AFTER UPDATE OF settings_config,meta,qoder_http_secret ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
    let trigger_sql: String = conn.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='synthetic_conversion_audit'", [], |r| r.get(0)).unwrap();
    let before = Database::content_digest(&conn).unwrap();
    assert!(conn
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap());
    let output = storage::prepare_storage_generation(&conn, &current, &next).unwrap();
    // The conversion may change only the inventory and mode identity, not
    // trigger-maintained unknown application data or the schema object itself.
    assert_values(&output, &next);
    let output_sql: String = output.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='synthetic_conversion_audit'", [], |r| r.get(0)).unwrap();
    assert_eq!(output_sql, trigger_sql);
    assert!(output
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap());
    assert!(conn
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap());
    assert_unchanged(&conn, &before);
    // Suppression is scoped to private transformation, not the ready connection.
    output
        .execute("UPDATE providers SET settings_config=settings_config", [])
        .unwrap();
    let audit: Vec<u8> = output
        .query_row(
            "SELECT value FROM future_unknown_data WHERE id=7",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audit, vec![1, 2, 3]);
    assert_unchanged(&conn, &before);
}
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_storage_generation_case(case: &str) {
    match case {
        "encrypted_to_plaintext_keeps_derived_and_unknown_rows_without_mutating_source" => {
            encrypted_to_plaintext_keeps_derived_and_unknown_rows_without_mutating_source()
        }
        "plaintext_to_encrypted_keeps_derived_and_unknown_rows_without_mutating_source" => {
            plaintext_to_encrypted_keeps_derived_and_unknown_rows_without_mutating_source()
        }
        "explicit_plaintext_generation_change_keeps_values_and_rebinds_identity" => {
            explicit_plaintext_generation_change_keeps_values_and_rebinds_identity()
        }
        "generation_conversion_rejects_owner_revision_and_generation_changes_outside_contract" => {
            generation_conversion_rejects_owner_revision_and_generation_changes_outside_contract()
        }
        "generation_conversion_does_not_accept_corrupt_encrypted_values" => {
            generation_conversion_does_not_accept_corrupt_encrypted_values()
        }
        "generation_conversion_preserves_unknown_rows_and_restores_audit_trigger" => {
            generation_conversion_preserves_unknown_rows_and_restores_audit_trigger()
        }
        _ => panic!("unknown storage generation fixture: {case}"),
    }
}
