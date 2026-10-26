#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn protected_startup_fresh() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("fresh");
}
#[test]
#[serial_test::serial]
fn protected_startup_cache() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("cache");
}
#[test]
#[serial_test::serial]
fn protected_startup_manual() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("manual");
}
#[test]
#[serial_test::serial]
fn protected_startup_unknown() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("unknown");
}

#[test]
#[serial_test::serial]
fn protected_startup_text_cache() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("text_cache");
}
#[test]
#[serial_test::serial]
fn protected_startup_text_missing_record() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("text_missing_record");
}

#[test]
#[serial_test::serial]
fn protected_startup_migration_state() {
    cc_switch_lib::integration_test_support::verify_credential_startup_case("migration_state");
}
