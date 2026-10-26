#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn original_encrypted_versioned_sync_join() {
    cc_switch_lib::Database::verify_versioned_sync_join();
}
