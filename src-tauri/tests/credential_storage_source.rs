#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn local_text_source_prepares_for_explicit_plaintext() {
    cc_switch_lib::Database::verify_storage_source_case(
        "local_text_source_prepares_for_explicit_plaintext",
    );
}
#[test]
#[serial_test::serial]
fn local_plaintext_source_prepares_for_user_text() {
    cc_switch_lib::Database::verify_storage_source_case(
        "local_plaintext_source_prepares_for_user_text",
    );
}
#[test]
#[serial_test::serial]
fn local_text_source_prepares_for_another_user_text() {
    cc_switch_lib::Database::verify_storage_source_case(
        "local_text_source_prepares_for_another_user_text",
    );
}
#[test]
#[serial_test::serial]
fn remote_authenticated_text_accepts_plaintext_receiver() {
    cc_switch_lib::Database::verify_storage_source_case(
        "remote_authenticated_text_accepts_plaintext_receiver",
    );
}
#[test]
#[serial_test::serial]
fn remote_authenticated_text_accepts_user_text_receiver() {
    cc_switch_lib::Database::verify_storage_source_case(
        "remote_authenticated_text_accepts_user_text_receiver",
    );
}
#[test]
#[serial_test::serial]
fn storage_join_preserves_original_local_rows_and_unknown_schema() {
    cc_switch_lib::Database::verify_storage_source_case(
        "storage_join_preserves_original_local_rows_and_unknown_schema",
    );
}
#[test]
#[serial_test::serial]
fn local_source_rejects_wrong_exact_text_without_writes() {
    cc_switch_lib::Database::verify_storage_source_case(
        "local_source_rejects_wrong_exact_text_without_writes",
    );
}
#[test]
#[serial_test::serial]
fn remote_source_requires_manifest_and_authenticated_key_match() {
    cc_switch_lib::Database::verify_storage_source_case(
        "remote_source_requires_manifest_and_authenticated_key_match",
    );
}
#[test]
#[serial_test::serial]
fn storage_source_rejects_missing_mixed_and_corrupt_identity() {
    cc_switch_lib::Database::verify_storage_source_case(
        "storage_source_rejects_missing_mixed_and_corrupt_identity",
    );
}
#[test]
#[serial_test::serial]
fn storage_source_requires_actual_current_identity_and_schema() {
    cc_switch_lib::Database::verify_storage_source_case(
        "storage_source_requires_actual_current_identity_and_schema",
    );
}
#[test]
#[serial_test::serial]
fn explicit_legacy_sql_uses_original_admission_for_both_modes() {
    cc_switch_lib::Database::verify_storage_source_case(
        "explicit_legacy_sql_uses_original_admission_for_both_modes",
    );
}
#[test]
#[serial_test::serial]
fn original_installer_restores_plaintext_without_changing_mode() {
    cc_switch_lib::Database::verify_storage_source_case(
        "original_installer_restores_plaintext_without_changing_mode",
    );
}
#[test]
#[serial_test::serial]
fn original_installer_restores_user_text_without_changing_key() {
    cc_switch_lib::Database::verify_storage_source_case(
        "original_installer_restores_user_text_without_changing_key",
    );
}
#[test]
#[serial_test::serial]
fn typed_install_rejects_another_destination_before_safety_copy() {
    cc_switch_lib::Database::verify_storage_source_case(
        "typed_install_rejects_another_destination_before_safety_copy",
    );
}
#[test]
#[serial_test::serial]
fn typed_install_rejects_corrupted_staging_before_safety_copy() {
    cc_switch_lib::Database::verify_storage_source_case(
        "typed_install_rejects_corrupted_staging_before_safety_copy",
    );
}
#[test]
#[serial_test::serial]
fn original_installer_keeps_unknown_rows_despite_source_audit_trigger() {
    cc_switch_lib::Database::verify_storage_source_case(
        "original_installer_keeps_unknown_rows_despite_source_audit_trigger",
    );
}
#[test]
#[serial_test::serial]
fn typed_preparation_cannot_revive_after_session_block_unblock() {
    cc_switch_lib::Database::verify_storage_source_case(
        "typed_preparation_cannot_revive_after_session_block_unblock",
    );
}
#[test]
#[serial_test::serial]
fn typed_preparation_is_bound_to_original_receiver_session() {
    cc_switch_lib::Database::verify_storage_source_case(
        "typed_preparation_is_bound_to_original_receiver_session",
    );
}
