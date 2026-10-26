#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn reset_preview_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_preview_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn reset_write_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_write_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn reset_recover_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_recover_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn reset_publish_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_publish_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn reset_finish_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_finish_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn reset_current_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_current_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn reset_current_text_db_survives_corrupt_vault_record() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_current_text_db_survives_corrupt_vault_record",
    );
}
#[test]
#[serial_test::serial]
fn reset_readable_text_vault_fenced_without_matching_db() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_readable_text_vault_fenced_without_matching_db",
    );
}
#[test]
#[serial_test::serial]
fn reset_intent_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_intent_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn reset_target_text_without_record() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_target_text_without_record",
    );
}
#[test]
#[serial_test::serial]
fn reset_finish_staged_or_previous_protected_scope() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_finish_staged_or_previous_protected_scope",
    );
}
#[test]
#[serial_test::serial]
fn reset_finish_staged_or_previous_text_metadata() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_finish_staged_or_previous_text_metadata",
    );
}
#[test]
#[serial_test::serial]
fn reset_legacy_preview_without_record_stays_read_only() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_fence_case(
        "reset_legacy_preview_without_record_stays_read_only",
    );
}
