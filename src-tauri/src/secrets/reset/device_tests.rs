//! Key-loss reset uses its real owner, isolated roots and only synthetic ciphertext.
use super::*;
use crate::live::engine::DeviceStore;
use crate::secrets::{
    owned_file::DeviceFile,
    testing::{MemoryKeyStore, TestHome},
};

struct Fixture {
    _home: TestHome,
    root: PathBuf,
    device: DeviceStore,
    original: Vec<(String, Vec<u8>)>,
    client: PathBuf,
}
impl Fixture {
    fn new(separate: bool, unknown: bool) -> Self {
        Self::with_layout(separate, unknown, false)
    }
    fn with_layout(separate: bool, unknown: bool, nested: bool) -> Self {
        let home = TestHome::new().unwrap();
        if nested {
            std::env::set_var("CC_SWITCH_TEST_HOME", home.path().join("device-home"));
        }
        crate::settings::reload_settings().unwrap();
        let device = DeviceStore::for_device();
        let root = if nested {
            home.path().join("device-home")
        } else if separate {
            home.path().join("shared-data")
        } else {
            device.root().to_path_buf()
        };
        let session = crate::secrets::session::SecretSession::open(
            &root,
            &MemoryKeyStore::default(),
            Some("synthetic old password"),
        )
        .unwrap();
        crate::settings::unlock_settings_for_test(session.clone()).unwrap();
        let db = crate::database::Database::init_with_secrets(session.clone()).unwrap();
        // Exercise the current reset owner at its supported schema17; ordinary
        // schema20 startup/reset admission is intentionally not activated here.
        db.save_provider(
            "codex",
            &crate::provider::Provider::with_id(
                "retained".into(),
                "Synthetic".into(),
                serde_json::json!({"auth":{},"config":""}),
                None,
            ),
        )
        .unwrap();
        session.complete_migration().unwrap();
        let mut original = Vec::new();
        let vault = session.read().unwrap();
        let state = if unknown {
            b"unknown encrypted operation disposition".to_vec()
        } else {
            serde_json::to_vec(&serde_json::json!({"version":1,"apps":{"codex":{"mode":"direct"}}}))
                .unwrap()
        };
        for (name, plain) in [
            (
                crate::secrets::owned_file::DEVICE_STATE_FILE,
                state.as_slice(),
            ),
            ("codex-login-stash.json", b"synthetic private native login"),
        ] {
            let file = DeviceFile::registered(name).unwrap();
            let ciphertext = file.encode(&vault, plain).unwrap();
            crate::config_file_io::write_durable(&device.path_for(&file), &ciphertext).unwrap();
            original.push((name.into(), ciphertext));
        }
        drop(vault);
        drop(db);
        let client = home.path().join("client-auth.json");
        std::fs::write(&client, b"existing native client remains unchanged").unwrap();
        Self {
            _home: home,
            root,
            device,
            original,
            client,
        }
    }
    fn stage_pending(&mut self) -> PathBuf {
        let session = crate::secrets::session::SecretSession::open_existing(
            &self.root,
            &MemoryKeyStore::default(),
            Some("synthetic old password"),
        )
        .unwrap();
        let vault = session.read().unwrap();
        let guard = crate::live::engine::lock_app("codex");
        let stash = DeviceFile::registered("codex-login-stash.json").unwrap();
        let stash_patch = crate::live::patch::WholeFile::Write(
            stash.encode(&vault, br#"{"logins":{}}"#).unwrap(),
        );
        let client_patch =
            crate::live::patch::WholeFile::Write(b"synthetic next client login".to_vec());
        crate::mode::operation::failpoint::crash_at(Some("pending"));
        let result = crate::mode::operation::run(
            &self.device,
            &vault,
            &guard,
            crate::mode::state::op::APPLY,
            &[
                crate::mode::operation::FileChange {
                    file: crate::live::engine::LiveFile::private(&self.client),
                    patch: &client_patch,
                },
                crate::mode::operation::FileChange {
                    file: crate::live::engine::LiveFile::private(self.device.path_for(&stash)),
                    patch: &stash_patch,
                },
            ],
            Default::default(),
            &|_| Ok(()),
        );
        crate::mode::operation::failpoint::crash_at(None);
        assert!(result.is_err());
        let pending = crate::mode::state::pending(&self.device, &vault, "codex")
            .unwrap()
            .unwrap();
        assert!(!pending.published);
        let client_stage = pending.files[0].staged.as_ref().unwrap().clone();
        let device_stage = pending.files[1].staged.as_ref().unwrap();
        let stage_bytes = std::fs::read(device_stage).unwrap();
        assert!(stash.decode(&vault, &stage_bytes).is_ok());
        let mode = self
            .original
            .iter_mut()
            .find(|(name, _)| name == "live-state.json")
            .unwrap();
        mode.1 = std::fs::read(self.device.state_path()).unwrap();
        self.original.push((
            device_stage.file_name().unwrap().to_str().unwrap().into(),
            stage_bytes,
        ));
        drop(guard);
        drop(vault);
        drop(session);
        client_stage
    }
    fn assert_reset_archive(&self, archive_path: &Path) {
        let archive: RecoveryArchive =
            serde_json::from_slice(&std::fs::read(archive_path).unwrap()).unwrap();
        let next = VaultContext::from_password(archive.metadata.clone(), "synthetic new password")
            .unwrap();
        let plain = next
            .open(&["local", "reset-archive", &archive.id], &archive.content)
            .unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(plain.as_slice())).unwrap();
        for (name, before) in &self.original {
            let mut archived = Vec::new();
            zip.by_name(&format!("device/{name}"))
                .unwrap()
                .read_to_end(&mut archived)
                .unwrap();
            assert_eq!(&archived, before);
            assert!(
                zip.by_name(&format!("data/{name}")).is_err(),
                "same-root device bytes must not be archived twice"
            );
            assert!(!self.device.root().join(name).exists());
        }
        assert_eq!(
            std::fs::read(&self.client).unwrap(),
            b"existing native client remains unchanged"
        );
        assert!(!pending(&self.root).unwrap());
        let session = crate::secrets::session::SecretSession::open_existing(
            &self.root,
            &MemoryKeyStore::default(),
            Some("synthetic new password"),
        )
        .unwrap();
        assert!(
            crate::mode::current::validate_direct_mode(
                &self.device,
                &session.read().unwrap(),
                &crate::app_config::AppType::Codex
            )
            .is_err(),
            "reset does not claim an old native operation completed"
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_archives_device_ciphertext_and_leaves_mode_unknown() {
    for separate in [false, true] {
        for unknown in [false, true] {
            let fixture = Fixture::new(separate, unknown);
            let preview = preview(&fixture.root).unwrap();
            let archive = reset(
                &fixture.root,
                &preview.fingerprint,
                "synthetic new password",
            )
            .unwrap();
            fixture.assert_reset_archive(&archive);
        }
    }
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify() -> Result<(), AppError> {
    reset_archives_device_ciphertext_and_leaves_mode_unknown();
    println!(
        "PASS explicit reset archives same/separate device roots, including unknown disposition"
    );
    device_reset_resumes_each_existing_commit_and_device_cleanup_boundary();
    println!("PASS explicit reset resumes all directory/device cleanup boundaries");
    device_source_changes_invalidate_reset_preview_without_cleanup();
    println!("PASS device bytes/membership changes invalidate reset preview");
    device_changes_during_committed_cleanup_remain_recoverable_and_untouched();
    println!("PASS changed device generations block cleanup without losing reset intent");
    device_root_binding_cannot_be_redirected_during_reset_recovery();
    println!("PASS reset binds the fixed device root across recovery");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn device_reset_resumes_each_existing_commit_and_device_cleanup_boundary() {
    for separate in [false, true] {
        let mut points = vec![
            Checkpoint::Published,
            Checkpoint::Archived,
            Checkpoint::Installed,
            Checkpoint::Settings,
            Checkpoint::CleanupCommitted,
        ];
        if separate {
            points.push(Checkpoint::Device(0));
        }
        for at in points {
            let mut fixture = Fixture::new(separate, false);
            let client_stage = fixture.stage_pending();
            let preview = preview(&fixture.root).unwrap();
            assert!(reset_with_hook(
                &fixture.root,
                &preview.fingerprint,
                "synthetic new password",
                &mut |point| {
                    if point == at {
                        Err(invalid())
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err());
            assert!(pending(&fixture.root).unwrap());
            let intent: Intent = serde_json::from_slice(
                &std::fs::read(intent_path(&fixture.root).unwrap()).unwrap(),
            )
            .unwrap();
            let archive = locations(&fixture.root, &intent.id).unwrap().2;
            assert!(recover(&fixture.root, None).is_err());
            assert!(recover(&fixture.root, Some("wrong synthetic password")).is_err());
            assert_eq!(
                std::fs::read(&fixture.client).unwrap(),
                b"existing native client remains unchanged"
            );
            recover(&fixture.root, Some("synthetic new password")).unwrap();
            fixture.assert_reset_archive(&archive);
            assert_eq!(
                std::fs::read(client_stage).unwrap(),
                b"synthetic next client login"
            );
        }
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn device_source_changes_invalidate_reset_preview_without_cleanup() {
    for membership in [false, true] {
        let fixture = Fixture::new(true, false);
        let preview = preview(&fixture.root).unwrap();
        let name = if membership {
            "codex-catalog-history.json"
        } else {
            "live-state.json"
        };
        let path = fixture
            .device
            .path_for(&DeviceFile::registered(name).unwrap());
        std::fs::write(&path, b"new external device generation").unwrap();
        let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
        assert!(
            matches!(reset(&fixture.root, &preview.fingerprint, "synthetic new password"), Err(AppError::Config(ref code)) if code == "secret.reset_source_changed")
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"new external device generation"
        );
        assert_eq!(
            std::fs::read(fixture.root.join("vault.json")).unwrap(),
            metadata
        );
        assert!(!pending(&fixture.root).unwrap());
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn device_changes_during_committed_cleanup_remain_recoverable_and_untouched() {
    for at in [Checkpoint::CleanupCommitted, Checkpoint::Device(0)] {
        let fixture = Fixture::new(true, true);
        let preview = preview(&fixture.root).unwrap();
        assert!(reset_with_hook(
            &fixture.root,
            &preview.fingerprint,
            "synthetic new password",
            &mut |point| {
                if point == at {
                    Err(invalid())
                } else {
                    Ok(())
                }
            }
        )
        .is_err());
        let intent: Intent =
            serde_json::from_slice(&std::fs::read(intent_path(&fixture.root).unwrap()).unwrap())
                .unwrap();
        let archive = locations(&fixture.root, &intent.id).unwrap().2;
        let path = fixture.device.state_path();
        std::fs::write(&path, b"externally replaced mode after reset commit").unwrap();
        assert!(recover(&fixture.root, Some("synthetic new password")).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"externally replaced mode after reset commit"
        );
        assert!(pending(&fixture.root).unwrap());
        // Only restoring the fixture's exact captured generation allows cleanup.
        std::fs::write(
            &path,
            &fixture
                .original
                .iter()
                .find(|(name, _)| name == "live-state.json")
                .unwrap()
                .1,
        )
        .unwrap();
        recover(&fixture.root, Some("synthetic new password")).unwrap();
        fixture.assert_reset_archive(&archive);
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn device_root_binding_cannot_be_redirected_during_reset_recovery() {
    let fixture = Fixture::new(true, true);
    let preview = preview(&fixture.root).unwrap();
    assert!(reset_with_hook(
        &fixture.root,
        &preview.fingerprint,
        "synthetic new password",
        &mut |point| {
            if point == Checkpoint::Published {
                Err(invalid())
            } else {
                Ok(())
            }
        }
    )
    .is_err());
    let intent: Intent =
        serde_json::from_slice(&std::fs::read(intent_path(&fixture.root).unwrap()).unwrap())
            .unwrap();
    let archive = locations(&fixture.root, &intent.id).unwrap().2;
    {
        let other = TestHome::new().unwrap();
        assert!(recover(&fixture.root, Some("synthetic new password")).is_err());
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
    }
    recover(&fixture.root, Some("synthetic new password")).unwrap();
    fixture.assert_reset_archive(&archive);
}

fn legacy_intent_at_archived(fixture: &Fixture) -> PathBuf {
    let preview = preview(&fixture.root).unwrap();
    assert!(reset_with_hook(
        &fixture.root,
        &preview.fingerprint,
        "synthetic new password",
        &mut |point| {
            if point == Checkpoint::Archived {
                Err(invalid())
            } else {
                Ok(())
            }
        }
    )
    .is_err());
    let intent: Intent =
        serde_json::from_slice(&std::fs::read(intent_path(&fixture.root).unwrap()).unwrap())
            .unwrap();
    let next =
        VaultContext::from_password(intent.metadata.clone(), "synthetic new password").unwrap();
    let plain = next
        .open(&["local", "reset-intent", &intent.id], &intent.body)
        .unwrap();
    let mut manifest: Manifest = serde_json::from_slice(&plain).unwrap();
    let (stage, previous, archive_path) = locations(&fixture.root, &intent.id).unwrap();
    let mut archive: RecoveryArchive =
        serde_json::from_slice(&std::fs::read(&archive_path).unwrap()).unwrap();
    let contents = next
        .open(&["local", "reset-archive", &intent.id], &archive.content)
        .unwrap();
    let mut old_zip = zip::ZipArchive::new(Cursor::new(contents.as_slice())).unwrap();
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().unix_permissions(0o600);
    let nested = fixture.device.root().strip_prefix(&fixture.root).ok();
    let mut original_settings = Vec::new();
    for index in 0..old_zip.len() {
        let mut entry = old_zip.by_index(index).unwrap();
        let mut name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == "device-settings.json" {
            original_settings = bytes.clone();
        }
        if let Some(relative) = name.strip_prefix("device/") {
            let prefix =
                nested.expect("legacy device entries belong inside this fixture's data root");
            let data_relative = prefix.join(relative);
            name = format!(
                "data/{}",
                data_relative.to_string_lossy().replace('\\', "/")
            );
            crate::config_file_io::write_durable(&stage.join(&data_relative), &bytes).unwrap();
        }
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    archive.content = next
        .seal(
            &["local", "reset-archive", &intent.id],
            &zip.finish().unwrap().into_inner(),
        )
        .unwrap();
    let archive_bytes = serde_json::to_vec(&archive).unwrap();
    crate::config_file_io::write_durable(&archive_path, &archive_bytes).unwrap();
    manifest.device = None;
    manifest.archive_hash = hash(&archive_bytes);
    manifest.fingerprint = fingerprint_with_device(
        &previous,
        &snapshot(&previous).unwrap(),
        &original_settings,
        None,
    )
    .unwrap();
    publish_intent(&fixture.root, &intent.id, &manifest, &next).unwrap();
    archive_path
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_separate_empty_device_inventory_recovers_after_data_root_rename() {
    let mut fixture = Fixture::new(true, false);
    for (name, _) in &fixture.original {
        std::fs::remove_file(
            fixture
                .device
                .path_for(&DeviceFile::registered(name).unwrap()),
        )
        .unwrap();
    }
    fixture.original.clear();
    let archive = legacy_intent_at_archived(&fixture);
    assert!(!fixture.root.exists());
    recover(&fixture.root, Some("synthetic new password")).unwrap();
    fixture.assert_reset_archive(&archive);
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn legacy_nested_device_ciphertext_is_not_installed_under_the_new_key() {
    let fixture = Fixture::with_layout(false, true, true);
    legacy_intent_at_archived(&fixture);
    assert!(!fixture.root.exists());
    let result = recover(&fixture.root, Some("synthetic new password"));
    assert!(
        result.is_err(),
        "legacy nested device assets must retain the reset barrier"
    );
    assert!(pending(&fixture.root).unwrap());
    assert!(
        !fixture.root.exists(),
        "legacy stage containing old-key device data must not install"
    );
    assert_eq!(
        std::fs::read(&fixture.client).unwrap(),
        b"existing native client remains unchanged"
    );
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn cleanup_detects_new_members_and_recreated_removed_members_in_the_same_call() {
    for recreate in [false, true] {
        let fixture = Fixture::new(true, true);
        let preview = preview(&fixture.root).unwrap();
        let name = if recreate {
            "codex-login-stash.json"
        } else {
            "codex-catalog-history.json"
        };
        let path = fixture
            .device
            .path_for(&DeviceFile::registered(name).unwrap());
        let result = reset_with_hook(
            &fixture.root,
            &preview.fingerprint,
            "synthetic new password",
            &mut |point| {
                if point == Checkpoint::Device(0) {
                    std::fs::write(&path, b"concurrent new device generation").unwrap();
                }
                Ok(())
            },
        );
        assert!(
            matches!(result, Err(AppError::Config(ref code)) if code == "secret.reset_source_changed"),
            "same-call membership change cannot complete reset: {result:?}"
        );
        assert!(pending(&fixture.root).unwrap());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"concurrent new device generation"
        );
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn oversized_device_manifest_never_publishes_an_unreadable_reset_intent() {
    let fixture = Fixture::new(true, true);
    let session = crate::secrets::session::SecretSession::open_existing(
        &fixture.root,
        &MemoryKeyStore::default(),
        Some("synthetic old password"),
    )
    .unwrap();
    let vault = session.read().unwrap();
    for index in 0..1000 {
        let name = format!(
            "{}/{index:064x}.backup",
            crate::secrets::owned_file::DEVICE_BACKUP_DIR
        );
        let file = DeviceFile::registered(name).unwrap();
        crate::config_file_io::write_durable(
            &fixture.device.path_for(&file),
            &file.encode(&vault, b"synthetic backup").unwrap(),
        )
        .unwrap();
    }
    drop(vault);
    drop(session);
    let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
    let preview = preview(&fixture.root).unwrap();
    let published = std::cell::Cell::new(false);
    let result = reset_with_hook(
        &fixture.root,
        &preview.fingerprint,
        "synthetic new password",
        &mut |point| {
            if point == Checkpoint::Published {
                published.set(true);
                Err(invalid())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    if published.get() {
        assert!(
            pending_record(&fixture.root).is_err(),
            "oversized published intent is unreadable to its recovery owner"
        );
    }
    assert!(
        !published.get(),
        "reset must check encoded intent size before publication"
    );
    assert!(!intent_path(&fixture.root).unwrap().exists());
    assert_eq!(
        std::fs::read(fixture.root.join("vault.json")).unwrap(),
        metadata
    );
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_review_cases() -> Result<(), AppError> {
    let legacy = std::panic::catch_unwind(
        legacy_separate_empty_device_inventory_recovers_after_data_root_rename,
    );
    let nested = std::panic::catch_unwind(
        legacy_nested_device_ciphertext_is_not_installed_under_the_new_key,
    );
    let membership = std::panic::catch_unwind(
        cleanup_detects_new_members_and_recreated_removed_members_in_the_same_call,
    );
    let size = std::panic::catch_unwind(
        oversized_device_manifest_never_publishes_an_unreadable_reset_intent,
    );
    assert!(
        legacy.is_ok() && nested.is_ok() && membership.is_ok() && size.is_ok(),
        "reset recovery review regressions"
    );
    println!(
        "PASS legacy separate/nested roots, concurrent cleanup membership and bounded reset intent"
    );
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_archives_real_pending_device_staging_without_touching_client_staging() {
    let outcomes = [false, true].map(|separate| {
        std::panic::catch_unwind(|| {
            let mut fixture = Fixture::new(separate, false);
            let client_stage = fixture.stage_pending();
            let preview = preview(&fixture.root).unwrap();
            let archive = reset(
                &fixture.root,
                &preview.fingerprint,
                "synthetic new password",
            )
            .unwrap();
            assert_eq!(
                std::fs::read(&client_stage).unwrap(),
                b"synthetic next client login"
            );
            fixture.assert_reset_archive(&archive);
        })
    });
    assert!(
        outcomes.iter().all(Result::is_ok),
        "real pending device stages must be archived in both root layouts"
    );
}

#[cfg(feature = "test-hooks")]
pub(crate) fn verify_pending_stage() -> Result<(), AppError> {
    reset_archives_real_pending_device_staging_without_touching_client_staging();
    reset_staging_membership_is_limited_to_registered_adjacent_names();
    println!("PASS explicit reset archives real pending device stages and preserves external client staging");
    Ok(())
}

#[cfg_attr(test, test)]
#[cfg_attr(test, serial_test::serial)]
fn reset_staging_membership_is_limited_to_registered_adjacent_names() {
    let fixture = Fixture::new(true, false);
    let session = crate::secrets::session::SecretSession::open_existing(
        &fixture.root,
        &MemoryKeyStore::default(),
        Some("synthetic old password"),
    )
    .unwrap();
    let vault = session.read().unwrap();
    let backup = DeviceFile::registered(format!(
        "{}/{:064x}.backup",
        crate::secrets::owned_file::DEVICE_BACKUP_DIR,
        42
    ))
    .unwrap();
    let staged = crate::config_file_io::stage_write(
        &fixture.device.path_for(&backup),
        &backup.encode(&vault, b"synthetic original").unwrap(),
        Some(0o600),
        true,
    )
    .unwrap();
    let observed = DeviceSnapshot::collect(fixture.device.root()).unwrap();
    let name = staged
        .tmp_path()
        .strip_prefix(fixture.device.root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert!(observed.contains_key(&name));
    // Ordinary generation inventory still rejects opaque staged backup bytes.
    assert!(crate::secrets::files::device_file_paths(fixture.device.root()).is_err());
    for name in [
        "codex-login-stash.json.tmp.1.2",
        "codex-login-stash.json.tmp.1.2.3.4",
        "codex-login-stash.json.tmp.1.x.3",
        "other.json.tmp.1.2.3",
        "../codex-login-stash.json.tmp.1.2.3",
        "codex-login-stash.json.tmp.1.2.3/extra",
    ] {
        assert!(
            crate::secrets::files::device_reset_file(name).is_err(),
            "{name}"
        );
    }
    let unrelated = fixture.device.root().join("other.json.tmp.1.2.3");
    std::fs::write(&unrelated, b"unrelated device directory content").unwrap();
    assert_eq!(
        DeviceSnapshot::collect(fixture.device.root()).unwrap(),
        observed
    );
    #[cfg(unix)]
    {
        let link = fixture
            .device
            .root()
            .join("codex-login-stash.json.tmp.1.2.3");
        std::os::unix::fs::symlink(&fixture.client, &link).unwrap();
        assert!(DeviceSnapshot::collect(fixture.device.root()).is_err());
        assert_eq!(
            std::fs::read(&fixture.client).unwrap(),
            b"existing native client remains unchanged"
        );
    }
}
