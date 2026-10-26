#![cfg(all(feature = "gui", feature = "test-hooks"))]
#[test]
#[serial_test::serial]
fn protection_ipc_locked() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("locked");
}
#[test]
#[serial_test::serial]
fn protection_ipc_manual_cache_denied() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("manual_cache_denied");
}
#[test]
#[serial_test::serial]
fn protection_ipc_fresh_skip() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("fresh_skip");
}

#[test]
#[serial_test::serial]
fn protection_ipc_stale_review() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("stale_review");
}

#[test]
#[serial_test::serial]
fn protection_ipc_explicit_skip() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("explicit_skip");
}

#[test]
#[serial_test::serial]
fn protection_ipc_legacy_prompt() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("legacy_prompt");
}

#[test]
#[serial_test::serial]
fn protection_ipc_plain_to_text() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("plain_to_text");
}

#[test]
#[serial_test::serial]
fn protection_ipc_text_to_plain() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("text_to_plain");
}

#[test]
#[serial_test::serial]
fn protection_ipc_rekey() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("rekey");
}

#[test]
#[serial_test::serial]
fn protection_ipc_new_key_cache_failure() {
    cc_switch_lib::integration_test_support::verify_protection_ipc_case("new_key_cache_failure");
}
