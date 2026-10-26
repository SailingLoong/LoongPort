//! Same original DB/DAO owner cases through the established synthetic test-hooks boundary.
#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn explicit_plaintext_database_reopens_without_vault_metadata() { cc_switch_lib::integration_test_support::verify_credential_storage_case("explicit_plaintext_database_reopens_without_vault_metadata").unwrap(); }
#[test]
#[serial_test::serial]
fn missing_plaintext_database_identity_is_not_legacy_adoption() { cc_switch_lib::integration_test_support::verify_credential_storage_case("missing_plaintext_database_identity_is_not_legacy_adoption").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_generation_mismatch_preserves_original_database() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_generation_mismatch_preserves_original_database").unwrap(); }
#[test]
#[serial_test::serial]
fn encrypted_database_cannot_be_opened_as_plaintext() { cc_switch_lib::integration_test_support::verify_credential_storage_case("encrypted_database_cannot_be_opened_as_plaintext").unwrap(); }
#[test]
#[serial_test::serial]
fn mixed_envelopes_reject_plaintext_reopen_without_writing() { cc_switch_lib::integration_test_support::verify_credential_storage_case("mixed_envelopes_reject_plaintext_reopen_without_writing").unwrap(); }
#[test]
#[serial_test::serial]
fn explicit_plaintext_staging_preserves_legacy_values_and_identity() { cc_switch_lib::integration_test_support::verify_credential_storage_case("explicit_plaintext_staging_preserves_legacy_values_and_identity").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_staging_rejection_rolls_back_schema_and_values() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_staging_rejection_rolls_back_schema_and_values").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_provider_and_setting_daos_preserve_original_sql_contracts() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_provider_and_setting_daos_preserve_original_sql_contracts").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_session_block_still_denies_secret_and_nonsecret_operations() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_session_block_still_denies_secret_and_nonsecret_operations").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_backup_keeps_mode_identity_and_raw_values() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_backup_keeps_mode_identity_and_raw_values").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_sql_export_reuses_original_dump_without_recovery_password() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_sql_export_reuses_original_dump_without_recovery_password").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_restore_reuses_original_backup_owner_and_preserves_new_safety_copy() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_restore_reuses_original_backup_owner_and_preserves_new_safety_copy").unwrap(); }
#[test]
#[serial_test::serial]
fn encryption_stamp_does_not_overwrite_explicit_plaintext_identity() { cc_switch_lib::integration_test_support::verify_credential_storage_case("encryption_stamp_does_not_overwrite_explicit_plaintext_identity").unwrap(); }
#[test]
#[serial_test::serial]
fn encryption_staging_does_not_implicitly_convert_explicit_plaintext() { cc_switch_lib::integration_test_support::verify_credential_storage_case("encryption_staging_does_not_implicitly_convert_explicit_plaintext").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_setup_does_not_adopt_authenticated_schema_with_missing_metadata() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_setup_does_not_adopt_authenticated_schema_with_missing_metadata").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_provider_guard_still_rejects_another_session() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_provider_guard_still_rejects_another_session").unwrap(); }
#[test]
#[serial_test::serial]
fn plaintext_qoder_lease_cannot_publish_after_session_epoch_changes() { cc_switch_lib::integration_test_support::verify_credential_storage_case("plaintext_qoder_lease_cannot_publish_after_session_epoch_changes").unwrap(); }
