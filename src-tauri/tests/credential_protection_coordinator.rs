#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn coordinator_fresh() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry("fresh");
}
#[test]
#[serial_test::serial]
fn coordinator_plaintext_restart() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry(
        "plaintext_restart",
    );
}
#[test]
#[serial_test::serial]
fn coordinator_record_strict() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry("record_strict");
}
#[test]
#[serial_test::serial]
fn coordinator_record_preimage() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry("record_preimage");
}
#[test]
#[serial_test::serial]
fn coordinator_unknown() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry("unknown");
}
#[test]
#[serial_test::serial]
fn coordinator_internal_master() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry("internal_master");
}
#[test]
#[serial_test::serial]
fn coordinator_legacy_password() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry("legacy_password");
}
#[test]
#[serial_test::serial]
fn coordinator_text_record_missing() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry(
        "text_record_missing",
    );
}

#[test]
#[serial_test::serial]
fn coordinator_record_insert_race() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry(
        "record_insert_race",
    );
}

#[test]
#[serial_test::serial]
fn coordinator_record_replace_race() {
    cc_switch_lib::integration_test_support::verify_coordinator_protection_entry(
        "record_replace_race",
    );
}
