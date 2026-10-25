#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]

#[tokio::test]
async fn same_version_refresh_is_single_flight_and_waiter_reuses_published_credentials() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "same_version_refresh_is_single_flight_and_waiter_reuses_published_credentials",
    )
    .await;
}

#[tokio::test]
async fn different_accounts_refresh_without_global_network_serialization() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "different_accounts_refresh_without_global_network_serialization",
    )
    .await;
}

#[tokio::test]
async fn cancelled_leader_returns_unknown_to_existing_waiter_without_replay() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "cancelled_leader_returns_unknown_to_existing_waiter_without_replay",
    )
    .await;
}

#[tokio::test]
async fn authorization_replaced_while_refresh_pending_cannot_publish_old_result() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "authorization_replaced_while_refresh_pending_cannot_publish_old_result",
    )
    .await;
}

#[tokio::test]
async fn failed_refresh_result_is_shared_without_a_waiter_retry() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "failed_refresh_result_is_shared_without_a_waiter_retry",
    )
    .await;
}

#[tokio::test]
async fn refresh_network_failure_is_not_reported_as_account_authentication_failure() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "refresh_network_failure_is_not_reported_as_account_authentication_failure",
    )
    .await;
}

#[tokio::test]
async fn successful_flight_waiter_rechecks_authorization_before_returning_credentials() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "successful_flight_waiter_rechecks_authorization_before_returning_credentials",
    )
    .await;
}

#[tokio::test]
async fn late_old_snapshot_after_flight_exit_keeps_original_stale_guard_without_network() {
    cc_switch_lib::integration_test_support::verify_minimax_refresh_owner(
        "late_old_snapshot_after_flight_exit_keeps_original_stale_guard_without_network",
    )
    .await;
}
