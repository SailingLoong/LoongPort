#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn text_archive_manifest_text() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("manifest_text");
}
#[test]
#[serial_test::serial]
fn text_archive_exact_text() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("exact_text");
}
#[test]
#[serial_test::serial]
fn text_archive_no_legacy_fallback() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("no_legacy_fallback");
}
#[test]
#[serial_test::serial]
fn text_archive_legacy_password() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("legacy_password");
}
#[test]
#[serial_test::serial]
fn text_archive_invalid_metadata() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("invalid_metadata");
}
#[test]
#[serial_test::serial]
fn text_archive_build_package() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("build_package");
}
#[test]
#[serial_test::serial]
fn text_archive_installer_fence() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("installer_fence");
}
#[test]
#[serial_test::serial]
fn text_archive_plaintext_explicit() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("plaintext_explicit");
}

#[test]
#[serial_test::serial]
fn text_archive_backup_restore_same_generation() {
    cc_switch_lib::integration_test_support::verify_text_archive_case(
        "backup_restore_same_generation",
    );
}

#[test]
#[serial_test::serial]
fn text_archive_backup_text_preflight() {
    cc_switch_lib::integration_test_support::verify_text_archive_case("backup_text_preflight");
}
