#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn backup_image_owns_older_same_key_authentication() {
    cc_switch_lib::integration_test_support::verify_backup_image_case(
        "backup_image_owns_older_same_key_authentication",
    );
}
#[test]
#[serial_test::serial]
fn backup_image_plaintext_identity_is_exact() {
    cc_switch_lib::integration_test_support::verify_backup_image_case(
        "backup_image_plaintext_identity_is_exact",
    );
}
#[test]
#[serial_test::serial]
fn backup_image_rejects_foreign_key_future_revision_and_invalid_edge() {
    cc_switch_lib::integration_test_support::verify_backup_image_case(
        "backup_image_rejects_foreign_key_future_revision_and_invalid_edge",
    );
}
#[test]
#[serial_test::serial]
fn backup_image_verifies_actual_key_and_keeps_relic_failclosed() {
    cc_switch_lib::integration_test_support::verify_backup_image_case(
        "backup_image_verifies_actual_key_and_keeps_relic_failclosed",
    );
}
