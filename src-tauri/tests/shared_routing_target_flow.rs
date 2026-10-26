#![cfg(all(feature = "gui", any(debug_assertions, feature = "test-hooks")))]
#[allow(dead_code)]
#[path = "support.rs"]
mod support;
#[test]
fn actual_native_serde_matches_shared_wire_fixture() {
    let actual = cc_switch_lib::integration_test_support::shared_routing_wire_fixture();
    println!("WIRE_JSON={}", actual);
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/shared-routing-wire.json"
    ))
    .unwrap();
    assert_eq!(actual, expected);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn manual_same_gateway_failure_never_attempts_other_account() {
    support::ensure_test_home();
    cc_switch_lib::integration_test_support::verify_shared_routing_flow("manual").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn native_target_confirm_preserves_gateway_bytes_and_receipt() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(&state, "confirm")
        .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn native_pending_target_recovers_exact_original_receipt() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "journal_recovery",
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn shared_group_preview_rejects_changed_native_source_facts() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "group_source_changed",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn codex_native_recovery_without_external_checks_reads_back_target() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Codex).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "codex_recovery",
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn native_recovery_rejects_changed_source_and_retains_original_pending() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "journal_source_changed",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn dirty_attached_client_cannot_be_adopted_as_route_preview() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "prepare_client_dirty",
    )
    .await;
}
#[cfg(feature = "test-hooks")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn pending_route_recovery_rejects_external_client_url_change() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "journal_client_dirty",
    )
    .await;
}
#[cfg(feature = "test-hooks")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn codex_pending_route_recovery_rejects_external_client_url_change() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Codex).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "codex_client_dirty",
    )
    .await;
}

#[cfg(feature = "test-hooks")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn group_confirmation_serializes_source_evidence_and_cas() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "group_atomic",
    )
    .await;
}
#[cfg(feature = "test-hooks")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn target_publication_serializes_source_evidence_and_cas() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "target_atomic",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn target_confirmation_rejects_source_changed_before_transaction() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "target_source_changed",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn gemini_native_target_preserves_original_gateway_bytes() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Gemini).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "gemini_confirm",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn grok_native_target_preserves_original_gateway_bytes() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::GrokBuild).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "grok_confirm",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn bound_target_rejects_legacy_selection_and_tray_switch() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "bound_legacy_selection",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn bound_target_rejects_ignored_legacy_order_and_auto() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "bound_legacy_order_auto",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn binding_invalidates_prepared_legacy_route_receipt() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "bound_legacy_receipt",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn unbound_legacy_selection_order_and_auto_keep_original_behavior() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "unbound_legacy",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn explicit_exit_preserves_original_direct_provider_selection() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "bound_exit_direct",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn bound_target_compat_config_command_rejects_owned_flags_but_keeps_options() {
    use tauri::Manager;
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Claude).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "bound_keep_for_compat_command",
    )
    .await;
    let app = tauri::test::mock_builder()
        .manage(state.clone())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let original = state.db.get_proxy_config_for_app("claude").await.unwrap();
    let mut auto = original.clone();
    auto.auto_failover_enabled = !auto.auto_failover_enabled;
    let auto_result = cc_switch_lib::update_proxy_config_for_app(app.state(), auto).await;
    let after_auto = state.db.get_proxy_config_for_app("claude").await.unwrap();
    // Restore only synthetic fixture metadata before exercising the second command.
    state
        .db
        .update_proxy_config_for_app(original.clone())
        .await
        .unwrap();
    let mut enable = original.clone();
    enable.enabled = !enable.enabled;
    let enable_result = cc_switch_lib::update_proxy_config_for_app(app.state(), enable).await;
    let after_enable = state.db.get_proxy_config_for_app("claude").await.unwrap();
    state
        .db
        .update_proxy_config_for_app(original.clone())
        .await
        .unwrap();
    let mut options = original.clone();
    options.non_streaming_timeout += 1;
    let options_result =
        cc_switch_lib::update_proxy_config_for_app(app.state(), options.clone()).await;
    let after_options = state.db.get_proxy_config_for_app("claude").await.unwrap();
    state
        .proxy_service
        .set_takeover_for_app("claude", false)
        .await
        .unwrap();
    assert!(
        auto_result.is_err(),
        "compat command accepted ignored auto flag"
    );
    assert!(
        enable_result.is_err(),
        "compat command impersonated original connection owner"
    );
    assert_eq!(
        after_auto.auto_failover_enabled,
        original.auto_failover_enabled
    );
    assert_eq!(after_enable.enabled, original.enabled);
    options_result.unwrap();
    assert_eq!(
        after_options.non_streaming_timeout,
        options.non_streaming_timeout
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn detached_bound_codex_rejects_old_selection_before_catalog_or_quit() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Codex).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "bound_detached_codex",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn bound_official_restore_rejects_before_quit_or_backup() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Codex).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "official_bound",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn explicit_exit_allows_original_official_restore_with_synthetic_effects() {
    support::reset_test_fs();
    let state = support::create_direct_test_state(cc_switch_lib::AppType::Codex).unwrap();
    cc_switch_lib::integration_test_support::verify_shared_routing_target_owner(
        &state,
        "official_direct",
    )
    .await;
}
