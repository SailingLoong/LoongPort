#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn user_text_sync_snapshot_preserves_metadata_and_values() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "user_text_sync_snapshot_preserves_metadata_and_values",
    );
}
#[test]
#[serial_test::serial]
fn user_text_file_export_reopens_with_exact_text() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "user_text_file_export_reopens_with_exact_text",
    );
}
#[test]
#[serial_test::serial]
fn user_text_export_rejects_corrupt_values_before_creating_file() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "user_text_export_rejects_corrupt_values_before_creating_file",
    );
}
#[test]
#[serial_test::serial]
fn user_text_export_rejects_malformed_descriptors() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "user_text_export_rejects_malformed_descriptors",
    );
}
#[test]
#[serial_test::serial]
fn legacy_portable_material_contract_is_preserved() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "legacy_portable_material_contract_is_preserved",
    );
}
#[test]
#[serial_test::serial]
fn plaintext_sync_does_not_gain_implicit_portability() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "plaintext_sync_does_not_gain_implicit_portability",
    );
}
#[test]
#[serial_test::serial]
fn sql_source_user_text_authenticates_without_bypassing_installer() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "sql_source_user_text_authenticates_without_bypassing_installer",
    );
}
#[test]
#[serial_test::serial]
fn sql_source_wrong_text_never_falls_back_to_legacy_wrapper() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "sql_source_wrong_text_never_falls_back_to_legacy_wrapper",
    );
}
#[test]
#[serial_test::serial]
fn sql_source_legacy_wrapper_uses_metadata_not_input_shape() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "sql_source_legacy_wrapper_uses_metadata_not_input_shape",
    );
}
#[test]
#[serial_test::serial]
fn sql_source_missing_descriptor_has_no_user_text_fallback() {
    cc_switch_lib::Database::verify_user_text_portable_case(
        "sql_source_missing_descriptor_has_no_user_text_fallback",
    );
}
