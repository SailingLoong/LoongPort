#![cfg(all(feature = "gui", feature = "test-hooks"))]

#[test]
#[serial_test::serial]
fn retained_checkpoint_dead_accept() {
    cc_switch_lib::integration_test_support::verify_ci_retained_attached_case("dead_accept");
}
#[test]
#[serial_test::serial]
fn retained_checkpoint_late_disabled() {
    cc_switch_lib::integration_test_support::verify_ci_retained_attached_case("late_disabled");
}
#[test]
#[serial_test::serial]
fn retained_checkpoint_restart() {
    cc_switch_lib::integration_test_support::verify_ci_retained_attached_case("restart");
}

#[test]
#[serial_test::serial]
fn retained_checkpoint_admitted_create() {
    cc_switch_lib::integration_test_support::verify_ci_retained_attached_case("admitted_create");
}

#[test]
#[serial_test::serial]
fn retained_checkpoint_admitted_key_edit() {
    cc_switch_lib::integration_test_support::verify_ci_retained_attached_case("admitted_key_edit");
}

#[test]
#[serial_test::serial]
fn retained_checkpoint_admitted_same_key_reissue() {
    cc_switch_lib::integration_test_support::verify_ci_retained_attached_case(
        "admitted_same_key_reissue",
    );
}
