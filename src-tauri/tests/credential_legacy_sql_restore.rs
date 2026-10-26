#![cfg(feature = "test-hooks")]

#[test]
#[serial_test::serial]
fn explicit_legacy_plaintext_file_restore_encrypts_the_imported_credentials() {
    cc_switch_lib::integration_test_support::verify_explicit_legacy_plaintext_file_restore();
}

#[test]
#[serial_test::serial]
fn modern_unmarked_plaintext_backup_is_rejected_without_writes() {
    cc_switch_lib::integration_test_support::verify_modern_unmarked_plaintext_backup();
}
