//! Synthetic original storage, credential writers, and journal consumers.
//! Not real-account credit consumption, user-data recovery, or protection-mode conversion.
#![cfg(all(feature = "gui", feature = "test-hooks"))]
macro_rules! storage_case {
    ($name:ident, $case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_shared_pool_storage($case).await;
        }
    };
}
storage_case!(
    plaintext_minimax_preserves_original_ordered_consumer,
    "minimax"
);
storage_case!(
    plaintext_workbuddy_preserves_original_ordered_consumer,
    "workbuddy"
);
storage_case!(
    plaintext_openrouter_keeps_original_cross_app_auth,
    "openrouter"
);
#[cfg(unix)]
storage_case!(plaintext_zcode_keeps_original_cross_app_auth, "zcode");
storage_case!(
    native_read_guard_compares_storage_mode_generation_and_revision,
    "identity"
);

// Expected security contract: same-key reissue and A->empty->A cannot revive
// either a captured routing request or an already acquired response fence.
storage_case!(
    plaintext_openrouter_request_reissue_is_retired,
    "openrouter_request_reissue"
);
storage_case!(
    plaintext_openrouter_response_reissue_is_retired,
    "openrouter_response_reissue"
);
storage_case!(
    plaintext_openrouter_request_aba_is_retired,
    "openrouter_request_aba"
);
storage_case!(
    plaintext_openrouter_response_aba_is_retired,
    "openrouter_response_aba"
);
storage_case!(
    plaintext_opencode_request_reissue_is_retired,
    "opencode_request_reissue"
);
storage_case!(
    plaintext_opencode_response_reissue_is_retired,
    "opencode_response_reissue"
);
storage_case!(
    plaintext_opencode_request_aba_is_retired,
    "opencode_request_aba"
);
storage_case!(
    plaintext_opencode_response_aba_is_retired,
    "opencode_response_aba"
);

storage_case!(credential_revision_schema_fresh, "revision_schema_fresh");

storage_case!(
    credential_revision_schema_migration,
    "revision_schema_migration"
);

storage_case!(credential_revision_schema_tamper, "revision_schema_tamper");

storage_case!(
    credential_revision_schema_legacy_prepare,
    "revision_schema_legacy_prepare"
);

storage_case!(
    credential_revision_provider_metadata,
    "revision_provider_metadata"
);

storage_case!(
    credential_revision_provider_recreate,
    "revision_provider_recreate"
);

storage_case!(
    credential_revision_provider_already_current,
    "revision_provider_already_current"
);

storage_case!(
    credential_revision_provider_proof,
    "revision_provider_proof"
);

storage_case!(
    credential_revision_provider_concurrent,
    "revision_provider_concurrent"
);

storage_case!(
    credential_revision_vendor_personal,
    "revision_vendor_personal"
);

storage_case!(
    credential_revision_vendor_metadata,
    "revision_vendor_metadata"
);

storage_case!(
    credential_revision_vendor_recreate,
    "revision_vendor_recreate"
);

storage_case!(credential_journal_replay, "journal_replay");

storage_case!(credential_journal_before_sql, "journal_before_sql");

storage_case!(credential_journal_after_sql, "journal_after_sql");

storage_case!(credential_journal_stale, "journal_stale");

storage_case!(
    plaintext_openrouter_label_preserves,
    "openrouter_label_preserves"
);

storage_case!(
    plaintext_openrouter_model_preserves,
    "openrouter_model_preserves"
);

storage_case!(plaintext_openrouter_recreate, "openrouter_recreate");

storage_case!(credential_review_vendor_binding, "journal_vendor_binding");

storage_case!(credential_review_cleanup_reissue, "journal_cleanup_reissue");

storage_case!(
    credential_recovery_cleanup_reissue,
    "journal_recover_cleanup_reissue"
);

storage_case!(
    credential_codex_recovery_cleanup_reissue,
    "journal_codex_recover_cleanup_reissue"
);

