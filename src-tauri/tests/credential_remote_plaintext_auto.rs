#![cfg(all(feature = "gui", feature = "test-hooks"))]
#[test]
#[serial_test::serial]
fn remote_plain_webdav_auto_attempt() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_auto_attempt("webdav");
}
#[test]
#[serial_test::serial]
fn remote_plain_s3_auto_attempt() {
    cc_switch_lib::integration_test_support::verify_remote_plaintext_auto_attempt("s3");
}
