//! Production backup-image entry: caller never reconstructs a source context.
use crate::{
    database::{storage, storage_generation_tests as fixture, Database},
    error::AppError,
    secrets::{storage::StorageContext, testing::TestHome, VaultContext},
};
use rusqlite::Connection;
fn code<T>(result: Result<T, AppError>, expected: &str) {
    let error = result.err().expect("backup image unexpectedly admitted");
    assert!(
        matches!(error,AppError::Config(ref actual) if actual==expected),
        "unexpected image admission error: {error}"
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_image_owns_older_same_key_authentication() {
    let _home = TestHome::new().unwrap();
    let old = VaultContext::generate().unwrap();
    let current = StorageContext::from(
        old.with_password("synthetic current backup wrapper")
            .unwrap(),
    );
    let next = fixture::plaintext_successor(&current);
    let source = fixture::source(&StorageContext::from(old));
    source.execute_batch("CREATE TRIGGER image_audit AFTER UPDATE OF settings_config,meta,qoder_http_secret ON providers BEGIN UPDATE future_unknown_data SET value=X'010203' WHERE id=7; END;").unwrap();
    let before = Database::content_digest(&source).unwrap();
    let prepared =
        storage::prepare_storage_backup_generation_from_image(&source, &current, &next).unwrap();
    fixture::assert_values(&prepared, &next);
    fixture::assert_unchanged(&source, &before);
    assert!(prepared
        .db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap());
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_image_plaintext_identity_is_exact() {
    let _home = TestHome::new().unwrap();
    let current =
        fixture::plaintext_successor(&StorageContext::from(VaultContext::generate().unwrap()));
    let next = StorageContext::from(
        VaultContext::create_generation_with_user_text(
            current.identity().store_id().into(),
            current.identity().revision() + 1,
            "synthetic image exact text",
        )
        .unwrap(),
    );
    let source = fixture::source(&current);
    let before = Database::content_digest(&source).unwrap();
    let prepared =
        storage::prepare_storage_backup_generation_from_image(&source, &current, &next).unwrap();
    fixture::assert_values(&prepared, &next);
    fixture::assert_unchanged(&source, &before);
    let other = fixture::plaintext_successor(&current);
    let source = fixture::source(&other);
    let before = Database::content_digest(&source).unwrap();
    code(
        storage::prepare_storage_backup_generation_from_image(&source, &current, &next),
        "protection.backup_identity_mismatch",
    );
    fixture::assert_unchanged(&source, &before);
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_image_rejects_foreign_key_future_revision_and_invalid_edge() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = fixture::plaintext_successor(&current);
    let other = VaultContext::generate().unwrap();
    let future = current
        .encrypted()
        .unwrap()
        .with_password("synthetic future backup wrapper")
        .unwrap();
    for wrong in [other, future] {
        let source = fixture::source(&StorageContext::from(wrong));
        let before = Database::content_digest(&source).unwrap();
        code(
            storage::prepare_storage_backup_generation_from_image(&source, &current, &next),
            "protection.backup_identity_mismatch",
        );
        fixture::assert_unchanged(&source, &before);
    }
    let source = fixture::source(&current);
    code(
        storage::prepare_storage_backup_generation_from_image(&source, &current, &current),
        "protection.invalid_generation",
    );
}
#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn backup_image_verifies_actual_key_and_keeps_relic_failclosed() {
    let _home = TestHome::new().unwrap();
    let current = StorageContext::from(VaultContext::generate().unwrap());
    let next = fixture::plaintext_successor(&current);
    let wrong = VaultContext::generate().unwrap();
    let source = fixture::source(&StorageContext::from(wrong.clone()));
    // Copy public IDs onto a different real key's verifier. A caller-side ID
    // match is insufficient; the single DB owner must authenticate actual key.
    let mut forged = wrong.metadata().clone();
    forged.vault_id = current.identity().store_id().into();
    forged.key_id = current.identity().generation_id().into();
    forged.revision = current.identity().revision();
    source
        .execute(
            "UPDATE loongport_vault SET metadata=?1 WHERE id=1",
            [serde_json::to_string(&forged).unwrap()],
        )
        .unwrap();
    let before = Database::content_digest(&source).unwrap();
    code(
        storage::prepare_storage_backup_generation_from_image(&source, &current, &next),
        "secret.key_rejected",
    );
    fixture::assert_unchanged(&source, &before);
    let source = fixture::source(&current);
    source
        .execute("UPDATE providers SET qoder_http_secret='lpenc:corrupt'", [])
        .unwrap();
    let before = Database::content_digest(&source).unwrap();
    code(
        storage::prepare_storage_backup_generation_from_image(&source, &current, &next),
        "secret.unsupported_format",
    );
    fixture::assert_unchanged(&source, &before);
    let relic = Connection::open_in_memory().unwrap();
    relic
        .execute_batch("CREATE TABLE providers(id TEXT PRIMARY KEY); PRAGMA user_version=20;")
        .unwrap();
    let before = Database::content_digest(&relic).unwrap();
    code(
        storage::prepare_storage_backup_generation_from_image(&relic, &current, &next),
        "protection.backup_legacy_unsupported",
    );
    fixture::assert_unchanged(&relic, &before);
}
pub(super) fn verify(case: &str) {
    match case {
        "backup_image_owns_older_same_key_authentication" => {
            backup_image_owns_older_same_key_authentication()
        }
        "backup_image_plaintext_identity_is_exact" => backup_image_plaintext_identity_is_exact(),
        "backup_image_rejects_foreign_key_future_revision_and_invalid_edge" => {
            backup_image_rejects_foreign_key_future_revision_and_invalid_edge()
        }
        "backup_image_verifies_actual_key_and_keeps_relic_failclosed" => {
            backup_image_verifies_actual_key_and_keeps_relic_failclosed()
        }
        _ => panic!("unknown backup image case"),
    }
}
