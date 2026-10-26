#![cfg(feature = "test-hooks")]
macro_rules! archive_case {
    ($name:ident, $case:literal) => {
        #[test]
        #[serial_test::serial]
        fn $name() {
            cc_switch_lib::integration_test_support::verify_archive_stage_privacy_case($case);
        }
    };
}
archive_case!(archive_member_conflict, "archive_member_conflict");
archive_case!(archive_corrupt_stream, "archive_corrupt_stream");
archive_case!(archive_invalid_structure, "archive_invalid_structure");
archive_case!(archive_boundary_control, "archive_boundary_control");
