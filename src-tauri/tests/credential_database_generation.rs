#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn encrypted_to_plaintext_keeps_derived_and_unknown_rows_without_mutating_source() {
    cc_switch_lib::integration_test_support::verify_credential_generation_case(
        "encrypted_to_plaintext_keeps_derived_and_unknown_rows_without_mutating_source",
    );
}
#[test]
#[serial_test::serial]
fn plaintext_to_encrypted_keeps_derived_and_unknown_rows_without_mutating_source() {
    cc_switch_lib::integration_test_support::verify_credential_generation_case(
        "plaintext_to_encrypted_keeps_derived_and_unknown_rows_without_mutating_source",
    );
}
#[test]
#[serial_test::serial]
fn explicit_plaintext_generation_change_keeps_values_and_rebinds_identity() {
    cc_switch_lib::integration_test_support::verify_credential_generation_case(
        "explicit_plaintext_generation_change_keeps_values_and_rebinds_identity",
    );
}
#[test]
#[serial_test::serial]
fn generation_conversion_rejects_owner_revision_and_generation_changes_outside_contract() {
    cc_switch_lib::integration_test_support::verify_credential_generation_case(
        "generation_conversion_rejects_owner_revision_and_generation_changes_outside_contract",
    );
}
#[test]
#[serial_test::serial]
fn generation_conversion_does_not_accept_corrupt_encrypted_values() {
    cc_switch_lib::integration_test_support::verify_credential_generation_case(
        "generation_conversion_does_not_accept_corrupt_encrypted_values",
    );
}
#[test]
#[serial_test::serial]
fn generation_conversion_preserves_unknown_rows_and_restores_audit_trigger() {
    cc_switch_lib::integration_test_support::verify_credential_generation_case(
        "generation_conversion_preserves_unknown_rows_and_restores_audit_trigger",
    );
}
