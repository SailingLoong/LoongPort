#![cfg(all(feature = "gui", feature = "test-hooks"))]
macro_rules! source_case {
    ($name:ident, $case:literal) => {
        #[test]
        #[serial_test::serial]
        fn $name() {
            cc_switch_lib::integration_test_support::verify_qoder_shared_source($case);
        }
    };
}
source_case!(original_ready_owner_projects_safe_source, "ready");
source_case!(identity_and_realm_define_canonical_account, "identity");
source_case!(normal_refresh_preserves_stable_source, "refresh");
source_case!(unready_owner_never_fabricates_source, "unready");
source_case!(corrupt_owner_is_not_silently_absent, "corrupt");
source_case!(identity_incarnation_changes_stable_evidence, "incarnation");
source_case!(
    route_identity_fenced_before_refresh_reservation,
    "route_fence"
);
macro_rules! flow_case {
    ($name:ident, $case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_qoder_shared_flow($case).await;
        }
    };
}
flow_case!(original_qoder_source_is_safe_and_codex_only, "source");
flow_case!(manual_shared_gateway_uses_original_qoder_worker, "manual");
flow_case!(ordered_shared_gateway_uses_original_qoder_worker, "ordered");
flow_case!(china_device_shared_gateway_uses_original_qoder_worker, "cn");

source_case!(captured_epoch_fenced_before_refresh_reservation, "epoch");

flow_case!(
    shared_group_preview_cas_gateway_binding_uses_original_worker,
    "review"
);
flow_case!(shared_group_tools_roundtrip_uses_original_worker, "tools");
flow_case!(
    same_global_group_resolves_qoder_codex_and_minimax_claude,
    "mixed"
);
flow_case!(
    bound_mixed_group_review_preserves_per_app_capability,
    "mixed_review"
);
flow_case!(bad_minimax_catalog_does_not_hide_verified_qoder, "bad_mm");
flow_case!(bad_workbuddy_catalog_does_not_hide_verified_qoder, "bad_wb");

flow_case!(
    qoder_only_group_cas_rejects_transient_epoch_revival,
    "group_epoch"
);
flow_case!(
    qoder_only_target_cas_rejects_epoch_and_pending_recovers,
    "target_epoch"
);

flow_case!(bad_qoder_row_does_not_hide_verified_minimax, "bad_qoder");

source_case!(
    pending_identity_has_capability_but_no_send_lease,
    "pending_identity"
);
source_case!(
    expired_identity_has_capability_but_no_send_lease,
    "expired_identity"
);
flow_case!(
    pending_qoder_identity_preserves_claude_capability_and_codex_auto_skip,
    "pending_auto"
);
flow_case!(
    pending_qoder_manual_never_replays_or_refreshes,
    "pending_manual"
);

flow_case!(
    bad_qoder_row_does_not_block_unrelated_minimax_runtime,
    "bad_qoder_runtime"
);
flow_case!(
    shared_qoder_unknown_submission_never_replays,
    "submitted_unknown"
);
flow_case!(
    shared_qoder_partial_output_never_completes_or_replays,
    "submitted_partial"
);
flow_case!(
    unsupported_qoder_manual_cannot_borrow_other_group_member,
    "unsupported_manual"
);
flow_case!(
    unknown_qoder_source_is_not_skippable_capability,
    "unknown_member"
);
flow_case!(
    qoder_model_conflict_is_not_skippable_capability,
    "conflicting_member_model"
);
flow_case!(
    all_members_unsupported_for_application_never_fall_outside_group,
    "all_unsupported"
);

flow_case!(
    new_group_cannot_count_unsupported_app_as_ready_source,
    "pending_new_group"
);

flow_case!(
    shared_reviewed_model_survives_late_override_through_original_worker,
    "model_options"
);
