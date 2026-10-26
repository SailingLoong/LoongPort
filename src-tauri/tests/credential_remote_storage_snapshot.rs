#![cfg(feature = "test-hooks")]

#[test]
#[serial_test::serial]
fn storage_sync_plaintext_export_returns_actual_identity() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_plaintext_export_returns_actual_identity",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_encrypted_export_keeps_portable_ciphertext() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_encrypted_export_keeps_portable_ciphertext",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_plaintext_export_rejects_damaged_identity_and_inventory() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_plaintext_export_rejects_damaged_identity_and_inventory",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_plaintext_source_prepares_for_both_destination_modes() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_plaintext_source_prepares_for_both_destination_modes",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_plaintext_manifest_and_source_identity_must_match() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_plaintext_manifest_and_source_identity_must_match",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_modern_unmarked_source_is_never_plaintext_fallback() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_modern_unmarked_source_is_never_plaintext_fallback",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_future_and_receiver_schema_remain_rejected() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_future_and_receiver_schema_remain_rejected",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_plaintext_and_encrypted_stamps_cannot_mix() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_plaintext_and_encrypted_stamps_cannot_mix",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_plaintext_source_keeps_original_sql_sandbox() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_plaintext_source_keeps_original_sql_sandbox",
    );
}

#[test]
#[serial_test::serial]
fn storage_sync_encrypted_old_revision_keeps_original_admission() {
    cc_switch_lib::Database::verify_remote_storage_snapshot_case(
        "storage_sync_encrypted_old_revision_keeps_original_admission",
    );
}
