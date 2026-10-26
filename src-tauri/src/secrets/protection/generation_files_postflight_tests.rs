//! Synthetic source-membership and interrupted-write admission, never cleanup.
use super::*;
use crate::secrets::{storage::PlaintextIdentity, VaultContext};

const OPAQUE: &[u8] = b"unparsed-new-source-credential-canary";
struct Roots {
    temp: tempfile::TempDir,
    data: PathBuf,
    device: PathBuf,
    target: PathBuf,
}
impl Roots {
    fn new(separate: bool) -> Self {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let data = temp.path().join("data");
        let device = if separate {
            temp.path().join("device")
        } else {
            data.clone()
        };
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&device).unwrap();
        let target = temp.path().join("target");
        Self {
            temp,
            data,
            device,
            target,
        }
    }
    fn stage(&self) -> Result<PreparedGenerationFiles, AppError> {
        let source = StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: "11111111-1111-4111-8111-111111111111".into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        });
        let next: StorageContext = VaultContext::create_generation_with_user_key(
            "11111111-1111-4111-8111-111111111111".into(),
            2,
            Zeroizing::new(vec![29; 32]),
        )
        .unwrap()
        .into();
        PreparedGenerationFiles::stage(&self.data, &self.device, &self.target, &source, &next)
    }
}
fn write(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, OPAQUE).unwrap();
}
fn assert_remaining(roots: &Roots, path: &Path) {
    write(path);
    match ensure_no_credential_files(&roots.data, &roots.device) {
        Err(AppError::Config(code)) => assert_eq!(code, "protection.source_credentials_remaining"),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(()) => panic!("newly present source credential was not detected"),
    }
    assert_eq!(
        std::fs::read(path).unwrap(),
        OPAQUE,
        "inventory must not delete or decode new bytes"
    );
    assert!(!roots.target.exists());
}
fn assert_staging_rejected(roots: &Roots, path: &Path) {
    write(path);
    match roots.stage() {
        Err(AppError::Config(code)) => assert_eq!(code, "protection.interrupted_credential_write"),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("registered durable staging bytes were accepted as neutral data"),
    }
    assert!(
        !roots.target.exists(),
        "reject before creating generation target"
    );
    assert_eq!(std::fs::read(path).unwrap(), OPAQUE);
    assert_remaining(roots, path);
}

#[cfg_attr(test, test)]
fn empty_and_unrelated_sources_are_accepted_without_writes() {
    let roots = Roots::new(true);
    write(&roots.data.join("skills/ordinary/SKILL.md"));
    write(&roots.device.join("workbuddy_accounts.json")); // inactive data, not fixed-device ownership
    write(&roots.device.join(crate::config::DB_FILE_NAME));
    ensure_no_credential_files(&roots.data, &roots.device).unwrap();
    assert_eq!(
        std::fs::read(roots.data.join("skills/ordinary/SKILL.md")).unwrap(),
        OPAQUE
    );
    let missing = roots.temp.path().join("missing");
    ensure_no_credential_files(&missing, &missing).unwrap();
    assert!(!missing.exists());
    assert!(!roots.target.exists());
}
#[cfg_attr(test, test)]
fn rebuilt_owned_and_recovery_files_remain_untouched_and_block_completion() {
    for relative in [
        "workbuddy_accounts.json",
        "backups/hermes/hermes_123.yaml",
        "backups/vault-recovery/codex_oauth_auth.json",
    ] {
        let roots = Roots::new(true);
        assert_remaining(&roots, &roots.data.join(relative));
    }
}
#[cfg_attr(test, test)]
fn rebuilt_fixed_device_files_remain_untouched_and_block_completion() {
    for separate in [false, true] {
        for relative in [
            "live-state.json".to_owned(),
            format!("backups/live-first-write/{}.backup", "a".repeat(64)),
        ] {
            let roots = Roots::new(separate);
            assert_remaining(&roots, &roots.device.join(relative));
        }
    }
}
#[cfg_attr(test, test)]
fn settings_members_use_the_original_three_physical_source_locations() {
    for where_ in 0..3 {
        let roots = Roots::new(true);
        let path = match where_ {
            0 => roots.data.join("settings.json"),
            1 => roots.data.join("backups/vault-recovery/settings.json"),
            _ => roots.device.join("settings.json"),
        };
        assert_remaining(&roots, &path);
    }
}
#[cfg_attr(test, test)]
fn owned_durable_staging_is_not_neutral_or_cleanup_evidence() {
    for relative in [
        "workbuddy_accounts.json.tmp.1.2.3",
        "backups/hermes/hermes_123.yaml.tmp.1.2.3",
        "backups/vault-recovery/codex_oauth_auth.json.tmp.1.2.3",
    ] {
        let roots = Roots::new(true);
        assert_staging_rejected(&roots, &roots.data.join(relative));
    }
}
fn device_staging(separate: bool) {
    for relative in [
        "live-state.json.tmp.1.2.3".to_owned(),
        format!(
            "backups/live-first-write/{}.backup.tmp.1.2.3",
            "a".repeat(64)
        ),
    ] {
        let roots = Roots::new(separate);
        assert_staging_rejected(&roots, &roots.device.join(relative));
    }
}
#[cfg_attr(test, test)]
fn device_durable_staging_is_rejected_in_the_shared_root() {
    device_staging(false);
}
#[cfg_attr(test, test)]
fn device_durable_staging_is_rejected_in_the_separate_device_root() {
    device_staging(true);
}
#[cfg_attr(test, test)]
fn settings_durable_staging_is_rejected_at_each_original_location() {
    for where_ in 0..3 {
        let roots = Roots::new(true);
        let path = match where_ {
            0 => roots.data.join("settings.json.tmp.1.2.3"),
            1 => roots
                .data
                .join("backups/vault-recovery/settings.json.tmp.1.2.3"),
            _ => roots.device.join("settings.json.tmp.1.2.3"),
        };
        assert_staging_rejected(&roots, &path);
    }
}
#[cfg_attr(test, test)]
fn unregistered_durable_looking_names_remain_original_neutral_data() {
    let roots = Roots::new(true);
    let names = [
        "notes.txt.tmp.1.2.3",
        "skills/local/readme.tmp.4.5.6",
        "workbuddy_accounts.json.tmp.not-a-durable-suffix",
    ];
    for name in names {
        write(&roots.data.join(name));
    }
    let staged = roots.stage().unwrap();
    staged.verify_source().unwrap();
    staged.verify_target().unwrap();
    for name in names {
        assert_eq!(std::fs::read(roots.target.join(name)).unwrap(), OPAQUE);
    }
    assert!(staged
        .evidence()
        .all(|entry| entry.role() == GenerationFileRole::Neutral));
    ensure_no_credential_files(&roots.data, &roots.device).unwrap();
}
#[cfg(unix)]
#[cfg_attr(test, test)]
fn source_inventory_does_not_follow_owned_ancestor_aliases() {
    use std::os::unix::fs::symlink;
    let roots = Roots::new(true);
    let outside = roots.temp.path().join("outside");
    write(&outside.join("hermes/hermes_123.yaml"));
    symlink(&outside, roots.data.join("backups")).unwrap();
    assert!(ensure_no_credential_files(&roots.data, &roots.device).is_err());
    assert_eq!(
        std::fs::read(outside.join("hermes/hermes_123.yaml")).unwrap(),
        OPAQUE
    );
    assert!(!roots.target.exists());
}

