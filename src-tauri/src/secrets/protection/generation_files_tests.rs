use super::*;
use crate::secrets::{
    files::OwnedFile, owned_file::DeviceFile, storage::PlaintextIdentity, VaultContext,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const STORE: &str = "11111111-1111-4111-8111-111111111111";
const SETTINGS: &[u8] = br#"{"webdavSync":{"baseUrl":"https://sync.example.invalid","username":"synthetic-user","password":"generation-file-canary","futureInner":{"n":18446744073709551615}},"foreign":{"n":18446744073709551615},"currentProviderCodex":"synthetic-provider","s3Sync":null}"#;
fn contexts() -> (StorageContext, StorageContext) {
    (
        StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: STORE.into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        }),
        VaultContext::create_generation_with_user_key(
            STORE.into(),
            2,
            Zeroizing::new(vec![23; 32]),
        )
        .unwrap()
        .into(),
    )
}
fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn raw_fields(bytes: &[u8]) -> std::collections::BTreeMap<String, String> {
    let fields: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(bytes).unwrap();
    fields
        .into_iter()
        .map(|(key, value)| (key, value.get().into()))
        .collect()
}
struct Fixture {
    temp: tempfile::TempDir,
    data: PathBuf,
    device: PathBuf,
    target: PathBuf,
    source: StorageContext,
    next: StorageContext,
}
impl Fixture {
    fn new(separate: bool) -> Self {
        let temp = crate::secrets::testing::tempdir().unwrap();
        let data = temp.path().join("data");
        let device = if separate {
            temp.path().join("device")
        } else {
            data.clone()
        };
        let target = temp.path().join("next");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&device).unwrap();
        let (source, next) = contexts();
        Self {
            temp,
            data,
            device,
            target,
            source,
            next,
        }
    }
    fn fill(&self) {
        for relative in [
            "workbuddy_accounts.json",
            "backups/hermes/hermes_123.yaml",
            "backups/vault-recovery/codex_oauth_auth.json",
        ] {
            write(&self.data.join(relative), b"generation-file-canary");
        }
        write(
            &self.device.join("live-state.json"),
            b"device-generation-canary",
        );
        write(
            &self.device.join(format!(
                "backups/live-first-write/{}.backup",
                "a".repeat(64)
            )),
            b"first-write-canary",
        );
        write(&self.data.join("settings.json"), SETTINGS);
        write(
            &self.data.join("backups/vault-recovery/settings.json"),
            SETTINGS,
        );
        if self.device != self.data {
            write(&self.device.join("settings.json"), SETTINGS);
        }
        write(
            &self.data.join("skills/nested/synthetic/SKILL.md"),
            b"# Synthetic skill\n",
        );
        write(&self.data.join("opaque.bin"), &[0, 255, 128, 17]);
        std::fs::create_dir_all(self.data.join("skills/empty")).unwrap();
    }
    fn stage(&self) -> PreparedGenerationFiles {
        PreparedGenerationFiles::stage(
            &self.data,
            &self.device,
            &self.target,
            &self.source,
            &self.next,
        )
        .unwrap()
    }
}

