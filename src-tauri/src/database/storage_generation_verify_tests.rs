//! Actual same-pending image comparison, including unknown schema and row data.
use crate::{
    database::{storage, storage_generation_tests as fixture},
    error::AppError,
    secrets::{inventory, storage::StorageContext, testing::TestHome, VaultContext},
};
use rusqlite::{config::DbConfig, Connection};
fn encrypted_successor(current: &StorageContext) -> StorageContext {
    VaultContext::create_generation_with_user_text(
        current.identity().store_id().into(),
        current.identity().revision() + 1,
        "synthetic verify exact text",
    )
    .unwrap()
    .into()
}
fn preserved(
    source: &Connection,
    current: &StorageContext,
    target: &Connection,
    next: &StorageContext,
) -> Result<(), AppError> {
    let source_bytes = source.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let target_bytes = target.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let source_flag = source
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap();
    let target_flag = target
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap();
    let result = storage::verify_storage_generation_image(source, current, target, next);
    assert!(
        &*source.serialize(rusqlite::MAIN_DB).unwrap() == source_bytes.as_slice(),
        "comparison wrote source pages"
    );
    assert!(
        &*target.serialize(rusqlite::MAIN_DB).unwrap() == target_bytes.as_slice(),
        "comparison wrote target pages"
    );
    assert_eq!(
        source
            .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
            .unwrap(),
        source_flag
    );
    assert_eq!(
        target
            .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
            .unwrap(),
        target_flag
    );
    result
}
fn code(result: Result<(), AppError>, expected: &str) {
    let error = result
        .err()
        .expect("different generation images compared equal");
    assert!(
        matches!(error,AppError::Config(ref actual) if actual==expected),
        "unexpected comparison error: {error}"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_image_equal_values_ignore_nonce_and_mode_identity() {
    let _home = TestHome::new().unwrap();
    let encrypted = StorageContext::from(VaultContext::generate().unwrap());
    let plain = fixture::plaintext_successor(&encrypted);
    for (current, next) in [
        (encrypted.clone(), fixture::plaintext_successor(&encrypted)),
        (encrypted.clone(), encrypted_successor(&encrypted)),
        (plain.clone(), encrypted_successor(&plain)),
        (plain.clone(), fixture::plaintext_successor(&plain)),
    ] {
        let source = fixture::source(&current);
        let target = storage::prepare_storage_generation(&source, &current, &next).unwrap();
        preserved(&source, &current, &target, &next).unwrap();
        if !next.is_plaintext() {
            let independent =
                storage::prepare_storage_generation(&source, &current, &next).unwrap();
            let column = |conn: &Connection| {
                conn.query_row(
                    "SELECT qoder_http_secret FROM providers WHERE id='generation-provider'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .unwrap()
            };
            assert_ne!(
                column(&target),
                column(&independent),
                "nonce fixture must really differ"
            );
            preserved(&source, &current, &independent, &next).unwrap();
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_image_rejects_secret_unknown_column_blob_and_schema_changes() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = encrypted_successor(&current);
    let source = fixture::source(&current);
    for change in ["ordinary", "blob", "null", "schema", "index", "derived"] {
        let target = storage::prepare_storage_generation(&source, &current, &next).unwrap();
        match change {
            "ordinary" => {
                target
                    .execute("UPDATE providers SET name='Changed synthetic name'", [])
                    .unwrap();
            }
            "blob" => {
                target
                    .execute("UPDATE future_unknown_data SET value=X'00FF13'", [])
                    .unwrap();
            }
            "null" => {
                target
                    .execute("UPDATE future_unknown_data SET absent=''", [])
                    .unwrap();
            }
            "schema" => target
                .execute_batch("CREATE TABLE future_extra(id INTEGER PRIMARY KEY,value BLOB);")
                .unwrap(),
            "index" => target
                .execute_batch("CREATE INDEX future_extra_index ON future_unknown_data(value);")
                .unwrap(),
            "derived" => {
                let raw = inventory::seal_db(
                    &next,
                    "providers",
                    "qoder_http_secret",
                    &["generation-provider", "codex"],
                    "changed authenticated derived credential",
                )
                .unwrap();
                target
                    .execute("UPDATE providers SET qoder_http_secret=?1", [raw])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        code(
            preserved(&source, &current, &target, &next),
            "protection.transformation_mismatch",
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_image_authenticates_both_sides_and_rejects_identity_extensions() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = encrypted_successor(&current);
    let source = fixture::source(&current);
    let target = storage::prepare_storage_generation(&source, &current, &next).unwrap();
    let wrong = StorageContext::from(VaultContext::generate().unwrap());
    assert!(preserved(&source, &wrong, &target, &next).is_err());
    assert!(preserved(&source, &current, &target, &wrong).is_err());
    target
        .execute("UPDATE providers SET qoder_http_secret='lpenc:corrupt'", [])
        .unwrap();
    code(
        preserved(&source, &current, &target, &next),
        "secret.unsupported_format",
    );
    for ddl in [
        "ALTER TABLE loongport_vault ADD COLUMN unknown BLOB DEFAULT X'AA';",
        "CREATE INDEX unknown_identity_index ON loongport_vault(metadata);",
    ] {
        let source = fixture::source(&current);
        let target = storage::prepare_storage_generation(&source, &current, &next).unwrap();
        source.execute_batch(ddl).unwrap();
        target.execute_batch(ddl).unwrap();
        code(
            preserved(&source, &current, &target, &next),
            "protection.database_identity_invalid",
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn generation_image_private_normalization_does_not_fire_unknown_triggers() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = encrypted_successor(&current);
    let source = fixture::source(&current);
    source.execute_batch("CREATE TRIGGER normalize_audit AFTER UPDATE OF settings_config,meta,qoder_http_secret ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
    let target = storage::prepare_storage_generation(&source, &current, &next).unwrap();
    source
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
        .unwrap();
    preserved(&source, &current, &target, &next).unwrap();
    target
        .execute("UPDATE future_unknown_data SET value=X'090909'", [])
        .unwrap();
    // If normalization fires both identical audit triggers, they erase this
    // real difference. Rejecting it proves normalization did not mask a row.
    code(
        preserved(&source, &current, &target, &next),
        "protection.transformation_mismatch",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_generation_image_verifier_keeps_primary_exact() {
    let _home = TestHome::new().unwrap();
    let old = VaultContext::generate().unwrap();
    let current = StorageContext::from(
        old.with_password("synthetic verified backup wrapper")
            .unwrap(),
    );
    let source_context = StorageContext::from(old);
    let next = encrypted_successor(&current);
    let source = fixture::source(&source_context);
    let target =
        storage::prepare_storage_backup_generation(&source, &source_context, &current, &next)
            .unwrap();
    let before_source = source.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    let before_target = target.serialize(rusqlite::MAIN_DB).unwrap().to_vec();
    storage::verify_storage_backup_generation_image(&source, &current, &target, &next).unwrap();
    code(
        preserved(&source, &current, &target, &next),
        "secret.identity_mismatch",
    );
    assert!(&*source.serialize(rusqlite::MAIN_DB).unwrap() == before_source.as_slice());
    assert!(&*target.serialize(rusqlite::MAIN_DB).unwrap() == before_target.as_slice());
    target
        .execute("UPDATE future_unknown_data SET value=X'AA'", [])
        .unwrap();
    code(
        storage::verify_storage_backup_generation_image(&source, &current, &target, &next),
        "protection.transformation_mismatch",
    );
    let foreign = fixture::source(&StorageContext::from(VaultContext::generate().unwrap()));
    code(
        storage::verify_storage_backup_generation_image(&foreign, &current, &target, &next),
        "protection.backup_identity_mismatch",
    );
}

pub(super) fn verify(case: &str) {
    match case {
        "backup_generation_image_verifier_keeps_primary_exact" => {
            backup_generation_image_verifier_keeps_primary_exact()
        }
        "generation_image_equal_values_ignore_nonce_and_mode_identity" => {
            generation_image_equal_values_ignore_nonce_and_mode_identity()
        }
        "generation_image_rejects_secret_unknown_column_blob_and_schema_changes" => {
            generation_image_rejects_secret_unknown_column_blob_and_schema_changes()
        }
        "generation_image_authenticates_both_sides_and_rejects_identity_extensions" => {
            generation_image_authenticates_both_sides_and_rejects_identity_extensions()
        }
        "generation_image_private_normalization_does_not_fire_unknown_triggers" => {
            generation_image_private_normalization_does_not_fire_unknown_triggers()
        }
        _ => panic!("unknown generation image comparison case"),
    }
}
