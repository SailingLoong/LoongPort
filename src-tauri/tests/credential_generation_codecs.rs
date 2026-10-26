#![cfg(feature="test-hooks")]
#[test]
#[serial_test::serial]
fn inventory_plaintext_membership() { cc_switch_lib::integration_test_support::verify_credential_transcode_case("inventory_plaintext_membership"); }
#[test]
#[serial_test::serial]
fn inventory_encrypted_strict() { cc_switch_lib::integration_test_support::verify_credential_transcode_case("inventory_encrypted_strict"); }
#[test]
#[serial_test::serial]
fn device_inventory_binary_guards() { cc_switch_lib::integration_test_support::verify_credential_transcode_case("device_inventory_binary_guards"); }
#[test]
#[serial_test::serial]
fn settings_transcode_roundtrip() { cc_switch_lib::integration_test_support::verify_credential_transcode_case("settings_transcode_roundtrip"); }
#[test]
#[serial_test::serial]
fn settings_transcode_strict_source() { cc_switch_lib::integration_test_support::verify_credential_transcode_case("settings_transcode_strict_source"); }
#[test]
#[serial_test::serial]
fn settings_transcode_generation() { cc_switch_lib::integration_test_support::verify_credential_transcode_case("settings_transcode_generation"); }
