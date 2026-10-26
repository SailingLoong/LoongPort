#![cfg(feature = "test-hooks")]

#[test]
#[serial_test::serial]
fn aggregate_export_plain() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case("plain");
}
#[test]
#[serial_test::serial]
fn aggregate_export_text() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case("text");
}
#[test]
#[serial_test::serial]
fn aggregate_export_legacy() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case("legacy");
}
#[test]
#[serial_test::serial]
fn aggregate_export_blocked() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case("blocked");
}
#[test]
#[serial_test::serial]
fn aggregate_export_plain_db_cipher() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "plain_db_cipher",
    );
}
#[test]
#[serial_test::serial]
fn aggregate_export_plain_auth_cipher() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "plain_auth_cipher",
    );
}
#[test]
#[serial_test::serial]
fn aggregate_export_qoder_cipher() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case("qoder_cipher");
}
#[test]
#[serial_test::serial]
fn aggregate_export_invalid_encrypted_auth() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "invalid_encrypted_auth",
    );
}

#[test]
#[serial_test::serial]
fn aggregate_export_foreign_plain_db() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "foreign_plain_db",
    );
}

#[test]
#[serial_test::serial]
fn aggregate_export_missing_plain_record() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "missing_plain_record",
    );
}

#[test]
#[serial_test::serial]
fn aggregate_export_missing_text_record() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "missing_text_record",
    );
}

#[test]
#[serial_test::serial]
fn aggregate_export_cleanup_pending() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case(
        "cleanup_pending",
    );
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn aggregate_export_auth_alias() {
    cc_switch_lib::integration_test_support::verify_aggregate_storage_export_case("auth_alias");
}
