#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn local_sql_plain_restore() {
    cc_switch_lib::integration_test_support::verify_local_sql_restore_case("plain_restore");
}
#[test]
#[serial_test::serial]
fn local_sql_text_restore() {
    cc_switch_lib::integration_test_support::verify_local_sql_restore_case("text_restore");
}
#[test]
#[serial_test::serial]
fn local_sql_wrong_input() {
    cc_switch_lib::integration_test_support::verify_local_sql_restore_case("wrong_input");
}
#[test]
#[serial_test::serial]
fn local_sql_invalid_sql() {
    cc_switch_lib::integration_test_support::verify_local_sql_restore_case("invalid_sql");
}
