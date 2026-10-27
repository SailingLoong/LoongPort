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
    paused_workbuddy_configuration_is_visible_without_ranking_or_source_proof,
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
    healthy_minimax_retired_after_review_prevents_group_commit,
    "preview_revoke"
);
case!(
    paused_workbuddy_does_not_revive_after_disable_and_enable,
    "revive"
);
case!(
    paused_workbuddy_does_not_revive_after_replacement_grant,
    "replace"
);
case!(
    paused_workbuddy_never_sends_to_channel_denial_fixture,
    "denied11004"
);
case!(
    paused_workbuddy_never_sends_to_account_restriction_fixture,
    "denied11140"
);
case!(
    nonstream_original_workbuddy_rejection_never_posts_or_falls_back,
    "nonstream"
);
case!(
    paused_stream_never_opens_or_replays_to_other_native_owner,
    "stream"
);
case!(
    paused_workbuddy_never_produces_buffered_response_bytes,
    "revoke"
);
case!(
    paused_model_route_does_not_refresh_expired_original_grant,
    "refresh"
);

case!(
    automatic_group_skips_workbuddy_local_pre_request_revocation,
    "wb_skip"
);
case!(
    automatic_group_with_revoked_minimax_and_paused_workbuddy_has_no_source,
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

case!(workbuddy_paused_source, "paused_source");

case!(workbuddy_paused_preview, "paused_preview");

case!(workbuddy_paused_group, "paused_group");

case!(workbuddy_paused_manual, "paused_manual");

case!(all_paused_group_has_no_outside_fallback, "all_paused");
case!(healthy_minimax_manual_route_remains_available, "mm_manual");

case!(
    workbuddy_paused_selection_ipc_is_zero_write,
    "paused_select"
);

case!(
    old_paused_workbuddy_current_survives_real_source_import,
    "paused_import"
);

case!(
    workbuddy_paused_legacy_selection_and_model_order_are_zero_write,
    "paused_legacy_select"
);
