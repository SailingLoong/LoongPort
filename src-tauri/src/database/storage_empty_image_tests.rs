use super::*;
use crate::secrets::{testing::TestHome, VaultContext};
use rusqlite::config::DbConfig;

fn contexts() -> [StorageContext; 2] {
    [
        StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: uuid::Uuid::new_v4().to_string(),
            generation_id: uuid::Uuid::new_v4().to_string(),
            revision: 1,
        }),
        StorageContext::from(VaultContext::generate().unwrap()),
    ]
}
fn bytes(image: &Connection) -> Vec<u8> {
    image.serialize(rusqlite::MAIN_DB).unwrap().to_vec()
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn empty_image_accepts_actual_default_clock_difference_without_source_writes() {
    let _home = TestHome::new().unwrap();
    for context in contexts() {
        let image = empty_storage_image(&context).unwrap();
        let created: String = image
            .query_row(
                "SELECT created_at FROM proxy_config WHERE app_type='claude'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let later = empty_storage_image(&context).unwrap();
        let other: String = later
            .query_row(
                "SELECT created_at FROM proxy_config WHERE app_type='claude'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(
            created, other,
            "fixture must cross a real SQLite default second"
        );
        assert_ne!(
            Database::content_digest(&image).unwrap(),
            Database::content_digest(&later).unwrap()
        );
        let before = bytes(&image);
        let flags = image
            .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
            .unwrap();
        if let Err(error) = verify_empty_storage_image(&image, &context) {
            diagnose_synthetic_empty_difference(&image, &context);
            panic!("actual empty verifier failed: {error}");
        }
        assert_eq!(bytes(&image), before);
        assert_eq!(
            image
                .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
                .unwrap(),
            flags
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn empty_image_rejects_unknown_schema_rows_and_credentials() {
    let _home = TestHome::new().unwrap();
    for context in contexts() {
        for change in [
            "unknown-row",
            "extra-column",
            "credential",
            "altered-default",
            "extra-trigger",
            "legacy-runtime-enabled",
            "legacy-default-row-missing",
            "legacy-clock-text",
            "legacy-clock-real",
            "legacy-clock-negative",
            "legacy-clock-out-of-range",
        ] {
            let image = empty_storage_image(&context).unwrap();
            match change {
                "unknown-row" => image.execute_batch("CREATE TABLE synthetic_unknown(value BLOB); INSERT INTO synthetic_unknown VALUES(X'00FF12');").unwrap(),
                "extra-column" => image.execute_batch("ALTER TABLE proxy_config ADD COLUMN synthetic_unknown TEXT").unwrap(),
                "credential" => {
                    let value = inventory::seal_db(&context, "settings", "value", &["global_proxy_url"], "synthetic-not-empty-secret").unwrap();
                    image.execute("INSERT INTO settings(key,value) VALUES('global_proxy_url',?1)", [value]).unwrap();
                },
                "altered-default" => { image.execute("UPDATE proxy_config SET listen_port=12345 WHERE app_type='claude'", []).unwrap(); },
                "extra-trigger" => image.execute_batch("CREATE TRIGGER synthetic_time_audit AFTER UPDATE ON proxy_config BEGIN DELETE FROM settings WHERE key='global_proxy_url'; END;").unwrap(),
                "legacy-runtime-enabled" => {
                    image.execute("UPDATE model_verification_settings SET runtime_auto_enabled=1", []).unwrap();
                },
                "legacy-default-row-missing" => {
                    image.execute("DELETE FROM model_verification_settings", []).unwrap();
                },
                "legacy-clock-text" => {
                    image.execute("UPDATE model_verification_settings SET updated_at='not-a-clock'", []).unwrap();
                },
                "legacy-clock-real" => {
                    image.execute("UPDATE model_verification_settings SET updated_at=1.5", []).unwrap();
                },
                "legacy-clock-negative" => {
                    image.execute("UPDATE model_verification_settings SET updated_at=-1", []).unwrap();
                },
                "legacy-clock-out-of-range" => {
                    image.execute("UPDATE model_verification_settings SET updated_at=?1", [i64::MAX]).unwrap();
                },
                _ => unreachable!(),
            }
            let before = bytes(&image);
            assert!(
                verify_empty_storage_image(&image, &context).is_err(),
                "admitted nonempty image: {change}"
            );
            assert_eq!(bytes(&image), before);
            assert!(image
                .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
                .unwrap());
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn empty_image_rejects_invalid_or_unequal_default_times() {
    let _home = TestHome::new().unwrap();
    let context = contexts()[0].clone();
    for (created, updated) in [
        ("2026-01-01 00:00:00", "2026-01-01 00:00:01"),
        ("2026-02-30 00:00:00", "2026-02-30 00:00:00"),
        ("2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        ("2026-01-01 00:00:00 ", "2026-01-01 00:00:00 "),
        ("", ""),
    ] {
        let image = empty_storage_image(&context).unwrap();
        image
            .execute(
                "UPDATE proxy_config SET created_at=?1,updated_at=?2",
                rusqlite::params![created, updated],
            )
            .unwrap();
        image
            .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
            .unwrap();
        let before = bytes(&image);
        assert!(verify_empty_storage_image(&image, &context).is_err());
        assert_eq!(bytes(&image), before);
        assert!(!image
            .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
            .unwrap());
    }
}
#[cfg(feature = "test-hooks")]
pub(super) fn verify(case: &str) {
    match case {
        "empty_image_accepts_actual_default_clock_difference_without_source_writes" => {
            empty_image_accepts_actual_default_clock_difference_without_source_writes()
        }
        "empty_image_rejects_unknown_schema_rows_and_credentials" => {
            empty_image_rejects_unknown_schema_rows_and_credentials()
        }
        "empty_image_rejects_invalid_or_unequal_default_times" => {
            empty_image_rejects_invalid_or_unequal_default_times()
        }
        _ => panic!("unknown empty image case"),
    }
}

// Test-only diagnostics over this module's synthetic empty fixture. Never used
// by runtime admission and never logs a caller/user database.
fn diagnose_synthetic_empty_difference(image: &Connection, context: &StorageContext) {
    fn tables(
        image: &Connection,
    ) -> std::collections::BTreeMap<String, (Vec<String>, Vec<String>)> {
        let names = image
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        names
            .into_iter()
            .map(|name| {
                let quoted = format!("\"{}\"", name.replace('"', "\"\""));
                let mut statement = image.prepare(&format!("SELECT * FROM {quoted}")).unwrap();
                let columns = statement
                    .column_names()
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let count = statement.column_count();
                let mut rows = statement
                    .query_map([], |row| {
                        (0..count)
                            .map(|index| row.get::<_, rusqlite::types::Value>(index))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .unwrap()
                    .map(|row| format!("{:?}", row.unwrap()))
                    .collect::<Vec<_>>();
                rows.sort();
                (name, (columns, rows))
            })
            .collect()
    }
    let mut private = Connection::open_in_memory().unwrap();
    vault::copy(image, &mut private).unwrap();
    let mut canonical = empty_storage_image(context).unwrap();
    println!("synthetic empty mode plaintext={}", context.is_plaintext());
    println!(
        "normalize clocks source={:?} canonical={:?}",
        normalize_empty_default_times(&mut private),
        normalize_empty_default_times(&mut canonical)
    );
    let left = tables(&private);
    let right = tables(&canonical);
    for (name, actual) in &left {
        if right.get(name) != Some(actual) {
            println!(
                "synthetic differing table={name} source={actual:?} canonical={:?}",
                right.get(name)
            );
        }
    }
    for name in right.keys().filter(|name| !left.contains_key(*name)) {
        println!("synthetic missing source table={name}");
    }
    // Distinguish logical-default differences from randomized encryption of
    // identical defaults, using the already-existing inventory normalization.
    super::super::storage_generation::with_private_triggers_disabled(&mut private, |image| {
        inventory::normalize_storage_values_for_digest(image, context)
    })
    .unwrap();
    super::super::storage_generation::with_private_triggers_disabled(&mut canonical, |image| {
        inventory::normalize_storage_values_for_digest(image, context)
    })
    .unwrap();
    println!(
        "after existing inventory normalization equal={}",
        Database::content_digest(&private).unwrap()
            == Database::content_digest(&canonical).unwrap()
    );
}
