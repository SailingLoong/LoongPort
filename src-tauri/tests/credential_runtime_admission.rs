#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn protected_runtime_admission() {
    cc_switch_lib::integration_test_support::verify_native_generation_case("runtime_admission");
}
