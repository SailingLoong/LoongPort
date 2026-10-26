#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn cleanup_recreated() {
    cc_switch_lib::integration_test_support::verify_native_generation_case("cleanup_recreated");
}
#[test]
#[serial_test::serial]
fn cleanup_new_owned() {
    cc_switch_lib::integration_test_support::verify_native_generation_case("cleanup_new_owned");
}
#[test]
#[serial_test::serial]
fn cleanup_new_backup() {
    cc_switch_lib::integration_test_support::verify_native_generation_case("cleanup_new_backup");
}
