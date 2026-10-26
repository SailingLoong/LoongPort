#![cfg(feature = "test-hooks")]
#[test]
#[serial_test::serial]
fn empty_image_accepts_actual_default_clock_difference_without_source_writes() {
    cc_switch_lib::Database::verify_empty_storage_image_case(
        "empty_image_accepts_actual_default_clock_difference_without_source_writes",
    );
}
#[test]
#[serial_test::serial]
fn empty_image_rejects_unknown_schema_rows_and_credentials() {
    cc_switch_lib::Database::verify_empty_storage_image_case(
        "empty_image_rejects_unknown_schema_rows_and_credentials",
    );
}
#[test]
#[serial_test::serial]
fn empty_image_rejects_invalid_or_unequal_default_times() {
    cc_switch_lib::Database::verify_empty_storage_image_case(
        "empty_image_rejects_invalid_or_unequal_default_times",
    );
}
