#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]

//! Synthetic upstream boundary; this is not real-account inference acceptance.
#[tokio::test]
async fn one_login_retains_one_complete_grant_with_separate_purpose_consent() {
    cc_switch_lib::integration_test_support::verify_workbuddy_shared_authorization("one_login")
        .await;
}
#[tokio::test]
async fn incomplete_login_cannot_fabricate_shared_credentials() {
    cc_switch_lib::integration_test_support::verify_workbuddy_shared_authorization(
        "incomplete_login",
    )
    .await;
}

macro_rules! fixture_case {
    ($name:ident, $case:expr) => {
        #[tokio::test]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_workbuddy_shared_authorization($case)
                .await;
        }
    };
}
fixture_case!(
    inference_revocation_preserves_claim_permission_and_credentials,
    "consent"
);
fixture_case!(
    concurrent_account_reads_share_one_refresh_owner_and_cannot_dispatch_models,
    "refresh"
);
fixture_case!(
    purpose_consent_does_not_replay_an_unresolved_claim,
    "pending"
);
fixture_case!(
    legacy_access_only_account_is_not_moved_cross_domain,
    "legacy"
);
fixture_case!(
    checkin_only_refusal_does_not_invalidate_account_login,
    "checkin_refusal"
);

fixture_case!(final_claim_fence_reports_definitely_unsent, "final_fence");
fixture_case!(
    private_pending_browser_cannot_undo_explicit_inference_revocation,
    "late_revoke"
);
fixture_case!(
    private_supplemental_authorization_cannot_replace_another_identity,
    "wrong_identity"
);

fixture_case!(shared_grant_claim_repeated_click_never_replays, "claim");

fixture_case!(
    legacy_schema_and_same_owner_encrypted_restore_preserve_fences,
    "schema"
);

fixture_case!(workbuddy_paused_lease, "paused_lease");

fixture_case!(workbuddy_paused_consent, "paused_consent");

fixture_case!(workbuddy_paused_login, "paused_login");

fixture_case!(workbuddy_paused_state, "paused_state");

fixture_case!(workbuddy_paused_final_dispatch, "paused_final_dispatch");

fixture_case!(workbuddy_paused_refresh, "paused_refresh");

fixture_case!(workbuddy_paused_claim, "paused_claim");

fixture_case!(
    workbuddy_paused_models_before_refresh,
    "paused_models_before_refresh"
);
fixture_case!(workbuddy_paused_acp, "paused_acp");

fixture_case!(codebuddy_paused_lease, "codebuddy_paused_lease");

fixture_case!(codebuddy_paused_consent, "codebuddy_paused_consent");

fixture_case!(codebuddy_paused_login, "codebuddy_paused_login");

fixture_case!(codebuddy_paused_state, "codebuddy_paused_state");

fixture_case!(
    codebuddy_paused_final_dispatch,
    "codebuddy_paused_final_dispatch"
);

fixture_case!(
    codebuddy_paused_models_before_refresh,
    "codebuddy_paused_models_before_refresh"
);

fixture_case!(codebuddy_paused_claim, "codebuddy_paused_claim");

fixture_case!(codebuddy_paused_refresh, "codebuddy_paused_refresh");

#[cfg(feature = "test-hooks")]
macro_rules! public_case {
    ($name:ident,$case:literal) => {
        #[tokio::test]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_workbuddy_public_pause($case).await;
        }
    };
}
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_1,
    "workbuddy_all_account_auto_pool_is_paused_without_retry"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_2,
    "workbuddy_manual_selection_never_switches_account"
);
#[cfg(feature = "test-hooks")]
public_case!(public_pause_3, "workbuddy_nonstream_is_paused_without_post");
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_4,
    "workbuddy_paused_response_contains_no_buffered_upstream_bytes"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_5,
    "workbuddy_repeated_requests_never_open_stream_or_change_accounts"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_6,
    "workbuddy_claude_tools_input_is_paused_before_conversion_dispatch"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_7,
    "workbuddy_missing_or_foreign_fixture_has_zero_http"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_8,
    "workbuddy_codex_responses_tools_input_is_paused_before_dispatch"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_9,
    "workbuddy_empty_input_is_never_model_permission"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_10,
    "workbuddy_all_three_public_entries_pause_stream_and_nonstream"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_11,
    "workbuddy_pause_does_not_rewrite_provider_or_priority"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_12,
    "workbuddy_free_only_stays_terminal_without_any_model_request"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_13,
    "workbuddy_replaced_grant_cannot_resurrect_paused_route"
);
#[cfg(feature = "test-hooks")]
public_case!(
    public_pause_14,
    "workbuddy_retired_first_owner_does_not_enable_second_paused_owner"
);
#[cfg(feature = "test-hooks")]
public_case!(public_pause_protocol, "protocol");

