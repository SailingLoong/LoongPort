#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn generation_files_plain_to_encrypted() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("plain_to_encrypted");
}
#[test]
#[serial_test::serial]
fn generation_files_encrypted_to_plain() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("encrypted_to_plain");
}
#[test]
#[serial_test::serial]
fn generation_files_separate_device() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("separate_device");
}
#[test]
#[serial_test::serial]
fn generation_files_source_fence() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("source_fence");
}
#[test]
#[serial_test::serial]
fn generation_files_target_fence() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("target_fence");
}
#[test]
#[serial_test::serial]
fn generation_files_exclusions() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("exclusions");
}
#[test]
#[serial_test::serial]
fn generation_files_collision() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("collision");
}
#[test]
#[serial_test::serial]
fn generation_files_strict_codec() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("strict_codec");
}
#[test]
#[serial_test::serial]
fn generation_files_target_overlap() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("target_overlap");
}
#[test]
#[serial_test::serial]
fn generation_files_path_types() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("path_types");
}
#[test]
#[serial_test::serial]
fn generation_files_reserved_artifacts() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("reserved_artifacts");
}
#[test]
#[serial_test::serial]
fn generation_files_nonempty_target() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("nonempty_target");
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn generation_files_neutral_execute() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("neutral_execute");
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn generation_files_source_execute_fence() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("source_execute_fence");
}

#[cfg(unix)]
#[test]
#[serial_test::serial]
fn generation_files_target_execute_fence() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("target_execute_fence");
}

#[test]
#[serial_test::serial]
fn generation_files_inspect_encrypted() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("inspect_encrypted");
}

#[test]
#[serial_test::serial]
fn generation_files_inspect_plaintext() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("inspect_plaintext");
}

#[test]
#[serial_test::serial]
fn generation_files_inspect_changes() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("inspect_changes");
}

#[test]
#[serial_test::serial]
fn generation_files_inspect_execute() {
    cc_switch_lib::integration_test_support::verify_generation_files_case("inspect_execute");
}

#[test]
#[serial_test::serial]
fn generation_files_settings_owner_equivalence() {
    cc_switch_lib::integration_test_support::verify_generation_files_case(
        "settings_owner_equivalence",
    );
}
