use super::super::admission::{BuildFingerprint, KeyMode, Platform, WriterState};
use super::*;
use std::path::PathBuf;
use std::sync::Mutex;

fn observation() -> ContextObservation {
    ContextObservation {
        install: BuildFingerprint { platform: Platform::MacOs, version: "synthetic-version".into(), build: "synthetic-build".into(), artifact_sha256: [9;32] },
        credential_root: PathBuf::from("/synthetic/selected/.zcode/v2"),
        settings_file: PathBuf::from("/synthetic/home/.zcode/v2/setting.json"),
        root_identity: [1,2], settings_identity: [1,3],
        home: "/synthetic/home".into(), settings_home: "/synthetic/home".into(), bootstrap_home: "/synthetic/home".into(), username: "private-system-user".into(),
        key_choice: KeyMode::Standard, writers: WriterState::Stopped,
        settings: br#"{"dataBaseDir":"/synthetic/selected","providerFamilyDomain":"zai","providerFamilyConnectionSelections":{"zai":{"kind":"individual-coding-plan"}},"unknown":"SYNTHETIC_SETTINGS_CANARY"}"#.to_vec(),
    }
}
fn contracts() -> Vec<ContractEntry> {
    vec![ContractEntry {
        fingerprint: observation().install,
        native_gate_passed: true,
    }]
}
struct Probe(Mutex<ContextObservation>);
impl ContextProbe for Probe {
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        Ok(self.0.lock().unwrap().clone())
    }
}
#[test]
fn source_selection_rejects_extra_trusted_flags_or_secret_fields() {
    let base = serde_json::json!({"installPath":"/synthetic/ZCode.app","dataRoot":"/synthetic/selected/.zcode/v2","keyMode":"standard"});
    assert!(serde_json::from_value::<ContextSelection>(base.clone()).is_ok());
    for field in [
        "nativeGatePassed",
        "appStopped",
        "secret",
        "home",
        "username",
        "family",
        "standardDesktopLaunch",
    ] {
        let mut input = base.clone();
        input[field] = serde_json::json!("untrusted");
        assert!(serde_json::from_value::<ContextSelection>(input).is_err());
    }
}
#[test]
fn source_summary_exposes_only_reviewed_path_and_public_metadata() {
    let status = summary(observation(), &contracts()).unwrap();
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 6);
    assert_eq!(json["dataRoot"], "/synthetic/selected/.zcode/v2");
    assert_eq!(json["family"], "zai");
    let text = json.to_string();
    assert!(!text.contains("SYNTHETIC_SETTINGS_CANARY"));
    assert!(!text.contains("private-system-user"));
    assert!(!text.contains("/synthetic/home"));
}
#[test]
fn expected_context_rejects_settings_or_root_replacement_between_review_and_action() {
    for replace_settings in [false, true] {
        let current = observation();
        let expected = VerifiedContext::assess(current.clone(), &contracts())
            .unwrap()
            .context_revision();
        let inner = Arc::new(Probe(Mutex::new(current)));
        let probe = ExpectedContextProbe {
            inner: inner.clone(),
            contracts: contracts(),
            expected,
        };
        assert!(probe.observe().is_ok());
        {
            let mut updated = inner.0.lock().unwrap();
            if replace_settings {
                updated.settings_identity[1] += 1;
            } else {
                updated.root_identity[1] += 1;
            }
        }
        assert!(matches!(
            probe.observe(),
            Err(BlockedReason::ContextChanged)
        ));
    }
}
#[test]
fn expected_context_rechecks_unknown_build_and_writer_even_when_revision_is_unchanged() {
    for change_build in [false, true] {
        let current = observation();
        let expected = VerifiedContext::assess(current.clone(), &contracts())
            .unwrap()
            .context_revision();
        let inner = Arc::new(Probe(Mutex::new(current)));
        let probe = ExpectedContextProbe {
            inner: inner.clone(),
            contracts: contracts(),
            expected,
        };
        {
            let mut updated = inner.0.lock().unwrap();
            if change_build {
                updated.install.artifact_sha256[0] ^= 1;
            } else {
                updated.writers = WriterState::Unknown;
            }
        }
        assert!(matches!(
            probe.observe(),
            Err(BlockedReason::UnsupportedBuild | BlockedReason::WriterStateUnknown)
        ));
    }
}
#[test]
fn public_errors_keep_known_commit_and_static_codes_without_dynamic_error_text() {
    for (error, code, committed) in [
        (
            RuntimeError::Transaction(TransactionError::CommittedNeedsCleanup),
            "zcode.account.committed_recovery_required",
            true,
        ),
        (
            RuntimeError::Transaction(TransactionError::ArchiveNeedsCleanup),
            "zcode.account.recovery_cleanup_required",
            false,
        ),
        (
            RuntimeError::Transaction(TransactionError::Recovery(RecoveryError::ArchiveFull)),
            "zcode.account.recovery_full",
            false,
        ),
        (
            RuntimeError::Transaction(TransactionError::Checkpoint(CheckpointError::Native(
                super::super::native::NativeError::AuthenticationFailed,
            ))),
            "zcode.account.native_session_invalid",
            false,
        ),
        (
            RuntimeError::VaultUnavailable,
            "zcode.account.vault_unavailable",
            false,
        ),
    ] {
        let public = PublicError::from(error);
        let json = serde_json::to_value(public).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert_eq!(json["code"], code);
        assert_eq!(json["committed"], committed);
        assert!(json.get("message").is_none());
    }
}

#[test]
fn native_input_construction_defers_all_os_and_file_access_to_worker_observation() {
    let source: ContextSelection = serde_json::from_value(serde_json::json!({
        "installPath":"/never-opened-by-construction/ZCode.app",
        "dataRoot":"/never-opened-by-construction/.zcode/v2","keyMode":"standard"
    }))
    .unwrap();
    assert!(inputs(source, "synthetic-reviewed-revision".into()).is_ok());
}
