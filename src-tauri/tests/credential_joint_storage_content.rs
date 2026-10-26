#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn joint_storage_plaintext() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("plaintext");
}
#[test]
#[serial_test::serial]
fn joint_storage_text() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("text");
}
#[test]
#[serial_test::serial]
fn joint_storage_wrong_source() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("wrong_source");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_recover_intent() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("recover_intent");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_recover_database() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("recover_database");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_recover_skills() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("recover_skills");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_metadata_change() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("metadata_change");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_database_change() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("database_change");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_skills_change() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("skills_change");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_stage_tamper() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("stage_tamper");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_home_change() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("home_change");
}

#[cfg(feature = "gui")]
#[test]
#[serial_test::serial]
fn joint_storage_real_process_exit() {
    cc_switch_lib::integration_test_support::verify_joint_storage_content_case("real_process_exit");
}