#[cfg_attr(test, test)]
fn explicit_plaintext_generation_stages_original_owned_device_and_recovery_owners() {
    let f = Fixture::new(false);
    f.fill();
    let prepared = f.stage();
    prepared.verify_source().unwrap();
    prepared.verify_target().unwrap();
    for relative in [
        "workbuddy_accounts.json",
        "backups/hermes/hermes_123.yaml",
        "backups/vault-recovery/codex_oauth_auth.json",
    ] {
        let identity = Path::new(relative)
            .strip_prefix("backups/vault-recovery")
            .unwrap_or(Path::new(relative));
        let encoded = std::fs::read(f.target.join(relative)).unwrap();
        assert!(!String::from_utf8_lossy(&encoded).contains("generation-file-canary"));
        assert_eq!(
            &*OwnedFile::registered(identity)
                .unwrap()
                .decode_storage(&f.next, &encoded)
                .unwrap(),
            b"generation-file-canary"
        );
        assert_eq!(
            std::fs::read(f.data.join(relative)).unwrap(),
            b"generation-file-canary"
        );
    }
    let device = std::fs::read(f.target.join("live-state.json")).unwrap();
    assert_eq!(
        &*DeviceFile::registered("live-state.json")
            .unwrap()
            .decode_storage(&f.next, &device)
            .unwrap(),
        b"device-generation-canary"
    );
    let evidence = prepared
        .evidence()
        .find(|e| e.relative_path() == Path::new("workbuddy_accounts.json"))
        .unwrap();
    assert_eq!(evidence.role(), GenerationFileRole::Owned);
    assert_eq!(evidence.source_root(), GenerationSourceRoot::Data);
    assert_eq!(
        evidence.source_path(),
        f.data.join("workbuddy_accounts.json")
    );
    assert_eq!(
        evidence.target_path(),
        f.target.join("workbuddy_accounts.json")
    );
    assert_eq!(
        evidence.source_hash(),
        &<[u8; 32]>::from(Sha256::digest(b"generation-file-canary"))
    );
    assert_eq!(
        evidence.target_hash(),
        &<[u8; 32]>::from(Sha256::digest(
            std::fs::read(evidence.target_path()).unwrap()
        ))
    );
}

#[cfg_attr(test, test)]
fn encrypted_to_plaintext_preserves_settings_raw_fields_and_neutral_tree() {
    let f = Fixture::new(false);
    f.fill();
    let first = f.stage();
    first.verify_target().unwrap();
    let restored = f.temp.path().join("restored");
    let second =
        PreparedGenerationFiles::stage(&f.target, &f.target, &restored, &f.next, &f.source)
            .unwrap();
    second.verify_source().unwrap();
    second.verify_target().unwrap();
    for relative in ["settings.json", "backups/vault-recovery/settings.json"] {
        assert_eq!(
            raw_fields(&std::fs::read(restored.join(relative)).unwrap()),
            raw_fields(SETTINGS)
        );
        assert!(
            !String::from_utf8_lossy(&std::fs::read(f.target.join(relative)).unwrap())
                .contains("generation-file-canary")
        );
    }
    assert_eq!(
        std::fs::read(restored.join("opaque.bin")).unwrap(),
        [0, 255, 128, 17]
    );
    assert_eq!(
        std::fs::read(restored.join("skills/nested/synthetic/SKILL.md")).unwrap(),
        b"# Synthetic skill\n"
    );
    assert!(restored.join("skills/empty").is_dir());
    assert_eq!(
        second
            .evidence()
            .find(|e| e.relative_path() == Path::new("opaque.bin"))
            .unwrap()
            .role(),
        GenerationFileRole::Neutral
    );
}

#[cfg_attr(test, test)]
fn separate_device_root_only_contributes_original_fixed_device_and_settings_inventory() {
    let f = Fixture::new(true);
    f.fill();
    write(&f.device.join("loongport.db"), b"old-inactive-database");
    write(
        &f.device.join("workbuddy_accounts.json"),
        b"old-inactive-account",
    );
    write(&f.device.join("skills/inactive.md"), b"old-inactive-skill");
    let prepared = f.stage();
    prepared.verify_source().unwrap();
    prepared.verify_target().unwrap();
    assert!(!f.target.join("loongport.db").exists());
    assert!(!f.target.join("skills/inactive.md").exists());
    let e = prepared
        .evidence()
        .find(|e| e.role() == GenerationFileRole::Device)
        .unwrap();
    assert_eq!(e.source_root(), GenerationSourceRoot::Device);
    assert!(e.source_path().starts_with(&f.device));
    assert_eq!(
        std::fs::read(f.device.join("workbuddy_accounts.json")).unwrap(),
        b"old-inactive-account"
    );
    assert_eq!(
        prepared
            .evidence()
            .filter(|e| e.relative_path() == Path::new("settings.json"))
            .count(),
        2
    );
}