#[cfg_attr(test, test)]
fn orphan_database_sidecars_keep_the_original_capture_refusal() {
    for relative in [
        "backups/retained.db-wal",
        "backups/retained.db-shm",
        "backups/retained.db-journal",
    ] {
        let roots = Roots::new(true);
        let path = roots.data.join(relative);
        write(&path);
        match ensure_no_credential_files(&roots.data, &roots.device) {
            Err(AppError::Config(code)) => {
                assert_eq!(code, "protection.unclaimed_database_artifact")
            }
            Err(other) => panic!("unexpected error: {other}"),
            Ok(()) => panic!("orphan DB sidecar was silently treated as neutral"),
        }
        assert_eq!(std::fs::read(path).unwrap(), OPAQUE);
    }
}
#[cfg_attr(test, test)]
fn recovery_database_shapes_keep_the_original_capture_refusal() {
    for relative in [
        "backups/vault-recovery/old.db",
        "backups/vault-recovery/deep/old.db-wal",
        "backups/vault-recovery/vault.json",
    ] {
        let roots = Roots::new(true);
        let path = roots.data.join(relative);
        write(&path);
        match ensure_no_credential_files(&roots.data, &roots.device) {
            Err(AppError::Config(code)) => {
                assert_eq!(code, "protection.unclaimed_recovery_artifact")
            }
            Err(other) => panic!("unexpected error: {other}"),
            Ok(()) => panic!("unclaimed recovery lifecycle artifact was treated as neutral"),
        }
        assert_eq!(std::fs::read(path).unwrap(), OPAQUE);
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "empty_neutral" => empty_and_unrelated_sources_are_accepted_without_writes(),
        "owned" => rebuilt_owned_and_recovery_files_remain_untouched_and_block_completion(),
        "device" => rebuilt_fixed_device_files_remain_untouched_and_block_completion(),
        "settings" => settings_members_use_the_original_three_physical_source_locations(),
        "staging_owned" => owned_durable_staging_is_not_neutral_or_cleanup_evidence(),
        "staging_device" => device_durable_staging_is_rejected_in_the_shared_root(),
        "staging_device_separate" => {
            device_durable_staging_is_rejected_in_the_separate_device_root()
        }
        "staging_settings" => settings_durable_staging_is_rejected_at_each_original_location(),
        "neutral_tmp" => unregistered_durable_looking_names_remain_original_neutral_data(),
        "orphan_sidecars" => orphan_database_sidecars_keep_the_original_capture_refusal(),
        "recovery_artifacts" => recovery_database_shapes_keep_the_original_capture_refusal(),
        #[cfg(unix)]
        "path_aliases" => source_inventory_does_not_follow_owned_ancestor_aliases(),
        _ => panic!("unknown credential file postflight case"),
    }
}
