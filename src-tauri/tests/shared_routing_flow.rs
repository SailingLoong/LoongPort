#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]
#[allow(dead_code)]
#[path = "support.rs"]
mod support;
#[tokio::test]
#[serial_test::serial]
async fn two_applications_share_order_instead_of_legacy_current() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("order").await;
}
#[tokio::test]
#[serial_test::serial]
async fn two_applications_use_existing_http_fallback_with_member_models() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("fallback").await;
}

#[tokio::test]
#[serial_test::serial]
async fn allowed_alias_does_not_authorize_a_different_group_member_model() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("auth_override").await;
}

#[tokio::test]
#[serial_test::serial]
async fn unbound_legacy_route_keeps_current_provider() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("legacy").await;
}

#[tokio::test]
#[serial_test::serial]
async fn allowed_alias_and_explicit_member_models_remain_usable() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("auth_allowed").await;
}

#[tokio::test]
#[serial_test::serial]
async fn member_model_revocation_fences_successful_response() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("auth_lease").await;
}

#[tokio::test]
#[serial_test::serial]
async fn all_group_members_unavailable_never_falls_outside_group() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("unavailable").await;
}

#[tokio::test]
#[serial_test::serial]
async fn unverified_member_model_fails_without_provisioning() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("unknown_model").await;
}

#[tokio::test]
#[serial_test::serial]
async fn historical_authenticated_layout_without_group_tables_remains_legacy() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("historical").await;
}

#[tokio::test]
#[serial_test::serial]
async fn current_schema_missing_group_tables_fails_closed() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("missing_schema").await;
}

#[tokio::test]
#[serial_test::serial]
async fn versioned_schema_migration_validates_before_commit() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("schema").await;
}

#[tokio::test]
#[serial_test::serial]
async fn inflight_group_order_and_models_stay_captured_next_request_sees_revision() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("snapshot").await;
}

#[tokio::test]
#[serial_test::serial]
async fn committed_stream_output_does_not_replay_on_next_member() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("stream").await;
}

#[tokio::test]
#[serial_test::serial]
async fn source_change_during_request_preserves_existing_capacity_guard() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("source_changed").await;
}

#[tokio::test]
#[serial_test::serial]
async fn retryable_group_exhaustion_fails_after_ordered_members_without_outside_fallback() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("exhausted").await;
}

#[tokio::test]
#[serial_test::serial]
async fn explicit_group_attempts_are_not_truncated_by_legacy_app_retry_budget() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("group_budget").await;
}

#[tokio::test]
#[serial_test::serial]
async fn unbound_legacy_zero_retry_budget_still_stops_after_first_failure() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("legacy_budget").await;
}