#[cfg_attr(test, test)]
fn source_inventory_additions_removals_and_changes_in_every_role_are_rejected() {
    for relative in [
        "workbuddy_accounts.json",
        "live-state.json",
        "settings.json",
        "backups/vault-recovery/settings.json",
        "opaque.bin",
    ] {
        for change in ["replace", "remove"] {
            let f = Fixture::new(false);
            f.fill();
            let prepared = f.stage();
            let path = f.data.join(relative);
            if change == "replace" {
                write(&path, b"changed");
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            assert!(prepared.verify_source().is_err(), "{relative} {change}");
        }
    }
    for relative in [
        "trae_accounts.json",
        "codex-login-stash.json",
        "backups/vault-recovery/backups/backup_123.json",
        "skills/new.md",
    ] {
        let f = Fixture::new(false);
        f.fill();
        let prepared = f.stage();
        write(&f.data.join(relative), b"new-source-file");
        assert!(prepared.verify_source().is_err(), "{relative}");
    }
}

#[cfg_attr(test, test)]
fn target_inventory_additions_removals_and_changes_are_rejected() {
    for relative in [
        "workbuddy_accounts.json",
        "live-state.json",
        "settings.json",
        "backups/vault-recovery/settings.json",
        "opaque.bin",
    ] {
        for change in ["replace", "remove"] {
            let f = Fixture::new(false);
            f.fill();
            let prepared = f.stage();
            let path = f.target.join(relative);
            if change == "replace" {
                write(&path, b"changed");
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            assert!(prepared.verify_target().is_err(), "{relative} {change}");
        }
    }
    let f = Fixture::new(false);
    f.fill();
    let prepared = f.stage();
    write(&f.target.join("skills/unexpected.md"), b"new-target-file");
    assert!(prepared.verify_target().is_err());
}

#[cfg_attr(test, test)]
fn metadata_database_and_original_diagnostics_are_never_neutral_copies() {
    let f = Fixture::new(false);
    f.fill();
    let excluded = [
        "loongport.db",
        "loongport.db-wal",
        "loongport.db-shm",
        "loongport.db-journal",
        "vault.json",
        "protection.json",
        "logs/debug.log",
        "crash.log",
        "crash.log.1",
        "backups/backup_20261026.db",
        "backups/backup_20261026.db-wal",
        "backups/backup_20261026.db-shm",
        "backups/backup_20261026.db-journal",
    ];
    for relative in excluded {
        write(&f.data.join(relative), b"belongs-to-another-owner");
    }
    let prepared = f.stage();
    for relative in excluded {
        assert!(!f.target.join(relative).exists(), "{relative}");
    }
    write(
        &f.target.join("loongport.db"),
        b"new-database-owned-elsewhere",
    );
    write(
        &f.target.join("vault.json"),
        b"new-metadata-owned-elsewhere",
    );
    prepared.verify_target().unwrap();
    prepared.verify_source().unwrap();
}

#[cfg_attr(test, test)]
fn duplicate_destination_conflict_fails_before_any_target_publication() {
    let f = Fixture::new(true);
    f.fill();
    write(
        &f.device.join("settings.json"),
        br#"{"foreign":"different"}"#,
    );
    assert!(
        PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next).is_err()
    );
    assert!(!f.target.join("workbuddy_accounts.json").exists());
    assert_eq!(
        std::fs::read(f.data.join("settings.json")).unwrap(),
        SETTINGS
    );
}

#[cfg_attr(test, test)]
fn wrong_mode_or_key_and_invalid_settings_fail_before_target_publication() {
    for relative in [
        "workbuddy_accounts.json",
        "live-state.json",
        "settings.json",
        "backups/vault-recovery/settings.json",
    ] {
        let f = Fixture::new(false);
        f.fill();
        write(&f.data.join(relative), b"lpenc1.invalid");
        assert!(
            PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next)
                .is_err(),
            "{relative}"
        );
        assert!(!f.target.join("workbuddy_accounts.json").exists());
    }
    let f = Fixture::new(false);
    f.fill();
    write(
        &f.data.join("settings.json"),
        br#"{"foreign":1,"foreign":2}"#,
    );
    assert!(
        PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next).is_err()
    );
    assert!(!f.target.exists());
    let f = Fixture::new(false);
    f.fill();
    let prepared = f.stage();
    prepared.verify_target().unwrap();
    let wrong = VaultContext::create_generation_with_user_key(
        STORE.into(),
        3,
        Zeroizing::new(vec![42; 32]),
    )
    .unwrap()
    .into();
    assert!(PreparedGenerationFiles::stage(
        &f.target,
        &f.target,
        &f.temp.path().join("wrong"),
        &wrong,
        &f.source
    )
    .is_err());
}

