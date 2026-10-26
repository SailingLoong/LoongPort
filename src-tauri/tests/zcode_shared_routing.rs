#![cfg(all(feature = "gui", feature = "test-hooks", unix))]

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn original_zcode_account_has_one_safe_source_for_existing_app_projections() {
    cc_switch_lib::integration_test_support::verify_zcode_shared_flow("source").await;
}

macro_rules! flow_case {
    ($name:ident,$case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_zcode_shared_flow($case).await;
        }
    };
}
flow_case!(
    manual_zcode_failure_does_not_switch_original_account,
    "manual"
);
flow_case!(ordered_zcode_a_503_uses_original_b_once, "ordered");
flow_case!(
    one_zcode_group_serves_two_existing_app_projections,
    "two_apps"
);

macro_rules! owner_case {
    ($name:ident,$case:literal) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial_test::serial]
        async fn $name() {
            cc_switch_lib::integration_test_support::verify_zcode_routing_owner($case).await;
        }
    };
}
owner_case!(
    original_zcode_routing_facts_are_read_only,
    "facts_read_only"
);
owner_case!(
    captured_zcode_stamp_is_checked_by_original_lease,
    "stamp_mismatch"
);
owner_case!(
    zcode_synthetic_routing_capability_is_scoped_and_not_implicit,
    "capability_missing_drop"
);
owner_case!(
    malformed_saved_zcode_credential_is_not_typed_as_missing,
    "credential_format"
);
owner_case!(
    expired_zcode_routing_facts_are_read_only_and_account_scoped,
    "facts_expired_read_only"
);

flow_case!(
    legacy_zcode_gateway_without_shared_binding_uses_original_lease,
    "legacy"
);
flow_case!(
    shared_zcode_missing_transport_capability_fails_before_network,
    "missing_transport"
);

flow_case!(
    visible_zcode_stream_retires_after_original_account_removal_without_replay,
    "stream_retire"
);
flow_case!(
    one_ordered_group_uses_zcode_then_original_minimax_owner,
    "mixed"
);

flow_case!(
    zcode_source_context_native_identity_is_frozen,
    "source_context_native"
);
flow_case!(
    zcode_source_context_selected_identity_overrides_native_and_is_frozen,
    "source_context_selected"
);
flow_case!(
    zcode_source_context_absence_does_not_revive_after_native_state_appears,
    "source_context_none"
);
flow_case!(
    zcode_source_context_stream_retains_owner_headers_and_account_fence,
    "source_context_stream"
);
