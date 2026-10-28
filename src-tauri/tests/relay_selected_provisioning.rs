#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]

//! Synthetic loopback fixtures only; not a real-account acceptance test.
#[allow(dead_code)]
#[path = "support.rs"]
mod support;

macro_rules! fixture_case {
    ($name:ident, $case:expr) => {
        #[tokio::test]
        #[serial_test::serial]
        async fn $name() {
            support::ensure_test_home();
            cc_switch_lib::integration_test_support::verify_selected_relay_provisioning($case)
                .await;
        }
    };
}

fixture_case!(
    newapi_prepares_only_selected_resources_and_reuses_shared_keys,
    "newapi_selected"
);
fixture_case!(
    sub2api_prepares_only_selected_resources_and_reuses_shared_keys,
    "sub2api_selected"
);
fixture_case!(
    inactive_selection_is_rejected_before_any_key_write,
    "sub2api_inactive"
);
fixture_case!(
    discovery_is_read_only_and_returns_no_credentials,
    "discovery"
);
fixture_case!(
    unresolved_identity_cannot_emit_unstable_provider_ids,
    "unresolved_identity"
);
fixture_case!(
    selected_persistence_preserves_unselected_existing_projections,
    "persistence"
);
fixture_case!(
    refresh_uses_stable_ids_without_requiring_new_group_metadata,
    "legacy"
);
fixture_case!(
    shared_account_preparation_is_serialized_across_app_windows,
    "lock"
);
fixture_case!(
    unknown_capability_is_disclosed_and_real_catalog_preserves_partial_outcomes,
    "capabilities"
);
fixture_case!(
    composite_catalog_failure_retains_created_resource_receipt_and_reuses_on_retry,
    "composite_partial"
);

fixture_case!(
    selected_preparation_preserves_existing_provider_current_catalog_mcp_and_chain,
    "selected_isolation"
);

fixture_case!(
    selected_new_projection_preserves_mergeable_current_duplicate_mcp_and_live_files,
    "selected_new_isolation"
);

fixture_case!(
    selected_initial_metadata_failure_rolls_back_provider_and_chain,
    "selected_metadata_rollback"
);
