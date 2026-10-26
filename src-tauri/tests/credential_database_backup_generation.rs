#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn backup_older_encrypted_revision_same_key_is_authenticated_and_preserved() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_older_encrypted_revision_same_key_is_authenticated_and_preserved"); }
#[test]
#[serial_test::serial]
fn backup_plaintext_exact_identity_converts_to_next_generation() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_plaintext_exact_identity_converts_to_next_generation"); }
#[test]
#[serial_test::serial]
fn backup_rejects_other_store_key_and_future_revision() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_rejects_other_store_key_and_future_revision"); }
#[test]
#[serial_test::serial]
fn backup_plaintext_requires_exact_current_identity_and_mode() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_plaintext_requires_exact_current_identity_and_mode"); }
#[test]
#[serial_test::serial]
fn backup_does_not_relax_primary_generation_contract() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_does_not_relax_primary_generation_contract"); }
#[test]
#[serial_test::serial]
fn backup_corrupt_values_and_wrong_supplied_context_are_rejected() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_corrupt_values_and_wrong_supplied_context_are_rejected"); }
#[test]
#[serial_test::serial]
fn backup_inventory_keeps_relic_visible_and_rejects_it_without_publication() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_inventory_keeps_relic_visible_and_rejects_it_without_publication"); }
#[test]
#[serial_test::serial]
fn backup_persistent_pages_wal_and_journal_have_no_plaintext_canaries() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("backup_persistent_pages_wal_and_journal_have_no_plaintext_canaries"); }
#[test]
#[serial_test::serial]
fn primary_persistent_pages_wal_and_journal_have_no_plaintext_canaries() { cc_switch_lib::integration_test_support::verify_storage_backup_generation_case("primary_persistent_pages_wal_and_journal_have_no_plaintext_canaries"); }
