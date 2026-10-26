#![cfg(all(feature = "gui", feature = "test-hooks"))]
#[allow(dead_code)]
#[path = "support.rs"]
mod support;
macro_rules! case {
    ($name:ident,$case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            support::reset_test_fs();
            cc_switch_lib::integration_test_support::verify_workbuddy_shared_flow(
                support::test_secret_session(),
                $case,
            )
            .await;
        }
    };
}
case!(
    configured_original_workbuddy_account_needs_no_optional_ranking_snapshot,
    "source"
);
case!(
    two_apps_share_mixed_native_group_through_original_http_consumers,
    "mixed"
);
case!(
    manual_workbuddy_gateway_never_falls_back_to_other_native_owner,
    "manual"
);

case!(
    configured_model_mismatch_is_not_invented_from_ranking,
    "unconfigured_model"
);
case!(
    inference_consent_revoked_after_review_prevents_group_commit,
    "preview_revoke"
);
case!(
    captured_consent_does_not_revive_after_disable_and_enable,
    "revive"
);
case!(
    captured_original_grant_cannot_switch_to_replacement_grant,
    "replace"
);
case!(
    explicit_channel_denial_11004_never_falls_back_across_native_accounts,
    "denied11004"
);
case!(
    explicit_account_restriction_11140_never_falls_back_across_native_accounts,
    "denied11140"
);
case!(
    nonstream_original_workbuddy_rejection_never_posts_or_falls_back,
    "nonstream"
);
case!(
    committed_workbuddy_stream_never_replays_to_other_native_owner,
    "stream"
);
case!(
    retired_workbuddy_owner_fences_all_buffered_response_bytes,
    "revoke"
);
case!(
    original_oauth_refresh_keeps_next_request_canonical_without_ranking_or_discovery,
    "refresh"
);

case!(
    automatic_group_skips_workbuddy_local_pre_request_revocation,
    "wb_skip"
);
case!(
    automatic_group_skips_minimax_local_pre_request_revocation,
    "mm_skip"
);
case!(
    manual_workbuddy_local_revocation_does_not_select_other_member,
    "wb_manual_revoked"
);
case!(
    manual_minimax_local_revocation_does_not_select_other_member,
    "mm_manual_revoked"
);
case!(
    all_locally_revoked_native_members_fail_without_outside_fallback,
    "all_revoked"
);
case!(
    runtime_model_conflict_is_not_treated_as_skippable_local_unavailability,
    "runtime_model_changed"
);
