#![cfg(all(feature = "gui", feature = "test-hooks"))]
#[test]
#[serial_test::serial]
fn separate_configuration_plaintext() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("separate_plaintext");
}
#[test]
#[serial_test::serial]
fn separate_configuration_encrypted() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("separate_encrypted");
}
#[test]
#[serial_test::serial]
fn separate_configuration_cache_retry() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("separate_cache_retry");
}

#[test]
#[serial_test::serial]
fn separate_configuration_noncustody() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("separate_noncustody");
}

#[test]
#[serial_test::serial]
fn separate_configuration_dead_process() {
    cc_switch_lib::integration_test_support::verify_native_generation_case("separate_dead_process");
}

#[test]
#[serial_test::serial]
fn separate_configuration_future() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("separate_future");
}
