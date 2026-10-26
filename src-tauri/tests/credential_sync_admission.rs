#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn original_foreign_source_precedes_skill_work() {
    cc_switch_lib::integration_test_support::verify_sync_admission_case("foreign");
}
#[test]
#[serial_test::serial]
fn original_schema_and_forgery_rejections_preserve_all_bytes() {
    cc_switch_lib::integration_test_support::verify_sync_admission_case("schema");
}
