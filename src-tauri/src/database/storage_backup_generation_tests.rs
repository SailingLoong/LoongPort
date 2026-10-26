use super::storage_generation_tests::{
    assert_unchanged, assert_values, plaintext_successor, source, DERIVED, VALUE,
};
use super::{storage, vault, Database};
use crate::{
    error::AppError,
    secrets::{
        inventory,
        storage::{PlaintextIdentity, StorageContext},
        testing::TestHome,
        VaultContext,
    },
};
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
const ACTIVE: &str = "synthetic-active-physical-secret-48f9c7b0";
const OBSOLETE: &str = "synthetic-retired-physical-secret-a714c53f";
fn plaintext() -> StorageContext {
    StorageContext::Plaintext(PlaintextIdentity {
        format_version: 1,
        store_id: uuid::Uuid::new_v4().to_string(),
        generation_id: uuid::Uuid::new_v4().to_string(),
        revision: 1,
    })
}
fn encrypted_next(current: &StorageContext) -> StorageContext {
    VaultContext::create_generation_with_user_text(
        current.identity().store_id().to_owned(),
        current.identity().revision() + 1,
        "synthetic exact backup key",
    )
    .unwrap()
    .into()
}
fn error_code<T>(result: Result<T, AppError>, expected: &str) {
    let error = result
        .err()
        .expect("backup conversion unexpectedly succeeded");
    assert!(
        matches!(error,AppError::Config(ref code) if code==expected),
        "unexpected backup error: {error}"
    );
}
fn relic() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE providers (id TEXT PRIMARY KEY); INSERT INTO providers VALUES('synthetic-upstream-era'); PRAGMA user_version=20;").unwrap();
    conn
}
fn digest_files(root: &Path) -> BTreeMap<PathBuf, String> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            assert!(path.is_file());
            (
                path.file_name().unwrap().into(),
                crate::services::sync_protocol::sha256_hex(&std::fs::read(path).unwrap()),
            )
        })
        .collect()
}
fn contains(bytes: &[u8], text: &str) -> bool {
    bytes
        .windows(text.len())
        .any(|part| part == text.as_bytes())
}
fn assert_no_canaries(bytes: &[u8]) {
    for text in [VALUE, DERIVED, ACTIVE, OBSOLETE] {
        assert!(
            !contains(bytes, text),
            "plaintext canary survived physical target materialization"
        );
    }
}
fn persist_and_scan(image: &Connection, next: &StorageContext, root: &Path) {
    std::fs::create_dir(root).unwrap();
    let serialized = image.serialize(rusqlite::MAIN_DB).unwrap();
    assert_no_canaries(&serialized);
    let serialized_path = root.join("serialized.db");
    crate::config::atomic_write_private(&serialized_path, &serialized).unwrap();
    let reopened =
        Connection::open_with_flags(&serialized_path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    storage::check_identity(&reopened, next).unwrap();
    inventory::validate_storage_database(&reopened, next).unwrap();
    assert_values(&reopened, next);
    let mut wal = Connection::open(root.join("wal.db")).unwrap();
    vault::copy(image, &mut wal).unwrap();
    wal.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; UPDATE providers SET name='Synthetic WAL write';").unwrap();
    assert!(std::fs::metadata(root.join("wal.db-wal")).unwrap().len() > 32);
    let mut journal = Connection::open(root.join("journal.db")).unwrap();
    vault::copy(image, &mut journal).unwrap();
    journal.execute_batch("PRAGMA journal_mode=DELETE; BEGIN IMMEDIATE; UPDATE providers SET name='Synthetic rollback write';").unwrap();
    assert!(
        std::fs::metadata(root.join("journal.db-journal"))
            .unwrap()
            .len()
            > 0
    );
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        assert_no_canaries(&std::fs::read(path).unwrap());
    }
    journal.execute_batch("ROLLBACK").unwrap();
    storage::check_identity(&wal, next).unwrap();
    inventory::validate_storage_database(&wal, next).unwrap();
}
fn physical_conversion(use_backup: bool) {
    let home = TestHome::new().unwrap();
    let current = plaintext();
    let next = encrypted_next(&current);
    let source_dir = home.path().join("physical-source");
    std::fs::create_dir(&source_dir).unwrap();
    let path = source_dir.join("source.db");
    let seed = source(&current);
    let mut live = Connection::open(&path).unwrap();
    vault::copy(&seed, &mut live).unwrap();
    live.execute_batch(
        "PRAGMA secure_delete=OFF; PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;",
    )
    .unwrap();
    live.execute(
        "INSERT INTO settings(key,value) VALUES('global_proxy_url',?1)",
        [OBSOLETE.repeat(4096)],
    )
    .unwrap();
    live.execute(
        "UPDATE settings SET value=?1 WHERE key='global_proxy_url'",
        [ACTIVE],
    )
    .unwrap();
    let wal_bytes = std::fs::read(source_dir.join("source.db-wal")).unwrap();
    assert!(contains(&wal_bytes, OBSOLETE));
    assert!(contains(&wal_bytes, ACTIVE));
    let before = digest_files(&source_dir);
    let captured = super::inspection::capture(&path).unwrap().unwrap();
    assert!(
        contains(
            &captured.image.serialize(rusqlite::MAIN_DB).unwrap(),
            OBSOLETE
        ),
        "fixture must carry obsolete free-page bytes into conversion"
    );
    let output = if use_backup {
        storage::prepare_storage_backup_generation(&captured.image, &current, &current, &next)
    } else {
        storage::prepare_storage_generation(&captured.image, &current, &next)
    }
    .unwrap();
    assert_values(&output, &next);
    persist_and_scan(&output, &next, &home.path().join("physical-target"));
    super::inspection::verify_unchanged(&path, &captured.revision).unwrap();
    assert_eq!(before, digest_files(&source_dir));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_older_encrypted_revision_same_key_is_authenticated_and_preserved() {
    let _home = TestHome::new().unwrap();
    let original = VaultContext::generate().unwrap();
    let current = StorageContext::from(
        original
            .with_password("synthetic backup recovery password")
            .unwrap(),
    );
    let source_context = StorageContext::from(original);
    let next = encrypted_next(&current);
    let conn = source(&source_context);
    conn.execute_batch("CREATE TRIGGER synthetic_backup_audit AFTER UPDATE OF settings_config,meta,qoder_http_secret ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
    let before = Database::content_digest(&conn).unwrap();
    let output =
        storage::prepare_storage_backup_generation(&conn, &source_context, &current, &next)
            .unwrap();
    assert_values(&output, &next);
    assert_unchanged(&conn, &before);
    assert!(output
        .db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap());
    assert_eq!(
        Database::get_user_version(&output).unwrap(),
        Database::get_user_version(&conn).unwrap()
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_plaintext_exact_identity_converts_to_next_generation() {
    let _home = TestHome::new().unwrap();
    let current = plaintext();
    let next = plaintext_successor(&current);
    let conn = source(&current);
    let before = Database::content_digest(&conn).unwrap();
    let output =
        storage::prepare_storage_backup_generation(&conn, &current, &current, &next).unwrap();
    assert_values(&output, &next);
    assert_unchanged(&conn, &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_rejects_other_store_key_and_future_revision() {
    let _home = TestHome::new().unwrap();
    let original = VaultContext::generate().unwrap();
    let current = StorageContext::from(
        original
            .with_password("synthetic current recovery password")
            .unwrap(),
    );
    let next = plaintext_successor(&current);
    let wrong_store = VaultContext::generate().unwrap();
    let wrong_key = VaultContext::create_generation_with_user_key(
        current.identity().store_id().to_owned(),
        1,
        zeroize::Zeroizing::new(vec![19; 32]),
    )
    .unwrap();
    let future = current
        .encrypted()
        .unwrap()
        .with_password("synthetic future recovery password")
        .unwrap();
    for source_context in [wrong_store, wrong_key, future]
        .into_iter()
        .map(StorageContext::from)
    {
        let conn = source(&source_context);
        let before = Database::content_digest(&conn).unwrap();
        error_code(
            storage::prepare_storage_backup_generation(&conn, &source_context, &current, &next),
            "protection.backup_identity_mismatch",
        );
        assert_unchanged(&conn, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_plaintext_requires_exact_current_identity_and_mode() {
    let _home = TestHome::new().unwrap();
    let current = plaintext();
    let next = plaintext_successor(&current);
    let StorageContext::Plaintext(id) = &current else {
        unreachable!()
    };
    let mut changed = id.clone();
    changed.revision += 1;
    let sources = [
        plaintext_successor(&current),
        StorageContext::Plaintext(changed),
        StorageContext::from(VaultContext::generate().unwrap()),
    ];
    for source_context in sources {
        let conn = source(&source_context);
        let before = Database::content_digest(&conn).unwrap();
        error_code(
            storage::prepare_storage_backup_generation(&conn, &source_context, &current, &next),
            "protection.backup_identity_mismatch",
        );
        assert_unchanged(&conn, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_does_not_relax_primary_generation_contract() {
    let _home = TestHome::new().unwrap();
    let current = plaintext();
    let conn = source(&current);
    let before = Database::content_digest(&conn).unwrap();
    let StorageContext::Plaintext(good) = plaintext_successor(&current) else {
        unreachable!()
    };
    let mut owner = good.clone();
    owner.store_id = uuid::Uuid::new_v4().to_string();
    let mut repeated = good.clone();
    repeated.generation_id = current.identity().generation_id().into();
    let mut skipped = good;
    skipped.revision += 1;
    for next in [owner, repeated, skipped]
        .into_iter()
        .map(StorageContext::Plaintext)
    {
        error_code(
            storage::prepare_storage_backup_generation(&conn, &current, &current, &next),
            "protection.invalid_generation",
        );
        assert_unchanged(&conn, &before);
    }
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_corrupt_values_and_wrong_supplied_context_are_rejected() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = plaintext_successor(&current);
    let conn = source(&current);
    conn.execute("UPDATE providers SET qoder_http_secret='lpenc:corrupt'", [])
        .unwrap();
    let before = Database::content_digest(&conn).unwrap();
    error_code(
        storage::prepare_storage_backup_generation(&conn, &current, &current, &next),
        "secret.unsupported_format",
    );
    assert_unchanged(&conn, &before);
    let other = StorageContext::from(VaultContext::generate().unwrap());
    let conn = source(&other);
    let before = Database::content_digest(&conn).unwrap();
    error_code(
        storage::prepare_storage_backup_generation(&conn, &current, &current, &next),
        "secret.identity_mismatch",
    );
    assert_unchanged(&conn, &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_inventory_keeps_relic_visible_and_rejects_it_without_publication() {
    let home = TestHome::new().unwrap();
    let root = home.path().join("data");
    let dir = root.join("backups");
    std::fs::create_dir_all(&dir).unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = plaintext_successor(&current);
    for (name, image) in [
        ("a-current.db", source(&current)),
        ("b-prefork.db", relic()),
    ] {
        let mut file = Connection::open(dir.join(name)).unwrap();
        vault::copy(&image, &mut file).unwrap();
    }
    std::fs::write(dir.join("notes.txt"), b"synthetic ignored non-database").unwrap();
    let before = digest_files(&dir);
    let paths = crate::secrets::transition::database_backups(&root).unwrap();
    assert_eq!(
        paths,
        vec![dir.join("a-current.db"), dir.join("b-prefork.db")]
    );
    let captured = super::inspection::capture(&paths[1]).unwrap().unwrap();
    error_code(
        storage::prepare_storage_backup_generation(&captured.image, &current, &current, &next),
        "protection.backup_legacy_unsupported",
    );
    super::inspection::verify_unchanged(&paths[1], &captured.revision).unwrap();
    assert_eq!(before, digest_files(&dir));
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_persistent_pages_wal_and_journal_have_no_plaintext_canaries() {
    physical_conversion(true);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn primary_persistent_pages_wal_and_journal_have_no_plaintext_canaries() {
    physical_conversion(false);
}
#[cfg(feature = "test-hooks")]
pub(crate) fn verify_storage_backup_generation_case(case: &str) {
    match case {
        "backup_older_encrypted_revision_same_key_is_authenticated_and_preserved" => {
            backup_older_encrypted_revision_same_key_is_authenticated_and_preserved()
        }
        "backup_plaintext_exact_identity_converts_to_next_generation" => {
            backup_plaintext_exact_identity_converts_to_next_generation()
        }
        "backup_rejects_other_store_key_and_future_revision" => {
            backup_rejects_other_store_key_and_future_revision()
        }
        "backup_plaintext_requires_exact_current_identity_and_mode" => {
            backup_plaintext_requires_exact_current_identity_and_mode()
        }
        "backup_does_not_relax_primary_generation_contract" => {
            backup_does_not_relax_primary_generation_contract()
        }
        "backup_corrupt_values_and_wrong_supplied_context_are_rejected" => {
            backup_corrupt_values_and_wrong_supplied_context_are_rejected()
        }
        "backup_inventory_keeps_relic_visible_and_rejects_it_without_publication" => {
            backup_inventory_keeps_relic_visible_and_rejects_it_without_publication()
        }
        "backup_persistent_pages_wal_and_journal_have_no_plaintext_canaries" => {
            backup_persistent_pages_wal_and_journal_have_no_plaintext_canaries()
        }
        "primary_persistent_pages_wal_and_journal_have_no_plaintext_canaries" => {
            primary_persistent_pages_wal_and_journal_have_no_plaintext_canaries()
        }
        _ => panic!("unknown backup generation fixture: {case}"),
    }
}
