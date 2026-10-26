#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn postflight_empty_neutral() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("empty_neutral");
}
#[test]
#[serial_test::serial]
fn postflight_owned() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("owned");
}
#[test]
#[serial_test::serial]
fn postflight_device() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("device");
}
#[test]
#[serial_test::serial]
fn postflight_settings() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("settings");
}
#[test]
#[serial_test::serial]
fn postflight_staging_owned() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("staging_owned");
}
#[test]
#[serial_test::serial]
fn postflight_staging_device() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("staging_device");
}
#[test]
#[serial_test::serial]
fn postflight_staging_device_separate() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("staging_device_separate");
}
#[test]
#[serial_test::serial]
fn postflight_staging_settings() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("staging_settings");
}
#[test]
#[serial_test::serial]
fn postflight_neutral_tmp() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("neutral_tmp");
}
#[test]
#[serial_test::serial]
fn postflight_orphan_sidecars() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("orphan_sidecars");
}
#[test]
#[serial_test::serial]
fn postflight_recovery_artifacts() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("recovery_artifacts");
}
#[test]
#[serial_test::serial]
fn postflight_path_aliases() {
    cc_switch_lib::integration_test_support::verify_file_postflight_case("path_aliases");
}