#[cfg_attr(test, test)]
fn target_collision_and_overlapping_roots_never_overwrite_existing_bytes() {
    let f = Fixture::new(false);
    f.fill();
    write(
        &f.target.join("workbuddy_accounts.json"),
        b"preexisting-target",
    );
    assert!(
        PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next).is_err()
    );
    assert_eq!(
        std::fs::read(f.target.join("workbuddy_accounts.json")).unwrap(),
        b"preexisting-target"
    );
    for target in [&f.data, &f.data.join("nested"), f.data.parent().unwrap()] {
        assert!(
            PreparedGenerationFiles::stage(&f.data, &f.device, target, &f.source, &f.next).is_err()
        );
    }
    let (same, _) = contexts();
    assert!(PreparedGenerationFiles::stage(
        &f.data,
        &f.device,
        &f.temp.path().join("same"),
        &f.source,
        &same
    )
    .is_err());
}

#[cfg(unix)]
#[cfg_attr(test, test)]
fn aliases_special_files_and_linked_neutral_files_fail_closed_without_following() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    for kind in ["symlink", "hardlink", "fifo", "ancestor"] {
        let f = Fixture::new(false);
        f.fill();
        let external = f.temp.path().join("external");
        write(&external, b"outside-original");
        match kind {
            "symlink" => symlink(&external, f.data.join("neutral-alias")).unwrap(),
            "hardlink" => std::fs::hard_link(&external, f.data.join("neutral-hardlink")).unwrap(),
            "fifo" => {
                let name =
                    std::ffi::CString::new(f.data.join("neutral-fifo").as_os_str().as_bytes())
                        .unwrap();
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            }
            "ancestor" => {
                std::fs::remove_dir_all(f.data.join("skills")).unwrap();
                symlink(f.temp.path(), f.data.join("skills")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next)
                .is_err(),
            "{kind}"
        );
        assert_eq!(std::fs::read(&external).unwrap(), b"outside-original");
        assert!(!f.target.join("workbuddy_accounts.json").exists());
    }
}

#[cfg_attr(test, test)]
fn reserved_recovery_database_and_orphan_backup_sidecars_are_not_neutral_data() {
    for relative in [
        "backups/vault-recovery/vault.json",
        "backups/vault-recovery/loongport.db",
        "backups/vault-recovery/old.db",
        "backups/orphan.db-wal",
    ] {
        let f = Fixture::new(false);
        f.fill();
        write(&f.data.join(relative), b"unclaimed-database-or-metadata");
        assert!(
            PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next)
                .is_err(),
            "{relative}"
        );
        assert_eq!(
            std::fs::read(f.data.join(relative)).unwrap(),
            b"unclaimed-database-or-metadata"
        );
        assert!(!f.target.exists());
    }
}

