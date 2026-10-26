#![cfg(feature = "test-hooks")]
macro_rules! transport_case {
    ($name:ident, $case:literal) => {
        #[test]
        #[serial_test::serial]
        fn $name() {
            cc_switch_lib::integration_test_support::verify_remote_plaintext_transport_case($case);
        }
    };
}
transport_case!(remote_plain_webdav_upload, "webdav_upload");
transport_case!(remote_plain_s3_upload, "s3_upload");
transport_case!(remote_plain_webdav_privacy, "webdav_privacy");
transport_case!(remote_plain_s3_privacy, "s3_privacy");
macro_rules! original_control {
    ($name:ident, $case:literal) => {
        #[test]
        #[serial_test::serial]
        fn $name() {
            cc_switch_lib::integration_test_support::verify_original_publication_control($case);
        }
    };
}
original_control!(remote_original_immutable_artifact, "immutable_artifact");
original_control!(remote_original_cas_loser, "cas_loser");
original_control!(remote_original_conditions, "conditions");
original_control!(remote_original_stale_receipt, "stale_receipt");

transport_case!(remote_plain_webdav_apply_plain, "webdav_apply_plain");
transport_case!(remote_plain_webdav_apply_text, "webdav_apply_text");
transport_case!(remote_plain_s3_apply_plain, "s3_apply_plain");
transport_case!(remote_plain_s3_apply_text, "s3_apply_text");
transport_case!(remote_plain_explicit_empty, "explicit_empty");
transport_case!(remote_plain_wrong_stale_source, "wrong_stale_source");

transport_case!(remote_plain_webdav_stale_receipt, "webdav_stale_receipt");
transport_case!(remote_plain_s3_stale_receipt, "s3_stale_receipt");

transport_case!(
    remote_plain_webdav_ignored_conditions,
    "webdav_ignored_conditions"
);
transport_case!(remote_plain_s3_ignored_conditions, "s3_ignored_conditions");
