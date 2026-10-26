#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn generation_image_equal_values_ignore_nonce_and_mode_identity() {
    cc_switch_lib::integration_test_support::verify_generation_image_case(
        "generation_image_equal_values_ignore_nonce_and_mode_identity",
    );
}
#[test]
#[serial_test::serial]
fn generation_image_rejects_secret_unknown_column_blob_and_schema_changes() {
    cc_switch_lib::integration_test_support::verify_generation_image_case(
        "generation_image_rejects_secret_unknown_column_blob_and_schema_changes",
    );
}
#[test]
#[serial_test::serial]
fn generation_image_authenticates_both_sides_and_rejects_identity_extensions() {
    cc_switch_lib::integration_test_support::verify_generation_image_case(
        "generation_image_authenticates_both_sides_and_rejects_identity_extensions",
    );
}
#[test]
#[serial_test::serial]
fn generation_image_private_normalization_does_not_fire_unknown_triggers() {
    cc_switch_lib::integration_test_support::verify_generation_image_case(
        "generation_image_private_normalization_does_not_fire_unknown_triggers",
    );
}

#[test]
#[serial_test::serial]
fn backup_generation_image_verifier_keeps_primary_exact() {
    cc_switch_lib::integration_test_support::verify_generation_image_case(
        "backup_generation_image_verifier_keeps_primary_exact",
    );
}
