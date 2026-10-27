#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]
#[allow(dead_code)]
#[path = "support.rs"]
mod support;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn original_minimax_account_and_two_app_projections_form_one_verified_source() {
    support::reset_test_fs();
    cc_switch_lib::integration_test_support::verify_minimax_shared_source(
        support::test_secret_session(),
        "source",
    )
    .await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn original_minimax_two_apps_share_ordered_native_protocol_fallback() {
    support::reset_test_fs();
    cc_switch_lib::integration_test_support::verify_minimax_shared_flow(
        support::test_secret_session(),
        "ordered",
    )
    .await;
}
#[cfg(feature = "test-hooks")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn original_minimax_manual_gateway_keeps_failure_on_selected_native_account() {
    support::reset_test_fs();
    cc_switch_lib::integration_test_support::verify_minimax_shared_flow(
        support::test_secret_session(),
        "manual",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn original_owner_block_and_unblock_does_not_revive_captured_source_facts() {
    support::reset_test_fs();
    cc_switch_lib::integration_test_support::verify_minimax_shared_source(
        support::test_secret_session(),
        "epoch",
    )
    .await;
}

#[cfg(feature = "test-hooks")]
macro_rules! native_case {
    ($name:ident,$case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            support::reset_test_fs();
            cc_switch_lib::integration_test_support::verify_minimax_shared_flow(
                support::test_secret_session(),
                $case,
            )
            .await;
        }
    };
}
#[cfg(feature = "test-hooks")]
native_case!(
    original_owner_uid_replacement_after_preview_rejects_group_commit,
    "preview_replace"
);
#[cfg(feature = "test-hooks")]
native_case!(
    original_owner_credential_revision_preserves_canonical_source_and_review,
    "refresh"
);
#[cfg(feature = "test-hooks")]
native_case!(
    captured_native_uid_replacement_before_fallback_cannot_use_new_grant,
    "replace"
);
#[cfg(feature = "test-hooks")]
native_case!(
    original_native_response_lease_fences_retired_account_bytes,
    "revoke"
);
#[cfg(feature = "test-hooks")]
native_case!(
    original_native_stream_commit_never_replays_on_second_account,
    "stream"
);
#[cfg(feature = "test-hooks")]
native_case!(
    pending_native_target_recovers_after_database_owner_reopen_and_epoch_change,
    "recovery"
);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn canonical_native_source_rejects_ambiguous_app_projections() {
    support::reset_test_fs();
    cc_switch_lib::integration_test_support::verify_minimax_shared_source(
        support::test_secret_session(),
        "duplicate",
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn changed_configured_native_model_cannot_resolve_old_group_source() {
    support::reset_test_fs();
    cc_switch_lib::integration_test_support::verify_minimax_shared_source(
        support::test_secret_session(),
        "model_changed",
    )
    .await;
}

#[cfg(feature = "test-hooks")]
native_case!(
    expired_native_access_in_ordered_group_never_calls_a_and_uses_b,
    "expired_ordered"
);
#[cfg(feature = "test-hooks")]
native_case!(
    expired_native_access_manual_target_never_calls_a_or_switches_to_b,
    "expired_manual"
);
