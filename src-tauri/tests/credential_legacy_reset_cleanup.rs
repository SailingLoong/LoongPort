#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn committed_cleanup_recovers_after_database_removed_before_sidecars() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_cleanup_case(
        "committed_cleanup_recovers_after_database_removed_before_sidecars",
    );
}
#[test]
#[serial_test::serial]
fn committed_cleanup_recovers_after_each_sidecar_is_removed_first() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_cleanup_case(
        "committed_cleanup_recovers_after_each_sidecar_is_removed_first",
    );
}
#[test]
#[serial_test::serial]
fn committed_cleanup_recovers_after_metadata_and_sidecars_are_removed() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_cleanup_case(
        "committed_cleanup_recovers_after_metadata_and_sidecars_are_removed",
    );
}
#[test]
#[serial_test::serial]
fn committed_cleanup_still_rejects_protected_previous_after_partial_deletion() {
    cc_switch_lib::integration_test_support::verify_legacy_reset_cleanup_case(
        "committed_cleanup_still_rejects_protected_previous_after_partial_deletion",
    );
}
