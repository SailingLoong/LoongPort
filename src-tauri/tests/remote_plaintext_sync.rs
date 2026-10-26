#![cfg(feature = "test-hooks")]

#[test]
#[serial_test::serial]
fn plaintext_snapshot_export() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("plain_export");
}

#[test]
#[serial_test::serial]
fn encrypted_snapshot_export_compatibility() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("encrypted_export");
}

#[test]
#[serial_test::serial]
fn mixed_descriptor_is_rejected() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("mixed_descriptor");
}
#[test]
#[serial_test::serial]
fn plaintext_source_restores_plaintext_without_input() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("plain_restore");
}
#[test]
#[serial_test::serial]
fn plaintext_source_preserves_encrypted_target() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("plain_to_encrypted");
}
#[test]
#[serial_test::serial]
fn plaintext_source_rejects_unneeded_input() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("plain_reject_input");
}

#[test]
#[serial_test::serial]
fn plaintext_source_preserves_legacy_encrypted_owner() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("plain_to_legacy");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn original_recovery_input_descriptor() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("recovery_input");
}

#[test]
#[serial_test::serial]
fn plaintext_skill_bytes_and_old_lease_retirement() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("skill_bytes");
}
#[test]
#[serial_test::serial]
fn missing_target_record_remains_failclosed() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_case("missing_choice");
}
