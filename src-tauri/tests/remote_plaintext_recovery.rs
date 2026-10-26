#![cfg(all(feature = "gui", feature = "test-hooks"))]
macro_rules! case {
    ($name:ident,$key:literal) => {
        #[test]
        #[serial_test::serial]
        fn $name() {
            cc_switch_lib::integration_test_support::verify_remote_plaintext_recovery_case($key);
        }
    };
}
case!(remote_plain_recovery_intent, "intent");
case!(remote_plain_recovery_database, "database");
case!(remote_plain_recovery_skills, "skills");
case!(remote_plain_recovery_descriptor, "descriptor");
case!(remote_plain_recovery_downgrade, "downgrade");
case!(remote_plain_recovery_drift, "drift");

case!(remote_plain_current_preview, "current_preview");

case!(remote_plain_recovery_legacy_cache, "legacy_cache");
case!(remote_plain_recovery_real_exit, "real_exit");
