#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn transition_rotate_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_rotate_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn transition_install_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_install_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn transition_upgrade_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_upgrade_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn transition_recover_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_recover_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn transition_plaintext_session_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_plaintext_session_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn transition_legacy_no_record_rotates() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_legacy_no_record_rotates",
    );
}
#[test]
#[serial_test::serial]
fn transition_target_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_target_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn transition_current_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "transition_current_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn rewrap_change_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "rewrap_change_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn rewrap_recover_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "rewrap_recover_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn rewrap_target_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "rewrap_target_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn rewrap_legacy_no_record_changes_password() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "rewrap_legacy_no_record_changes_password",
    );
}
#[test]
#[serial_test::serial]
fn rewrap_current_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "rewrap_current_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn bootstrap_finalize_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "bootstrap_finalize_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn bootstrap_recover_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "bootstrap_recover_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn bootstrap_target_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "bootstrap_target_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn bootstrap_legacy_no_record_restores() {
    cc_switch_lib::integration_test_support::verify_legacy_generation_fence_case(
        "bootstrap_legacy_no_record_restores",
    );
}
