//! Actual original snapshot/SQL owners, synthetic local data only.
use super::*;
use crate::database::{storage, storage_generation_tests as fixture, storage_identity};
use crate::secrets::{
    inventory, session::SecretSession, storage::PlaintextIdentity, testing::TestHome,
    transition::fence_test_support::tree,
};

fn plain() -> StorageContext {
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    })
}
fn encrypted() -> StorageContext {
    VaultContext::create_generation_with_user_text(
        uuid::Uuid::new_v4().to_string(),
        1,
        "  synthetic remote key 用户\n ",
    )
    .unwrap()
    .into()
}
fn db(home: &TestHome, context: &StorageContext, version: i32) -> Database {
    let conn = fixture::source(context);
    if version == 20 {
        Database::apply_upstream4_migrations_on_conn(&conn).unwrap();
    }
    let root = home.path().join(uuid::Uuid::new_v4().to_string());
    crate::config::ensure_private_directory(&root).unwrap();
    let mut persisted = Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
    crate::database::vault::copy(&conn, &mut persisted).unwrap();
    Database::from_connection(
        persisted,
        SecretSession::from_storage_context(root, context.clone()),
    )
}
fn sql(db: &Database) -> String {
    Database::dump_sql(&db.conn.lock().unwrap(), SYNC_SKIP_TABLES).unwrap()
}
fn code<T>(result: Result<T, AppError>, expected: &str) {
    match result {
        Err(AppError::Config(actual)) => assert_eq!(actual, expected),
        Err(error) => panic!("expected {expected}, got {error}"),
        Ok(_) => panic!("expected {expected}, unexpectedly succeeded"),
    }
}
fn reject(
    home: &TestHome,
    local: &Database,
    source_sql: &str,
    expected: &StorageIdentity,
    source: &StorageContext,
    error: &str,
) {
    let before = tree(home.path());
    let digest = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    let current = local.secret_session().read().unwrap();
    code(
        local.prepare_sync_storage_snapshot_for_storage(
            source_sql, expected, source, &current, &current,
        ),
        error,
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &digest);
}

