#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn headless_plaintext() {
    cc_switch_lib::integration_test_support::verify_imagegen_storage_case("plaintext");
}
#[test]
#[serial_test::serial]
fn headless_text() {
    cc_switch_lib::integration_test_support::verify_imagegen_storage_case("text");
}
#[test]
#[serial_test::serial]
fn headless_legacy() {
    cc_switch_lib::integration_test_support::verify_imagegen_storage_case("legacy");
}
#[test]
#[serial_test::serial]
fn headless_blocked() {
    cc_switch_lib::integration_test_support::verify_imagegen_storage_case("blocked");
}
