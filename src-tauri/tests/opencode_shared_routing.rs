#![cfg(all(feature = "gui", feature = "test-hooks"))]
macro_rules! case {
    ($name:ident, $case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_opencode_shared_flow($case).await;
        }
    };
}
case!(
    workspace_original_protocol_after_group_publication,
    "workspace"
);
case!(
    workspace_ordered_503_uses_next_original_account,
    "workspace_ordered"
);
case!(
    workspace_manual_failure_never_changes_account,
    "workspace_manual_failure"
);
case!(
    personal_go_deepseek_one_global_group_two_original_protocols,
    "personal_two_apps"
);
case!(
    personal_go_ordered_503_preserves_session,
    "personal_ordered"
);
case!(
    personal_go_manual_failure_never_changes_account,
    "personal_manual_failure"
);
case!(
    personal_go_requires_original_conversation_session,
    "personal_missing_session"
);
case!(
    personal_go_replaced_session_epoch_fails_closed,
    "personal_epoch"
);

case!(
    original_key_replacement_invalidates_group_review,
    "personal_stale_group"
);
case!(
    original_key_replacement_invalidates_target_review,
    "personal_stale_target"
);
case!(
    manual_revoked_key_never_borrows_second_account,
    "personal_key_revoked"
);
case!(
    original_web_logout_preserves_independent_inference_key,
    "personal_web_logout"
);

case!(
    personal_go_committed_stream_never_replays,
    "personal_stream"
);
case!(
    personal_go_committed_stream_epoch_fence_never_replays,
    "personal_stream_epoch"
);

case!(
    personal_glm_union_catalog_is_not_messages_capability,
    "personal_glm_messages_unsupported"
);
case!(
    personal_unknown_model_cannot_be_published,
    "personal_unknown_model"
);
case!(
    personal_removed_catalog_model_invalidates_review_and_runtime,
    "personal_catalog_removed"
);

case!(
    captured_group_cannot_adopt_reissued_b_key_but_new_request_can,
    "personal_snapshot_reseal"
);

case!(
    saved_group_with_empty_later_key_keeps_healthy_first_account,
    "personal_empty_later"
);

case!(
    original_workspace_without_shared_binding_keeps_legacy_fence_path,
    "workspace_legacy"
);

case!(
    saved_group_skips_locally_empty_first_key_to_authorized_second,
    "personal_empty_first"
);

case!(
    empty_group_never_falls_back_to_healthy_legacy_account,
    "personal_all_empty"
);
case!(
    corrupt_first_credential_is_not_skippable_empty_state,
    "personal_corrupt_first"
);
case!(
    unknown_first_account_is_not_skippable_empty_state,
    "personal_unknown_first"
);

case!(external_policy_free_false, "policy_free_false");

case!(
    external_policy_free_paid_key_route,
    "policy_free_paid_key_route"
);

case!(external_policy_fallback_free, "policy_fallback_free");

case!(external_policy_override_free, "policy_override_free");

case!(external_policy_unknown, "policy_unknown");

case!(
    external_policy_paid_go_preserved,
    "policy_paid_go_preserved"
);

case!(
    external_policy_other_platforms_preserved,
    "policy_other_platforms_preserved"
);

case!(
    external_consumer_runtime_free_fallback,
    "runtime_free_fallback"
);

case!(
    external_consumer_runtime_unknown_override,
    "runtime_unknown_override"
);

case!(
    external_consumer_personal_runtime_free_fallback,
    "personal_runtime_free_fallback"
);

case!(external_consumer_free_config, "free_config");

case!(
    external_consumer_free_legacy_inventory,
    "free_legacy_inventory"
);

case!(external_policy_go_beam_exact, "policy_go_beam_exact");

case!(external_policy_current_catalog, "policy_current_catalog");

case!(
    external_policy_channel_normalization,
    "policy_channel_normalization"
);

case!(
    external_consumer_personal_runtime_beam_go,
    "personal_runtime_beam_go"
);

case!(external_policy_sticky_rejection, "policy_sticky_rejection");
case!(
    external_consumer_group_free_no_paid_fallback,
    "personal_policy_group_free"
);
case!(
    external_consumer_group_unknown_no_paid_fallback,
    "personal_policy_group_unknown"
);