// Compare independently generated dumps without treating their export clock as data.
fn sync_sql_matches_ignoring_generated_at(left: &str, right: &str) -> bool {
    fn stable_parts(sql: &str) -> Option<(&str, &str)> {
        let (title, rest) = sql.split_once('\n')?;
        if title != "-- CC Switch SQLite 导出" {
            return None;
        }
        let (generated, body) = rest.split_once('\n')?;
        let timestamp = generated.strip_prefix("-- 生成时间: ")?;
        if timestamp.len() != 19
            || !timestamp
                .bytes()
                .enumerate()
                .all(|(index, byte)| match index {
                    4 | 7 => byte == b'-',
                    10 => byte == b' ',
                    13 | 16 => byte == b':',
                    _ => byte.is_ascii_digit(),
                })
        {
            return None;
        }
        let (version, _) = body.split_once('\n')?;
        let version = version.strip_prefix("-- user_version: ")?;
        if version.parse::<i64>().ok()?.to_string() != version {
            return None;
        }
        // Keep the first line and every byte after the clock line, including
        // embedded newlines, comments, identity metadata, and ciphertext.
        Some((title, body))
    }

    match (stable_parts(left), stable_parts(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_plaintext_export_returns_actual_identity() {
    let home = TestHome::new().unwrap();
    let context = plain();
    for version in [17, 20] {
        let source = db(&home, &context, version);
        let before = tree(home.path());
        let digest = Database::content_digest(&source.conn.lock().unwrap()).unwrap();
        let (output, identity) = source.export_sync_storage_snapshot().unwrap();
        assert_eq!(identity, context.identity());
        assert!(output.contains(fixture::VALUE));
        assert!(output.contains(fixture::DERIVED));
        let image = Database::stage_import_sql(&output, true).unwrap();
        fixture::assert_values(&image, &context);
        assert!(crate::database::vault::stored_metadata(&image)
            .unwrap()
            .is_none());
        assert!(storage_identity::stored(&image).unwrap().is_some());
        fixture::assert_unchanged(&source.conn.lock().unwrap(), &digest);
        assert_eq!(tree(home.path()), before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_encrypted_export_keeps_portable_ciphertext() {
    let home = TestHome::new().unwrap();
    let context = encrypted();
    let source = db(&home, &context, 17);
    let before = tree(home.path());
    let (output, identity) = source.export_sync_storage_snapshot().unwrap();
    assert_eq!(identity, context.identity());
    assert!(!output.contains(fixture::VALUE));
    assert!(!output.contains(fixture::DERIVED));
    let (legacy, metadata) = source.export_sync_snapshot().unwrap();
    assert_eq!(identity, StorageIdentity::Encrypted(Box::new(metadata)));
    assert!(sync_sql_matches_ignoring_generated_at(&legacy, &output));
    let image = Database::stage_import_sql(&output, true).unwrap();
    fixture::assert_values(&image, &context);
    assert_eq!(tree(home.path()), before);
    let raw = db(&home, &VaultContext::generate().unwrap().into(), 17);
    code(
        raw.export_sync_storage_snapshot(),
        "sync.recovery_password_required",
    );
    code(
        raw.export_sync_snapshot(),
        "sync.recovery_password_required",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_plaintext_export_rejects_damaged_identity_and_inventory() {
    let home = TestHome::new().unwrap();
    let context = plain();
    for change in ["ciphertext", "identity"] {
        let source = db(&home, &context, 17);
        let expected = if change == "ciphertext" {
            source
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE providers SET qoder_http_secret='lpenc1.synthetic-damage'",
                    [],
                )
                .unwrap();
            "secret.unsupported_format"
        } else {
            let StorageContext::Plaintext(other) = plain() else {
                unreachable!()
            };
            source
                .conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE loongport_plaintext_storage SET identity=?1",
                    [serde_json::to_string(&other).unwrap()],
                )
                .unwrap();
            "protection.database_identity_mismatch"
        };
        let before = tree(home.path());
        code(source.export_sync_storage_snapshot(), expected);
        assert_eq!(tree(home.path()), before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_plaintext_source_prepares_for_both_destination_modes() {
    let home = TestHome::new().unwrap();
    let source_context = plain();
    for current in [plain(), encrypted()] {
        for version in [17, 20] {
            let source = db(&home, &source_context, version);
            let local = db(&home, &current, version);
            let before = tree(home.path());
            let local_digest = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
            let guard = local.secret_session().read().unwrap();
            let epoch = local.secret_session().lifecycle_epoch();
            let staged = local
                .prepare_sync_storage_snapshot_for_storage(
                    &sql(&source),
                    &source_context.identity(),
                    &source_context,
                    &guard,
                    &guard,
                )
                .unwrap();
            assert_eq!(staged.destination, current.identity());
            let (receiver, prepared_epoch) = staged.receiver.as_ref().unwrap();
            assert_eq!(*prepared_epoch, epoch);
            assert!(std::sync::Arc::ptr_eq(
                &receiver.upgrade().unwrap(),
                &local.secrets
            ));
            storage::check_identity(&staged.connection, &current).unwrap();
            inventory::validate_storage_database(&staged.connection, &current).unwrap();
            fixture::assert_values(&staged.connection, &current);
            assert_eq!(
                Database::get_user_version(&staged.connection).unwrap(),
                version
            );
            fixture::assert_unchanged(&local.conn.lock().unwrap(), &local_digest);
            assert_eq!(tree(home.path()), before);
        }
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_plaintext_manifest_and_source_identity_must_match() {
    let home = TestHome::new().unwrap();
    let source_context = plain();
    let source = db(&home, &source_context, 17);
    let local = db(&home, &plain(), 17);
    let wrong = plain();
    reject(
        &home,
        &local,
        &sql(&source),
        &wrong.identity(),
        &source_context,
        "sync.source_identity_mismatch",
    );
    reject(
        &home,
        &local,
        &sql(&source),
        &source_context.identity(),
        &wrong,
        "sync.source_identity_mismatch",
    );
    let encrypted = encrypted();
    reject(
        &home,
        &local,
        &sql(&source),
        &encrypted.identity(),
        &source_context,
        "sync.source_identity_mismatch",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_modern_unmarked_source_is_never_plaintext_fallback() {
    let home = TestHome::new().unwrap();
    let source_context = plain();
    let source = db(&home, &source_context, 17);
    source
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "DROP TABLE loongport_plaintext_storage; DROP TABLE IF EXISTS loongport_vault;",
        )
        .unwrap();
    let local = db(&home, &plain(), 17);
    reject(
        &home,
        &local,
        &sql(&source),
        &source_context.identity(),
        &source_context,
        "sync.source_identity_mismatch",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_future_and_receiver_schema_remain_rejected() {
    let home = TestHome::new().unwrap();
    let source_context = plain();
    let local = db(&home, &plain(), 17);
    for kind in ["upstream", "loongport", "newer-supported"] {
        let source = db(
            &home,
            &source_context,
            if kind == "newer-supported" { 20 } else { 17 },
        );
        let expected = match kind {
            "upstream" => {
                source
                    .conn
                    .lock()
                    .unwrap()
                    .pragma_update(None, "user_version", crate::database::SCHEMA_VERSION + 1)
                    .unwrap();
                "secret.database_version_too_new"
            }
            "loongport" => {
                source
                    .conn
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE loongport_schema_version SET version=?1",
                        [crate::database::loongport_schema::LOONGPORT_SCHEMA_VERSION + 1],
                    )
                    .unwrap();
                "secret.database_version_too_new"
            }
            _ => "sync.database_version_incompatible",
        };
        reject(
            &home,
            &local,
            &sql(&source),
            &source_context.identity(),
            &source_context,
            expected,
        );
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_plaintext_and_encrypted_stamps_cannot_mix() {
    let home = TestHome::new().unwrap();
    let source_context = plain();
    let source = db(&home, &source_context, 17);
    let vault = VaultContext::generate().unwrap();
    crate::database::vault::create_schema(&source.conn.lock().unwrap()).unwrap();
    source
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO loongport_vault(id,metadata) VALUES(1,?1)",
            [serde_json::to_string(vault.metadata()).unwrap()],
        )
        .unwrap();
    let local = db(&home, &plain(), 17);
    reject(
        &home,
        &local,
        &sql(&source),
        &source_context.identity(),
        &source_context,
        "protection.encrypted_data_present",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_plaintext_source_keeps_original_sql_sandbox() {
    let home = TestHome::new().unwrap();
    let source_context = plain();
    let source = db(&home, &source_context, 17);
    let local = db(&home, &plain(), 17);
    let path = home.path().join("synthetic-unapproved-attach.db");
    let raw = format!(
        "{}\nATTACH DATABASE '{}' AS escaped;",
        sql(&source),
        path.display().to_string().replace('\'', "''")
    );
    reject(
        &home,
        &local,
        &raw,
        &source_context.identity(),
        &source_context,
        "backup.sql.unauthorized",
    );
    assert!(!path.exists());

    const CANARY: &str = "SYNTHETIC_REMOTE_SQL_BODY_SECRET_DO_NOT_ECHO";
    let malformed = format!(
        "{}\nINSERT INTO synthetic_missing_table VALUES('{CANARY}');",
        sql(&source)
    );
    reject(
        &home,
        &local,
        &malformed,
        &source_context.identity(),
        &source_context,
        "backup.sql.invalid",
    );
    source
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE providers SET settings_config=?1",
            [CANARY.as_bytes()],
        )
        .unwrap();
    let before = tree(home.path());
    let digest = Database::content_digest(&local.conn.lock().unwrap()).unwrap();
    let current = local.secret_session().read().unwrap();
    let error = local
        .prepare_sync_storage_snapshot_for_storage(
            &sql(&source),
            &source_context.identity(),
            &source_context,
            &current,
            &current,
        )
        .err()
        .expect("a BLOB credential must not become accepted plaintext");
    assert!(
        matches!(error, AppError::Config(ref code) if code == "secret.invalid_text"),
        "unexpected field type error: {error}"
    );
    assert!(
        !error.to_string().contains(CANARY),
        "field error echoed source content"
    );
    assert_eq!(tree(home.path()), before);
    fixture::assert_unchanged(&local.conn.lock().unwrap(), &digest);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn storage_sync_encrypted_old_revision_keeps_original_admission() {
    let home = TestHome::new().unwrap();
    let older = VaultContext::generate()
        .unwrap()
        .with_password("synthetic older backup password")
        .unwrap();
    let newer = older
        .with_password("synthetic newer backup password")
        .unwrap();
    assert!(older.metadata().revision < newer.metadata().revision);
    let source_context: StorageContext = older.clone().into();
    let authenticated: StorageContext = newer.into();
    let source = db(&home, &source_context, 17);
    let current = plain();
    let local = db(&home, &current, 17);
    let before = tree(home.path());
    let guard = local.secret_session().read().unwrap();
    let staged = local
        .prepare_sync_storage_snapshot_for_storage(
            &sql(&source),
            &source_context.identity(),
            &authenticated,
            &guard,
            &guard,
        )
        .unwrap();
    fixture::assert_values(&staged.connection, &current);
    assert_eq!(tree(home.path()), before);
}

#[cfg(feature = "test-hooks")]
pub(super) fn verify(case: &str) {
    match case {
        "storage_sync_plaintext_export_returns_actual_identity" => {
            storage_sync_plaintext_export_returns_actual_identity()
        }
        "storage_sync_encrypted_export_keeps_portable_ciphertext" => {
            storage_sync_encrypted_export_keeps_portable_ciphertext()
        }
        "storage_sync_plaintext_export_rejects_damaged_identity_and_inventory" => {
            storage_sync_plaintext_export_rejects_damaged_identity_and_inventory()
        }
        "storage_sync_plaintext_source_prepares_for_both_destination_modes" => {
            storage_sync_plaintext_source_prepares_for_both_destination_modes()
        }
        "storage_sync_plaintext_manifest_and_source_identity_must_match" => {
            storage_sync_plaintext_manifest_and_source_identity_must_match()
        }
        "storage_sync_modern_unmarked_source_is_never_plaintext_fallback" => {
            storage_sync_modern_unmarked_source_is_never_plaintext_fallback()
        }
        "storage_sync_future_and_receiver_schema_remain_rejected" => {
            storage_sync_future_and_receiver_schema_remain_rejected()
        }
        "storage_sync_plaintext_and_encrypted_stamps_cannot_mix" => {
            storage_sync_plaintext_and_encrypted_stamps_cannot_mix()
        }
        "storage_sync_plaintext_source_keeps_original_sql_sandbox" => {
            storage_sync_plaintext_source_keeps_original_sql_sandbox()
        }
        "storage_sync_encrypted_old_revision_keeps_original_admission" => {
            storage_sync_encrypted_old_revision_keeps_original_admission()
        }
        _ => panic!("unknown remote storage snapshot case"),
    }
}

#[cfg(test)]
mod sync_sql_comparison_tests {
    use super::sync_sql_matches_ignoring_generated_at;

    const DUMP: &str = "-- CC Switch SQLite 导出\n\
-- 生成时间: 2026-10-27 22:00:00\n\
-- user_version: 17\n\
PRAGMA user_version=17;\n\
CREATE TABLE loongport_vault (metadata TEXT);\n\
INSERT INTO loongport_vault VALUES ('{\"vaultId\":\"synthetic-vault\",\"keyId\":\"synthetic-key\"}');\n\
INSERT INTO providers VALUES ('lpenc1.synthetic-ciphertext');\n\
COMMIT;\n";

    #[test]
    fn independently_generated_sync_dumps_may_cross_a_second() {
        let later = DUMP.replacen("2026-10-27 22:00:00", "2026-10-27 22:00:01", 1);
        assert_ne!(DUMP, later);
        assert!(sync_sql_matches_ignoring_generated_at(DUMP, &later));
        assert!(sync_sql_matches_ignoring_generated_at(&later, DUMP));
        assert!(sync_sql_matches_ignoring_generated_at(DUMP, DUMP));
    }

    #[test]
    fn malformed_or_displaced_fixed_headers_are_not_ignored() {
        for malformed in [
            String::new(),
            DUMP.replacen("-- CC Switch SQLite 导出", "-- other export", 1),
            DUMP.replacen("-- 生成时间: ", "-- other clock: ", 1),
            DUMP.replacen("2026-10-27 22:00:00", "", 1),
            DUMP.replacen("2026-10-27 22:00:00", "2026-10-27T22:00:00", 1),
            DUMP.replacen("2026-10-27 22:00:00", "202x-10-27 22:00:00", 1),
            DUMP.replacen("2026-10-27 22:00:00", "2026-10-27 22:00:00 extra", 1),
            DUMP.replacen("-- user_version: 17", "-- another field: 17", 1),
            DUMP.replacen("-- user_version: 17", "-- user_version: invalid", 1),
            DUMP.replacen("-- user_version: 17", "-- user_version: 017", 1),
            format!("\n{DUMP}"),
            DUMP.replace('\n', "\r\n"),
        ] {
            assert!(!sync_sql_matches_ignoring_generated_at(
                &malformed, &malformed
            ));
            assert!(!sync_sql_matches_ignoring_generated_at(DUMP, &malformed));
        }
    }

    #[test]
    fn every_non_clock_character_remains_part_of_the_comparison() {
        let clock_start = DUMP.find("2026-10-27 22:00:00").unwrap();
        let clock_end = clock_start + "2026-10-27 22:00:00".len();
        // Includes SQL, the schema version, vault/key identity, and ciphertext.
        for (index, character) in DUMP.char_indices() {
            if (clock_start..clock_end).contains(&index) {
                continue;
            }
            let mut changed = DUMP.to_owned();
            changed.replace_range(
                index..index + character.len_utf8(),
                if character == 'X' { "Y" } else { "X" },
            );
            assert!(
                !sync_sql_matches_ignoring_generated_at(DUMP, &changed),
                "changed byte {index}"
            );
        }
        assert!(!sync_sql_matches_ignoring_generated_at(
            DUMP,
            DUMP.trim_end()
        ));
        assert!(!sync_sql_matches_ignoring_generated_at(
            DUMP,
            &format!("{DUMP}\n")
        ));
    }

    #[test]
    fn clock_like_text_inside_sql_is_never_filtered() {
        let source = format!("{DUMP}INSERT INTO settings VALUES ('line one\n-- 生成时间: 2026-10-27 22:00:00\nline three');\n");
        let changed_payload = source.replace("line three", "changed payload");
        let changed_embedded_clock = source.replacen(
            "\n-- 生成时间: 2026-10-27 22:00:00\nline three",
            "\n-- 生成时间: 2026-10-27 22:00:01\nline three",
            1,
        );
        assert!(sync_sql_matches_ignoring_generated_at(&source, &source));
        assert!(!sync_sql_matches_ignoring_generated_at(
            &source,
            &changed_payload
        ));
        assert!(!sync_sql_matches_ignoring_generated_at(
            &source,
            &changed_embedded_clock
        ));
    }
}