#[cfg_attr(test, test)]
fn existing_target_metadata_is_not_permission_to_enter_a_nonempty_generation() {
    for relative in [
        "loongport.db",
        "vault.json",
        "protection.json",
        "logs/prior.log",
    ] {
        let f = Fixture::new(false);
        f.fill();
        write(
            &f.target.join(relative),
            b"existing-not-this-staging-operation",
        );
        assert!(
            PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next)
                .is_err(),
            "{relative}"
        );
        assert_eq!(
            std::fs::read(f.target.join(relative)).unwrap(),
            b"existing-not-this-staging-operation"
        );
        assert!(!f.target.join("workbuddy_accounts.json").exists());
    }
}

#[cfg(unix)]
#[cfg_attr(test, test)]
fn neutral_owner_execute_is_preserved_without_inheriting_group_or_other_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(false);
    f.fill();
    let script = "skills/synthetic/scripts/run.sh";
    write(&f.data.join(script), b"#!/bin/sh\nprintf synthetic\n");
    std::fs::set_permissions(f.data.join(script), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(
        f.data.join("opaque.bin"),
        std::fs::Permissions::from_mode(0o611),
    )
    .unwrap();
    std::fs::set_permissions(
        f.data.join("workbuddy_accounts.json"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let prepared = f.stage();
    let evidence = prepared
        .evidence()
        .find(|e| e.relative_path() == Path::new(script))
        .unwrap();
    assert!(evidence.source_owner_execute());
    assert!(evidence.target_owner_execute());
    for relative in ["opaque.bin", "workbuddy_accounts.json"] {
        let evidence = prepared
            .evidence()
            .find(|e| e.relative_path() == Path::new(relative))
            .unwrap();
        assert!(!evidence.source_owner_execute());
        assert!(!evidence.target_owner_execute());
    }

    assert_eq!(
        std::fs::metadata(f.target.join(script))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(f.target.join("opaque.bin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "other execute does not grant owner execute"
    );
    assert_eq!(
        std::fs::metadata(f.target.join("workbuddy_accounts.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "credential owner remains private nonexecutable"
    );
    assert_eq!(
        std::fs::metadata(f.data.join(script))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::read(f.target.join(script)).unwrap(),
        b"#!/bin/sh\nprintf synthetic\n"
    );
    prepared.verify_source().unwrap();
    prepared.verify_target().unwrap();
}

#[cfg(unix)]
#[cfg_attr(test, test)]
fn source_neutral_execute_change_invalidates_the_captured_image() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(false);
    f.fill();
    std::fs::set_permissions(
        f.data.join("opaque.bin"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let prepared = f.stage();
    std::fs::set_permissions(
        f.data.join("opaque.bin"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(
        prepared.verify_source().is_err(),
        "source behavior changed without a byte change"
    );
}

#[cfg(unix)]
#[cfg_attr(test, test)]
fn target_neutral_permissions_are_part_of_readback_not_just_content_hashes() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o700, 0o644, 0o611] {
        let f = Fixture::new(false);
        f.fill();
        std::fs::set_permissions(
            f.data.join("opaque.bin"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let prepared = f.stage();
        std::fs::set_permissions(
            f.target.join("opaque.bin"),
            std::fs::Permissions::from_mode(mode),
        )
        .unwrap();
        assert!(
            prepared.verify_target().is_err(),
            "target permission mode {mode:o}"
        );
    }
}

fn target_tree(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[cfg_attr(test, test)]
fn existing_encrypted_generation_is_reopened_without_writes_or_reencoding() {
    let f = Fixture::new(true);
    f.fill();
    let prepared = f.stage();
    let before = target_tree(&f.target);
    let original: Vec<_> = prepared
        .evidence()
        .map(|e| {
            (
                e.source_path().to_path_buf(),
                *e.source_hash(),
                *e.target_hash(),
                e.source_owner_execute(),
                e.target_owner_execute(),
            )
        })
        .collect();
    let inspected = PreparedGenerationFiles::inspect_existing(
        &f.data, &f.device, &f.target, &f.source, &f.next,
    )
    .unwrap();
    inspected.verify_source().unwrap();
    inspected.verify_target().unwrap();
    assert_eq!(target_tree(&f.target), before);
    let reopened: Vec<_> = inspected
        .evidence()
        .map(|e| {
            (
                e.source_path().to_path_buf(),
                *e.source_hash(),
                *e.target_hash(),
                e.source_owner_execute(),
                e.target_owner_execute(),
            )
        })
        .collect();
    assert_eq!(
        reopened, original,
        "must retain actual authenticated target images, not random-nonce reencoding"
    );
    assert!(
        PreparedGenerationFiles::stage(&f.data, &f.device, &f.target, &f.source, &f.next).is_err(),
        "read-only resume does not relax stage target admission"
    );
}

#[cfg_attr(test, test)]
fn existing_plaintext_generation_preserves_raw_settings_and_neutral_source_evidence() {
    let f = Fixture::new(false);
    f.fill();
    let _first = f.stage();
    let plain_target = f.temp.path().join("plain-target");
    let prepared =
        PreparedGenerationFiles::stage(&f.target, &f.target, &plain_target, &f.next, &f.source)
            .unwrap();
    let before = target_tree(&plain_target);
    let inspected = PreparedGenerationFiles::inspect_existing(
        &f.target,
        &f.target,
        &plain_target,
        &f.next,
        &f.source,
    )
    .unwrap();
    inspected.verify_source().unwrap();
    inspected.verify_target().unwrap();
    assert_eq!(target_tree(&plain_target), before);
    assert_eq!(
        raw_fields(
            &std::fs::read(plain_target.join("backups/vault-recovery/settings.json")).unwrap()
        ),
        raw_fields(SETTINGS)
    );
    assert_eq!(inspected.evidence().count(), prepared.evidence().count());
}

#[cfg_attr(test, test)]
fn existing_inspection_rejects_mutated_added_missing_and_absent_target_without_writes() {
    for change in ["replace", "add", "remove", "source"] {
        let f = Fixture::new(false);
        f.fill();
        let _prepared = f.stage();
        match change {
            "replace" => write(&f.target.join("opaque.bin"), b"changed"),
            "add" => write(&f.target.join("unclaimed.bin"), b"unexpected"),
            "remove" => std::fs::remove_file(f.target.join("workbuddy_accounts.json")).unwrap(),
            "source" => write(&f.data.join("workbuddy_accounts.json"), b"new-source-value"),
            _ => unreachable!(),
        }
        let before = target_tree(&f.target);
        assert!(
            PreparedGenerationFiles::inspect_existing(
                &f.data, &f.device, &f.target, &f.source, &f.next
            )
            .is_err(),
            "{change}"
        );
        assert_eq!(
            target_tree(&f.target),
            before,
            "{change} must not repair or republish"
        );
    }
    let f = Fixture::new(false);
    f.fill();
    assert!(PreparedGenerationFiles::inspect_existing(
        &f.data, &f.device, &f.target, &f.source, &f.next
    )
    .is_err());
    assert!(!f.target.exists());
}

#[cfg(unix)]
#[cfg_attr(test, test)]
fn existing_inspection_rejects_changed_execute_bits_without_chmod_or_rewrite() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let f = Fixture::new(false);
    f.fill();
    std::fs::set_permissions(
        f.data.join("opaque.bin"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let _prepared = f.stage();
    let path = f.target.join("opaque.bin");
    let initial = std::fs::metadata(&path).unwrap();
    let _inspected = PreparedGenerationFiles::inspect_existing(
        &f.data, &f.device, &f.target, &f.source, &f.next,
    )
    .unwrap();
    let after = std::fs::metadata(&path).unwrap();
    assert_eq!(
        (
            after.ino(),
            after.mtime(),
            after.mtime_nsec(),
            after.permissions().mode()
        ),
        (
            initial.ino(),
            initial.mtime(),
            initial.mtime_nsec(),
            initial.permissions().mode()
        )
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(PreparedGenerationFiles::inspect_existing(
        &f.data, &f.device, &f.target, &f.source, &f.next
    )
    .is_err());
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[cfg_attr(test, test)]
fn shared_settings_owner_equivalence_preserves_unknown_raw_fields_and_strict_authentication() {
    let (plain, encrypted) = contexts();
    let encoded =
        crate::settings::transcode_settings_with_storage(SETTINGS, &plain, &encrypted).unwrap();
    assert!(crate::settings::settings_images_equal_with_storage(
        SETTINGS, &plain, &encoded, &encrypted
    )
    .unwrap());
    let changed = std::str::from_utf8(SETTINGS)
        .unwrap()
        .replace("18446744073709551615", "18446744073709551614");
    assert!(!crate::settings::settings_images_equal_with_storage(
        changed.as_bytes(),
        &plain,
        &encoded,
        &encrypted
    )
    .unwrap());
    assert!(crate::settings::settings_images_equal_with_storage(
        br#"{"foreign":1,"foreign":2}"#,
        &plain,
        &encoded,
        &encrypted
    )
    .is_err());
    assert!(crate::settings::settings_images_equal_with_storage(
        SETTINGS, &encrypted, &encoded, &encrypted
    )
    .is_err());
    assert!(crate::settings::settings_images_equal_with_storage(
        SETTINGS, &plain, &encoded, &plain
    )
    .is_err());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run_case(case: &str) {
    match case {
        "plain_to_encrypted" => {
            explicit_plaintext_generation_stages_original_owned_device_and_recovery_owners()
        }
        "encrypted_to_plain" => {
            encrypted_to_plaintext_preserves_settings_raw_fields_and_neutral_tree()
        }
        "separate_device" => {
            separate_device_root_only_contributes_original_fixed_device_and_settings_inventory()
        }
        "source_fence" => {
            source_inventory_additions_removals_and_changes_in_every_role_are_rejected()
        }
        "target_fence" => target_inventory_additions_removals_and_changes_are_rejected(),
        "exclusions" => metadata_database_and_original_diagnostics_are_never_neutral_copies(),
        "collision" => duplicate_destination_conflict_fails_before_any_target_publication(),
        "strict_codec" => wrong_mode_or_key_and_invalid_settings_fail_before_target_publication(),
        "target_overlap" => target_collision_and_overlapping_roots_never_overwrite_existing_bytes(),
        #[cfg(unix)]
        "path_types" => {
            aliases_special_files_and_linked_neutral_files_fail_closed_without_following()
        }
        "reserved_artifacts" => {
            reserved_recovery_database_and_orphan_backup_sidecars_are_not_neutral_data()
        }
        "nonempty_target" => {
            existing_target_metadata_is_not_permission_to_enter_a_nonempty_generation()
        }
        #[cfg(unix)]
        "neutral_execute" => {
            neutral_owner_execute_is_preserved_without_inheriting_group_or_other_permissions()
        }
        #[cfg(unix)]
        "source_execute_fence" => source_neutral_execute_change_invalidates_the_captured_image(),
        #[cfg(unix)]
        "target_execute_fence" => {
            target_neutral_permissions_are_part_of_readback_not_just_content_hashes()
        }
        "inspect_encrypted" => {
            existing_encrypted_generation_is_reopened_without_writes_or_reencoding()
        }
        "inspect_plaintext" => {
            existing_plaintext_generation_preserves_raw_settings_and_neutral_source_evidence()
        }
        "inspect_changes" => {
            existing_inspection_rejects_mutated_added_missing_and_absent_target_without_writes()
        }
        #[cfg(unix)]
        "inspect_execute" => {
            existing_inspection_rejects_changed_execute_bits_without_chmod_or_rewrite()
        }
        "settings_owner_equivalence" => {
            shared_settings_owner_equivalence_preserves_unknown_raw_fields_and_strict_authentication(
            )
        }
        _ => panic!("unknown generation file case"),
    }
}
