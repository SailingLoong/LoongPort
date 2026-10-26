#![cfg(all(feature = "gui", feature = "test-hooks"))]
macro_rules! source_case {
    ($name:ident, $case:literal) => {
        #[test]
        #[serial_test::serial]
        fn $name() {
            cc_switch_lib::integration_test_support::verify_openrouter_source_fixture($case);
        }
    };
}
source_case!(original_rows_produce_safe_local_source_identity, "ready");
source_case!(
    only_exact_original_official_destination_is_eligible,
    "destination"
);
source_case!(
    original_row_credential_replacement_invalidates_evidence,
    "reseal"
);
source_case!(
    invalid_and_absent_original_rows_do_not_fabricate_identity,
    "invalid"
);
source_case!(
    unsupported_original_wire_format_is_not_guessed,
    "unsupported"
);

macro_rules! flow_case {
    ($name:ident, $case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_openrouter_shared_flow($case).await;
        }
    };
}
flow_case!(
    original_openrouter_rows_have_safe_shared_inventory,
    "source"
);
flow_case!(manual_openrouter_failure_never_borrows_next_row, "manual");
flow_case!(ordered_openrouter_503_uses_next_original_row, "ordered");
flow_case!(
    one_global_group_uses_origin_rows_for_two_apps_without_copying_keys,
    "two_apps"
);
flow_case!(
    cross_app_origin_health_never_mutates_same_id_target_row,
    "collision"
);
flow_case!(changed_original_key_rejects_stale_group_review, "stale");
flow_case!(
    missing_sealed_transport_never_reaches_official_network,
    "missing_cap"
);

/// Pure in-memory lifecycle invalidation only. This does not execute a restore,
/// import, backup, authorization recovery, or persistent-consent restoration.
#[test]
#[serial_test::serial]
fn database_replacement_epoch_retires_lease_without_unlocking() {
    cc_switch_lib::integration_test_support::verify_database_replacement_lease_epoch();
}

source_case!(
    original_owner_projection_is_transient_and_protocol_explicit,
    "projection"
);
source_case!(
    captured_original_row_rejects_reissued_credential,
    "expected_reseal"
);
source_case!(original_response_fence_detects_key_replacement, "fence_key");
source_case!(
    original_response_fence_detects_session_epoch_change,
    "fence_epoch"
);
source_case!(
    original_effective_format_precedence_is_preserved,
    "format_aliases"
);

flow_case!(
    ordered_local_empty_first_key_skips_to_authorized_original_b,
    "empty_first"
);
flow_case!(
    manual_local_empty_key_never_borrows_another_row,
    "manual_empty"
);
flow_case!(
    all_group_keys_empty_never_falls_back_to_healthy_legacy_b,
    "all_empty"
);
flow_case!(
    corrupt_original_row_is_not_skippable_empty_credential,
    "corrupt"
);
flow_case!(
    unknown_origin_row_is_not_replaced_by_same_id_target,
    "unknown"
);
flow_case!(
    captured_group_rejects_reissued_b_but_fresh_b_request_succeeds,
    "reseal_b"
);
flow_case!(
    visible_chat_output_never_replays_after_stream_failure,
    "stream_partial"
);
flow_case!(
    ordinary_session_epoch_retires_visible_output_but_new_request_succeeds,
    "stream_epoch"
);

source_case!(
    original_request_options_are_preserved_in_transient_projection,
    "request_options"
);

source_case!(
    original_pricing_overrides_survive_transient_projection,
    "pricing_overrides"
);
flow_case!(
    canonical_usage_pricing_uses_original_override_or_consumer_default,
    "pricing_reference"
);
flow_case!(
    legacy_limit_statistics_exclude_shared_history_without_erasing_it,
    "limits_reference"
);

flow_case!(
    shared_final_model_wins_over_local_body_override_without_losing_safe_options,
    "model_options"
);

/// Original final-body override seam, without HTTP, restores, or account operations.
#[test]
#[serial_test::serial]
fn shared_final_model_preserves_adapter_result_while_legacy_overrides_remain() {
    cc_switch_lib::integration_test_support::verify_shared_final_model_override();
}

flow_case!(
    ordinary_same_content_save_preserves_captured_group,
    "generic_save_b"
);