#[cfg(feature = "test-hooks")]
macro_rules! private_case {
    ($name:ident,$case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_workbuddy_private_protocol($case).await;
        }
    };
}
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_two_selected_accounts_keep_distinct_new_grants_and_preserve_body,
    "two_selected_accounts_keep_distinct_new_grants_and_preserve_body"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_legacy_rows_are_not_inference_authorized_and_keep_existing_ui_dto,
    "legacy_rows_are_not_inference_authorized_and_keep_existing_ui_dto"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_explicit_new_grant_binds_only_matching_original_row_and_keeps_checkin,
    "explicit_new_grant_binds_only_matching_original_row_and_keeps_checkin"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_wrong_uid_or_domain_or_platform_never_binds_or_migrates,
    "wrong_uid_or_domain_or_platform_never_binds_or_migrates"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_stale_login_flow_cannot_replace_a_newer_grant,
    "stale_login_flow_cannot_replace_a_newer_grant"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_unsafe_authorization_urls_are_rejected_without_following,
    "unsafe_authorization_urls_are_rejected_without_following"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_models_use_only_live_cli_ids_without_static_fallback_or_pricing,
    "models_use_only_live_cli_ids_without_static_fallback_or_pricing"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_explicit_401_refreshes_same_grant_once_and_never_sends_old_checkin_token,
    "explicit_401_refreshes_same_grant_once_and_never_sends_old_checkin_token"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_expiry_refresh_uses_the_only_replay_budget_and_preserves_second_401,
    "expiry_refresh_uses_the_only_replay_budget_and_preserves_second_401"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_concurrent_refreshes_spend_a_refresh_token_once,
    "concurrent_refreshes_spend_a_refresh_token_once"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_refresh_identity_mismatch_is_rejected_without_overwriting,
    "refresh_identity_mismatch_is_rejected_without_overwriting"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_refusals_are_terminal_structured_and_never_rewrite_replay_or_rotate,
    "refusals_are_terminal_structured_and_never_rewrite_replay_or_rotate"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_status_429_and_retry_after_are_preserved_but_secret_headers_are_removed,
    "status_429_and_retry_after_are_preserved_but_secret_headers_are_removed"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_success_headers_are_allowlisted_and_sse_bytes_unchanged,
    "success_headers_are_allowlisted_and_sse_bytes_unchanged"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_final_fence_rejects_previously_primed_or_buffered_bytes_after_retirement,
    "final_fence_rejects_previously_primed_or_buffered_bytes_after_retirement"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_blocked_then_unblocked_session_never_revives_old_lease,
    "blocked_then_unblocked_session_never_revives_old_lease"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_post_output_stream_failure_is_sanitized_and_never_replayed,
    "post_output_stream_failure_is_sanitized_and_never_replayed"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_nonstream_request_is_rejected_before_network_instead_of_silently_rewritten,
    "nonstream_request_is_rejected_before_network_instead_of_silently_rewritten"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_response_echo_across_chunks_is_withheld_then_terminated_without_replay,
    "response_echo_across_chunks_is_withheld_then_terminated_without_replay"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_declared_compression_is_rejected_and_requests_ask_identity,
    "declared_compression_is_rejected_and_requests_ask_identity"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_fixture_origin_and_owner_mismatch_never_fall_back_to_production,
    "fixture_origin_and_owner_mismatch_never_fall_back_to_production"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_every_documented_denial_envelope_blocks_401_refresh_and_preserves_exact_codes,
    "every_documented_denial_envelope_blocks_401_refresh_and_preserves_exact_codes"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_pending_refresh_is_single_flight_and_second_lease_reads_persisted_rotation,
    "pending_refresh_is_single_flight_and_second_lease_reads_persisted_rotation"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_late_refresh_response_cannot_revive_retired_original_grant,
    "late_refresh_response_cannot_revive_retired_original_grant"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_waiting_token_response_after_session_epoch_change_is_not_published,
    "waiting_token_response_after_session_epoch_change_is_not_published"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_incomplete_error_body_has_a_bounded_terminal_deadline_without_refresh,
    "incomplete_error_body_has_a_bounded_terminal_deadline_without_refresh"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_cancelled_pending_profile_late_response_cannot_publish_grant,
    "cancelled_pending_profile_late_response_cannot_publish_grant"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_closing_old_flow_cannot_cancel_reopened_flow_or_other_original_account,
    "closing_old_flow_cannot_cancel_reopened_flow_or_other_original_account"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_cancel_publish_race_linearizes_and_cancel_never_revokes_published_grant,
    "cancel_publish_race_linearizes_and_cancel_never_revokes_published_grant"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_inference_status_uses_only_original_private_grant_and_safe_dto,
    "inference_status_uses_only_original_private_grant_and_safe_dto"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_discovery_rejects_secret_echo_in_decoded_model_id_or_label_and_nonidentity,
    "discovery_rejects_secret_echo_in_decoded_model_id_or_label_and_nonidentity"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_decoded_unicode_first_event_sets_terminal_opaque_fence_without_output,
    "decoded_unicode_first_event_sets_terminal_opaque_fence_without_output"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_decoded_secret_split_across_delta_events_holds_original_prefix,
    "decoded_secret_split_across_delta_events_holds_original_prefix"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_reasoning_aliases_and_original_trimmed_tool_reasoning_are_screened,
    "reasoning_aliases_and_original_trimmed_tool_reasoning_are_screened"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_nested_unicode_tool_arguments_are_held_until_complete_and_screened,
    "nested_unicode_tool_arguments_are_held_until_complete_and_screened"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_independent_tools_and_content_fields_never_join_into_false_secret,
    "independent_tools_and_content_fields_never_join_into_false_secret"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_semantic_screen_preserves_normal_multibyte_and_line_endings_byte_for_byte,
    "semantic_screen_preserves_normal_multibyte_and_line_endings_byte_for_byte"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_single_choice_capability_rejects_n_before_network_and_other_choice_on_response,
    "single_choice_capability_rejects_n_before_network_and_other_choice_on_response"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_event_and_total_holdback_limits_fail_closed_with_terminal_fence,
    "event_and_total_holdback_limits_fail_closed_with_terminal_fence"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_malformed_sse_or_incomplete_tool_json_never_becomes_successful_eof,
    "malformed_sse_or_incomplete_tool_json_never_becomes_successful_eof"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_clean_eof_without_a_terminal_signal_is_not_success,
    "clean_eof_without_a_terminal_signal_is_not_success"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_explicit_terminal_signals_preserve_clean_eof_wire_bytes,
    "explicit_terminal_signals_preserve_clean_eof_wire_bytes"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_finish_reason_only_keeps_usage_tail_and_completed_tools,
    "finish_reason_only_keeps_usage_tail_and_completed_tools"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_split_leading_think_and_reasoning_alias_follow_original_late_parse_order,
    "split_leading_think_and_reasoning_alias_follow_original_late_parse_order"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_completed_tool_rejects_late_argument_fragments_without_reinterpretation,
    "completed_tool_rejects_late_argument_fragments_without_reinterpretation"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_structured_sse_denials_preserve_safe_terminal_meaning_before_and_after_output,
    "structured_sse_denials_preserve_safe_terminal_meaning_before_and_after_output"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_structured_sse_denial_detection_never_matches_messages_or_code_prefixes,
    "structured_sse_denial_detection_never_matches_messages_or_code_prefixes"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_structured_sse_typed_fence_keeps_retirement_terminal_not_auth_retry,
    "structured_sse_typed_fence_keeps_retirement_terminal_not_auth_retry"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_workbuddy_finish_reason_rejects_late_content_reasoning_role_or_new_tool,
    "workbuddy_finish_reason_rejects_late_content_reasoning_role_or_new_tool"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_workbuddy_finish_reason_rejects_general_error_frames,
    "workbuddy_finish_reason_rejects_general_error_frames"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_workbuddy_done_is_withheld_until_the_following_tail_is_checked,
    "workbuddy_done_is_withheld_until_the_following_tail_is_checked"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_workbuddy_finished_tools_usage_and_done_keep_their_original_wire_order,
    "workbuddy_finished_tools_usage_and_done_keep_their_original_wire_order"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_expiry_owner_model_discovery_and_meter_join_require_same_credential_generation,
    "expiry_owner_model_discovery_and_meter_join_require_same_credential_generation"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_expiry_review_future_model_observation_cannot_hide_behind_older_credits,
    "expiry_review_future_model_observation_cannot_hide_behind_older_credits"
);
#[cfg(feature = "test-hooks")]
private_case!(
    protocol_regression_expiry_review_new_meter_generation_does_not_relabel_old_model_snapshot,
    "expiry_review_new_meter_generation_does_not_relabel_old_model_snapshot"
);