storage_case!(
    plaintext_credential_journal_replay,
    "journal_plaintext_replay"
);

storage_case!(
    plaintext_credential_journal_after_sql,
    "journal_plaintext_after_sql"
);

storage_case!(
    plaintext_credential_journal_before_sql,
    "journal_plaintext_before_sql"
);

storage_case!(
    plaintext_credential_journal_stale,
    "journal_plaintext_stale"
);

storage_case!(
    plaintext_credential_cleanup_reissue,
    "journal_plaintext_cleanup_reissue"
);

storage_case!(credential_journal_legacy_replay, "journal_legacy_replay");
storage_case!(
    plaintext_credential_journal_legacy_replay,
    "journal_plaintext_legacy_replay"
);
storage_case!(
    credential_revision_original_json_import,
    "revision_provider_json_import"
);

storage_case!(
    plaintext_sql_replacement_retires_original_leases,
    "openrouter_sql_replacement"
);
storage_case!(
    credential_legacy_update_cleanup_reissue,
    "journal_legacy_cleanup_reissue"
);
storage_case!(
    plaintext_credential_legacy_update_cleanup_reissue,
    "journal_plaintext_legacy_cleanup_reissue"
);

storage_case!(
    credential_revision_checkpoint_boundary,
    "revision_schema_checkpoint_boundary"
);
storage_case!(
    credential_revision_encrypted_legacy_prepare,
    "revision_schema_encrypted_legacy_prepare"
);

storage_case!(credential_prior_v18, "revision_prior_v18");

storage_case!(credential_prior_v22, "revision_prior_v22");

storage_case!(credential_prior_cn_v27, "revision_prior_cn_v27");

storage_case!(credential_prior_cn_v28, "revision_prior_cn_v28");

storage_case!(credential_prior_cn_v29, "revision_prior_cn_v29");

storage_case!(credential_prior_cn_rollback, "revision_prior_cn_rollback");

storage_case!(credential_prior_cn_missing, "revision_prior_cn_missing");

storage_case!(
    credential_prior_cn_interrupted,
    "revision_prior_cn_interrupted"
);

storage_case!(
    credential_prior_checkpoint_v24,
    "revision_prior_checkpoint_v24"
);

storage_case!(
    credential_prior_checkpoint_current,
    "revision_prior_checkpoint_current"
);

storage_case!(
    credential_prior_checkpoint_boundary,
    "revision_prior_checkpoint_boundary"
);

storage_case!(
    credential_prior_checkpoint_v29,
    "revision_prior_checkpoint_v29"
);
storage_case!(
    credential_prior_checkpoint_archive_v24,
    "revision_prior_checkpoint_archive_v24"
);
storage_case!(
    credential_revision_schema_missing_vendor,
    "revision_schema_missing_vendor"
);
storage_case!(
    credential_revision_schema_legacy_missing_vendor,
    "revision_schema_legacy_missing_vendor"
);
storage_case!(
    credential_revision_schema_encrypted_legacy_missing_vendor,
    "revision_schema_encrypted_legacy_missing_vendor"
);

storage_case!(credential_prior_legacy_v4, "revision_prior_v4");

storage_case!(credential_prior_legacy_v7, "revision_prior_v7");

storage_case!(credential_prior_legacy_v16, "revision_prior_v16");

storage_case!(credential_prior_legacy_v6, "revision_prior_v6");

storage_case!(
    credential_legacy_original_write_and_migration_boundary,
    "revision_legacy_write"
);
storage_case!(
    credential_legacy_latest_missing_column_is_rejected,
    "revision_legacy_latest_missing"
);
storage_case!(
    credential_legacy_mixed_schema_is_rejected,
    "revision_legacy_mixed"
);

storage_case!(
    credential_legacy_mixed_case_schema_is_rejected,
    "revision_legacy_mixed_case"
);
