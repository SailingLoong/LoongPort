//! Synthetic startup diagnostics only; no OS key, reset or recovery operation.
#[cfg(feature = "test-hooks")]
#[test]
fn storage_upgrade_and_unknown_failures_do_not_claim_missing_keys() {
    cc_switch_lib::integration_test_support::verify_normal_upgrade_startup("error-classification");
}

#[cfg(feature = "test-hooks")]
#[test]
fn startup_errors_and_early_logs_use_original_redaction_and_preserve_reset_marker() {
    cc_switch_lib::integration_test_support::verify_normal_upgrade_startup("error-redaction");
}

#[test]
fn original_redacted_logger_and_version_are_ready_before_inspection_or_recovery_returns() {
    let source = include_str!("../src/lib.rs");
    let start = source.find("pub fn run() {").unwrap();
    let startup = &source[start..];
    let logger = startup
        .find("prepare_startup_diagnostic_root(&app_config_dir)")
        .unwrap();
    let inspection = startup
        .find("let inspection = match secrets::upgrade::inspect")
        .unwrap();
    let recovery = startup
        .find("if inspection.is_recovery_required() || inspection.is_database_resume_candidate()")
        .unwrap();
    let version = startup.find("=== LoongPort v{} started ===").unwrap();
    assert!(
        logger < inspection,
        "passive inspection failures must reach the original redacted log"
    );
    assert!(
        version < recovery,
        "early recovery return must not skip the versioned log initialization"
    );
    assert!(startup[logger..inspection].contains("diagnostics::format_log_line"));
}

#[cfg(feature = "test-hooks")]
#[test]
fn diagnostic_path_preflight_rejects_aliases_without_writing_or_chmod() {
    cc_switch_lib::integration_test_support::verify_normal_upgrade_startup("diagnostics-preflight");
}

#[test]
fn early_diagnostic_failure_keeps_original_error_payload_path() {
    let source = include_str!("../src/lib.rs");
    let startup = &source[source.find("pub fn run() {").unwrap()..];
    assert!(
        startup.contains("if let Err(error) = prepare_startup_diagnostic_root(&app_config_dir)"),
        "early diagnostic failure must be reported, not escape Tauri setup via ?"
    );
}
