#[test]
#[serial_test::serial]
fn rotation_reencrypts_device_files_for_equal_and_distinct_roots() {
    use crate::secrets::owned_file::DeviceFile;
    const CANARY: &[u8] = b"device-transition-canary\0\xff";
    for separate in [false, true] {
        let fixture = Fixture::with_separate_data_root(separate);
        let device_root = fixture._temporary.path().join(crate::APP_DIR_NAME);
        let old = {
            let current = fixture.db.secrets.read().unwrap();
            VaultContext::from_key(current.metadata().clone(), current.export_key()).unwrap()
        };
        let names = [
            "live-state.json".to_owned(),
            "codex-login-stash.json".to_owned(),
            "codex-catalog-history.json".to_owned(),
            format!("backups/live-first-write/{}.backup", "a".repeat(64)),
            format!("backups/live-first-write/{}.source", "a".repeat(64)),
        ];
        for name in &names {
            let file = DeviceFile::registered(name).unwrap();
            write_durable(&device_root.join(name), &file.encode(&old, CANARY).unwrap()).unwrap();
        }
        rotate(
            &fixture.db,
            &fixture.store,
            "next device recovery password",
            true,
        )
        .unwrap();
        let next = fixture.db.secrets.read().unwrap();
        for name in &names {
            let file = DeviceFile::registered(name).unwrap();
            let bytes = std::fs::read(device_root.join(name)).unwrap();
            assert_eq!(
                &*file
                    .decode(&next, &bytes)
                    .expect("device ciphertext must use the committed generation"),
                CANARY
            );
            assert!(file.decode(&old, &bytes).is_err());
            if separate {
                assert!(!fixture.root.join(name).exists());
            }
        }
        assert!(fixture
            .store
            .load(&old.metadata().vault_id, &old.metadata().key_id)
            .unwrap()
            .is_none());
    }
}

const DEVICE_CANARY: &[u8] = b"device-transition-canary\0\xff";
const DEVICE_PASSWORD: &str = "next recovery password";

fn device_names() -> Vec<String> {
    vec![
        "live-state.json".into(),
        "codex-login-stash.json".into(),
        "codex-catalog-history.json".into(),
        format!("backups/live-first-write/{}.backup", "a".repeat(64)),
        format!("backups/live-first-write/{}.source", "a".repeat(64)),
    ]
}
fn seed_device(fixture: &Fixture) -> (PathBuf, VaultContext, Vec<Vec<u8>>) {
    let root = fixture._temporary.path().join(crate::APP_DIR_NAME);
    let current = fixture.db.secrets.read().unwrap();
    let old = VaultContext::from_key(current.metadata().clone(), current.export_key()).unwrap();
    let bytes = device_names()
        .iter()
        .map(|name| {
            let bytes = crate::secrets::owned_file::DeviceFile::registered(name)
                .unwrap()
                .encode(&old, DEVICE_CANARY)
                .unwrap();
            write_durable(&root.join(name), &bytes).unwrap();
            bytes
        })
        .collect();
    (root, old, bytes)
}
fn assert_old_key(fixture: &Fixture, old: &VaultContext) {
    assert!(fixture
        .store
        .load(&old.metadata().vault_id, &old.metadata().key_id)
        .unwrap()
        .is_some());
    assert!(fixture.root.join(INTENT).exists());
    assert!(fixture.db.secrets.read().is_err());
}
fn interrupted_manifest(fixture: &Fixture) -> (Intent, VaultContext, Manifest) {
    let intent = fixture.intent();
    let next = VaultContext::from_password(intent.next.metadata.clone(), DEVICE_PASSWORD).unwrap();
    let plaintext = next
        .open(
            &["local", "generation-transition", &intent.id],
            &intent.manifest,
        )
        .unwrap();
    let manifest = serde_json::from_slice(&plaintext).unwrap();
    (intent, next, manifest)
}

#[test]
#[serial_test::serial]
fn device_external_change_keeps_old_key_and_intent() {
    for change in 0..3 {
        let fixture = Fixture::with_separate_data_root(true);
        let (root, old, _) = seed_device(&fixture);
        fixture.interrupt(Checkpoint::Intent);
        let path = root.join("live-state.json");
        if change == 0 {
            write_durable(&path, b"external-device-change").unwrap();
        }
        if change == 1 {
            std::fs::remove_file(&path).unwrap();
        }
        if change == 2 {
            let file = crate::secrets::owned_file::DeviceFile::registered(format!(
                "backups/live-first-write/{}.source",
                "b".repeat(64)
            ))
            .unwrap();
            write_durable(
                &root.join(file.relative_path()),
                &file.encode(&old, DEVICE_CANARY).unwrap(),
            )
            .unwrap();
        }
        let db = std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
        assert!(
            recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).is_err(),
            "changed/missing/extra device member must refuse replay"
        );
        assert_eq!(
            std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            db
        );
        assert_eq!(
            std::fs::read(fixture.root.join("vault.json")).unwrap(),
            metadata
        );
        assert_old_key(&fixture, &old);
        if change == 0 {
            assert_eq!(std::fs::read(&path).unwrap(), b"external-device-change");
        }
        if change == 1 {
            assert!(!path.exists());
        }
    }
}

#[test]
#[serial_test::serial]
fn device_readback_precedes_retirement() {
    use crate::secrets::key_store::KeyStoreError;
    struct RecordingStore<'a> {
        inner: &'a MemoryKeyStore,
        root: &'a Path,
        removed: std::sync::atomic::AtomicBool,
    }
    impl KeyStore for RecordingStore<'_> {
        fn load(&self, v: &str, k: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
            self.inner.load(v, k)
        }
        fn save(&self, v: &str, k: &str, b: &[u8]) -> Result<(), KeyStoreError> {
            self.inner.save(v, k, b)
        }
        fn remove(&self, v: &str, k: &str) -> Result<(), KeyStoreError> {
            self.removed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let saved = read_metadata(self.root).unwrap();
            let next = VaultContext::from_password(saved.metadata, DEVICE_PASSWORD).unwrap();
            let device = crate::config::get_home_dir().join(crate::APP_DIR_NAME);
            for name in device_names() {
                let file = crate::secrets::owned_file::DeviceFile::registered(&name).unwrap();
                assert_eq!(
                    &*file
                        .decode(&next, &std::fs::read(device.join(name)).unwrap())
                        .expect("retirement must follow device readback"),
                    DEVICE_CANARY
                );
            }
            self.inner.remove(v, k)
        }
    }
    for corrupt in [false, true] {
        let fixture = Fixture::with_separate_data_root(true);
        let (root, old, _) = seed_device(&fixture);
        let store = RecordingStore {
            inner: &fixture.store,
            root: &fixture.root,
            removed: Default::default(),
        };
        let result = rotate_with_hook(&fixture.db, &store, DEVICE_PASSWORD, false, &mut |point| {
            if corrupt && point == Checkpoint::Metadata {
                write_durable(&root.join("live-state.json"), b"post-metadata-corruption")?;
            }
            Ok(())
        });
        if corrupt {
            assert!(
                result.is_err(),
                "post-metadata change must prevent retirement"
            );
            assert!(!store.removed.load(std::sync::atomic::Ordering::SeqCst));
            assert_old_key(&fixture, &old);
        } else {
            result.unwrap();
            assert!(store.removed.load(std::sync::atomic::Ordering::SeqCst));
            assert!(!fixture.root.join(INTENT).exists());
        }
    }
}

#[test]
#[serial_test::serial]
fn device_root_binding_cannot_be_retargeted() {
    let fixture = Fixture::with_separate_data_root(true);
    let (root, old, original) = seed_device(&fixture);
    fixture.interrupt(Checkpoint::Intent);
    let home_b = fixture._temporary.path().join("other-home");
    let device_b = home_b.join(crate::APP_DIR_NAME);
    write_durable(&device_b.join("live-state.json"), b"other-home-canary").unwrap();
    std::env::set_var("CC_SWITCH_TEST_HOME", &home_b);
    assert!(
        recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).is_err(),
        "resolved device root must match encrypted capture"
    );
    assert_eq!(
        std::fs::read(device_b.join("live-state.json")).unwrap(),
        b"other-home-canary"
    );
    for (name, bytes) in device_names().iter().zip(original) {
        assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
    }
    assert_old_key(&fixture, &old);
}

#[test]
#[serial_test::serial]
fn same_key_install_preserves_device_ciphertext() {
    for separate in [false, true] {
        let fixture = Fixture::with_separate_data_root(separate);
        let (root, old, original) = seed_device(&fixture);
        #[cfg(unix)]
        let identities = device_names()
            .iter()
            .map(|name| {
                use std::os::unix::fs::MetadataExt;
                let metadata = std::fs::metadata(root.join(name)).unwrap();
                (metadata.dev(), metadata.ino())
            })
            .collect::<Vec<_>>();
        install_with_hook(
            &fixture.db,
            &fixture.store,
            |current| {
                current
                    .with_password(DEVICE_PASSWORD)
                    .map_err(inventory::secret_error)
            },
            true,
            |source, _, next| {
                let mut memory = Connection::open_in_memory().unwrap();
                vault::copy(source, &mut memory)?;
                vault::stamp(&memory, next)?;
                Ok(memory)
            },
            Replacements {
                skills: None,
                settings: None,
            },
            &mut |_| Ok(()),
        )
        .unwrap();
        #[cfg(unix)]
        for (name, identity) in device_names().iter().zip(identities) {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(root.join(name)).unwrap();
            assert_eq!(
                (metadata.dev(), metadata.ino()),
                identity,
                "same-key install must avoid rewriting the device data path"
            );
        }
        let next = fixture.db.secrets.read().unwrap();
        assert_eq!(next.metadata().key_id, old.metadata().key_id);
        for (name, bytes) in device_names().iter().zip(original) {
            assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
            assert_eq!(
                &*crate::secrets::owned_file::DeviceFile::registered(name)
                    .unwrap()
                    .decode(&next, &bytes)
                    .unwrap(),
                DEVICE_CANARY
            );
        }
    }
}

#[test]
#[serial_test::serial]
fn device_stage_tamper_blocks_all_publication() {
    for tamper in 0..3 {
        let fixture = Fixture::with_separate_data_root(true);
        let (root, old, original) = seed_device(&fixture);
        fixture.interrupt(Checkpoint::Intent);
        let (intent, next, mut manifest) = interrupted_manifest(&fixture);
        let index = manifest
            .artifacts
            .iter()
            .position(|a| serde_json::to_value(&a.destination).unwrap()["kind"] == "device")
            .expect("device stage must be captured in the transaction");
        let path = stage_file(&fixture.root, &intent.id, index).unwrap();
        if tamper == 0 {
            write_durable(&path, b"bad-stage").unwrap();
        }
        if tamper == 1 {
            std::fs::remove_file(&path).unwrap();
        }
        if tamper == 2 {
            let bytes = crate::secrets::owned_file::DeviceFile::registered("live-state.json")
                .unwrap()
                .encode(&next, DEVICE_CANARY)
                .unwrap();
            write_durable(&path, &bytes).unwrap();
            manifest.artifacts[index].digest = hash(&bytes);
            let mut intent = intent;
            intent.manifest = next
                .seal(
                    &["local", "generation-transition", &intent.id],
                    &serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            write_durable(
                &fixture.root.join(INTENT),
                &serde_json::to_vec(&intent).unwrap(),
            )
            .unwrap();
        }
        let db = std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
        assert!(recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).is_err());
        assert_eq!(
            std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            db
        );
        assert_eq!(
            std::fs::read(fixture.root.join("vault.json")).unwrap(),
            metadata
        );
        for (name, bytes) in device_names().iter().zip(original) {
            assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
        }
        assert!(fixture
            .store
            .load(&next.metadata().vault_id, &next.metadata().key_id)
            .unwrap()
            .is_none());
        assert_old_key(&fixture, &old);
    }
}

#[test]
#[serial_test::serial]
fn device_transition_recovers_at_every_checkpoint() {
    for separate in [false, true] {
        for point in [
            Checkpoint::Staged,
            Checkpoint::Intent,
            Checkpoint::Database,
            Checkpoint::Artifact(1),
            Checkpoint::Artifact(2),
            Checkpoint::Artifact(3),
            Checkpoint::Artifact(4),
            Checkpoint::Artifact(5),
            Checkpoint::Artifact(6),
            Checkpoint::Metadata,
            Checkpoint::Keys,
        ] {
            let fixture = Fixture::with_separate_data_root(separate);
            let (root, old, original) = seed_device(&fixture);
            fixture.interrupt(point);
            if point == Checkpoint::Staged {
                assert!(fixture.db.secrets.read().is_ok());
                assert!(!fixture.root.join(INTENT).exists());
                assert!(fixture
                    .store
                    .load(&old.metadata().vault_id, &old.metadata().key_id)
                    .unwrap()
                    .is_some());
                for (name, bytes) in device_names().iter().zip(original) {
                    assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
                }
                continue;
            }
            assert!(fixture.root.join(INTENT).exists());
            assert!(fixture.db.secrets.read().is_err());
            let recovered =
                SecretSession::open_existing(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD))
                    .unwrap();
            let next = recovered.read().unwrap();
            for name in device_names() {
                let file = crate::secrets::owned_file::DeviceFile::registered(&name).unwrap();
                let bytes = std::fs::read(root.join(&name)).unwrap();
                assert_eq!(&*file.decode(&next, &bytes).unwrap(), DEVICE_CANARY);
                assert!(file.decode(&old, &bytes).is_err());
                if separate {
                    assert!(!fixture.root.join(name).exists());
                }
            }
            assert!(!fixture.root.join(INTENT).exists());
            assert!(fixture
                .store
                .load(&old.metadata().vault_id, &old.metadata().key_id)
                .unwrap()
                .is_none());
            recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).unwrap();
        }
    }
}

#[test]
#[serial_test::serial]
fn device_legacy_recovery_refuses_uncaptured_members() {
    for present in [false, true] {
        let fixture = Fixture::with_separate_data_root(true);
        let old = if present {
            seed_device(&fixture).1
        } else {
            let current = fixture.db.secrets.read().unwrap();
            VaultContext::from_key(current.metadata().clone(), current.export_key()).unwrap()
        };
        fixture.interrupt(Checkpoint::Intent);
        let (mut intent, next, manifest) = interrupted_manifest(&fixture);
        let mut value = serde_json::to_value(&manifest).unwrap();
        value.as_object_mut().unwrap().remove("roots");
        value["artifacts"]
            .as_array_mut()
            .unwrap()
            .retain(|a| a["destination"]["kind"] != "device");
        intent.version = 1;
        intent.manifest = next
            .seal(
                &["local", "generation-transition", &intent.id],
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        write_durable(
            &fixture.root.join(INTENT),
            &serde_json::to_vec(&intent).unwrap(),
        )
        .unwrap();
        let before = std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let result = recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD));
        if present {
            assert!(
                result.is_err(),
                "format-1 intent never authorized existing device files"
            );
            assert_eq!(
                std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap(),
                before
            );
            assert_old_key(&fixture, &old);
        } else {
            result.unwrap();
            assert!(!fixture.root.join(INTENT).exists());
        }
    }
}

#[test]
#[serial_test::serial]
fn rewrap_leaves_device_ciphertext_identical() {
    use crate::secrets::key_store::KeyStoreError;
    struct FailRemove<'a>(&'a MemoryKeyStore);
    impl KeyStore for FailRemove<'_> {
        fn load(&self, v: &str, k: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
            self.0.load(v, k)
        }
        fn save(&self, v: &str, k: &str, b: &[u8]) -> Result<(), KeyStoreError> {
            self.0.save(v, k, b)
        }
        fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
            Err(KeyStoreError::Unavailable)
        }
    }
    for separate in [false, true] {
        for policy in 0..3 {
            let fixture = Fixture::with_separate_data_root(separate);
            let (root, old, original) = seed_device(&fixture);
            let failing = FailRemove(&fixture.store);
            let result = crate::secrets::rewrap::change_password(
                &fixture.db,
                if policy == 2 {
                    &failing as &dyn KeyStore
                } else {
                    &fixture.store as &dyn KeyStore
                },
                DEVICE_PASSWORD,
                policy == 0,
            );
            if policy == 2 {
                assert!(result.is_err());
                assert!(fixture.db.secrets.read().is_err());
            } else {
                result.unwrap();
            }
            let recovered =
                SecretSession::open_existing(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD))
                    .unwrap();
            let next = recovered.read().unwrap();
            assert_eq!(next.metadata().key_id, old.metadata().key_id);
            for (name, bytes) in device_names().iter().zip(original) {
                assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
                assert_eq!(
                    &*crate::secrets::owned_file::DeviceFile::registered(name)
                        .unwrap()
                        .decode(&next, &bytes)
                        .unwrap(),
                    DEVICE_CANARY
                );
            }
            assert!(!fixture.root.join(".vault-rewrap").exists());
        }
    }
}

#[test]
#[serial_test::serial]
fn device_exact_staged_target_is_valid_interrupted_publication() {
    let fixture = Fixture::with_separate_data_root(true);
    let (root, old, _) = seed_device(&fixture);
    fixture.interrupt(Checkpoint::Intent);
    let (intent, _, manifest) = interrupted_manifest(&fixture);
    let index = manifest
        .artifacts
        .iter()
        .position(|a| serde_json::to_value(&a.destination).unwrap()["kind"] == "device")
        .expect("device source and target must be journaled");
    let relative = serde_json::to_value(&manifest.artifacts[index].destination).unwrap()
        ["relative"]
        .as_str()
        .unwrap()
        .to_owned();
    let target = read_stage(&fixture.root, &intent.id, index, &manifest.artifacts[index]).unwrap();
    write_durable(&root.join(relative), &target).unwrap();
    recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).unwrap();
    let recovered =
        SecretSession::open_existing(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).unwrap();
    for name in device_names() {
        let bytes = std::fs::read(root.join(&name)).unwrap();
        let file = crate::secrets::owned_file::DeviceFile::registered(name).unwrap();
        assert!(file.decode(&old, &bytes).is_err());
        assert_eq!(
            &*file.decode(&recovered.read().unwrap(), &bytes).unwrap(),
            DEVICE_CANARY
        );
    }
}

#[test]
#[serial_test::serial]
fn device_change_after_artifact_hook_is_never_overwritten() {
    let fixture = Fixture::with_separate_data_root(true);
    let (root, old, _) = seed_device(&fixture);
    let mut index = None;
    assert!(rotate_with_hook(
        &fixture.db,
        &fixture.store,
        DEVICE_PASSWORD,
        false,
        &mut |point| {
            if point == Checkpoint::Intent {
                let (_, _, manifest) = interrupted_manifest(&fixture);
                index = manifest
                    .artifacts
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| {
                        serde_json::to_value(&a.destination).unwrap()["kind"] == "device"
                    })
                    .map(|(i, _)| i)
                    .next();
                assert!(
                    index.is_some(),
                    "device publication must belong to the existing coordinator"
                );
            }
            if index.is_some_and(|i| point == Checkpoint::Artifact(i)) {
                write_durable(
                    &root.join(format!(
                        "backups/live-first-write/{}.backup",
                        "a".repeat(64)
                    )),
                    b"after-artifact-canary",
                )?;
                return Err(invalid());
            }
            Ok(())
        }
    )
    .is_err());
    assert!(recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).is_err());
    assert_eq!(
        std::fs::read(root.join(format!(
            "backups/live-first-write/{}.backup",
            "a".repeat(64)
        )))
        .unwrap(),
        b"after-artifact-canary"
    );
    assert_old_key(&fixture, &old);
}

#[test]
#[serial_test::serial]
fn device_invalid_or_duplicate_destinations_fail_before_effects() {
    for case in 0..4 {
        let fixture = Fixture::with_separate_data_root(true);
        let (device_root, old, _) = seed_device(&fixture);
        fixture.interrupt(Checkpoint::Intent);
        let (mut intent, next, manifest) = interrupted_manifest(&fixture);
        let mut value = serde_json::to_value(&manifest).unwrap();
        let artifacts = value["artifacts"].as_array_mut().unwrap();
        let index = artifacts
            .iter()
            .position(|a| a["destination"]["kind"] == "device")
            .expect("typed device destination is required");
        if case == 0 {
            artifacts[index]["destination"]["relative"] = serde_json::json!("../live-state.json");
        }
        if case == 1 {
            artifacts[index]["destination"]["sourceDigest"] = serde_json::json!("bad-digest");
        }
        if case == 2 {
            let mut duplicate = artifacts[index].clone();
            duplicate["destination"]["sourceDigest"] = serde_json::json!("f".repeat(64));
            artifacts.push(duplicate);
            let from = stage_file(&fixture.root, &intent.id, index).unwrap();
            let to = stage_file(&fixture.root, &intent.id, artifacts.len() - 1).unwrap();
            let bytes = std::fs::read(from).unwrap();
            write_durable(&to, &bytes).unwrap();
            // Both descriptors now satisfy the target precondition, so only
            // resolved-path deduplication can reject the second publication.
            let relative = artifacts[index]["destination"]["relative"]
                .as_str()
                .unwrap();
            write_durable(&device_root.join(relative), &bytes).unwrap();
        }
        if case == 3 {
            intent.version = 99;
        }
        intent.manifest = next
            .seal(
                &["local", "generation-transition", &intent.id],
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        write_durable(
            &fixture.root.join(INTENT),
            &serde_json::to_vec(&intent).unwrap(),
        )
        .unwrap();
        let db = std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
        assert!(recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).is_err());
        assert_eq!(
            std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            db
        );
        assert_eq!(
            std::fs::read(fixture.root.join("vault.json")).unwrap(),
            metadata
        );
        assert_old_key(&fixture, &old);
    }
}

#[test]
#[serial_test::serial]
fn device_different_vault_generation_install_reseals_local_members() {
    let fixture = Fixture::with_separate_data_root(true);
    let (root, old, _) = seed_device(&fixture);
    let next = VaultContext::generate()
        .unwrap()
        .with_password(DEVICE_PASSWORD)
        .unwrap();
    let expected = next.metadata().clone();
    install_generation(
        &fixture.db,
        &fixture.store,
        next,
        false,
        |source, current, next| {
            let mut memory = Connection::open_in_memory().unwrap();
            vault::copy(source, &mut memory)?;
            inventory::transform_database(&memory, Some(current), next)?;
            vault::stamp(&memory, next)?;
            Ok(memory)
        },
        None,
    )
    .unwrap();
    let next = fixture.db.secrets.read().unwrap();
    assert_eq!(*next.metadata(), expected);
    assert_ne!(next.metadata().vault_id, old.metadata().vault_id);
    for name in device_names() {
        let file = crate::secrets::owned_file::DeviceFile::registered(&name).unwrap();
        let bytes = std::fs::read(root.join(&name)).unwrap();
        assert_eq!(&*file.decode(&next, &bytes).unwrap(), DEVICE_CANARY);
        assert!(file.decode(&old, &bytes).is_err());
        assert!(!fixture.root.join(name).exists());
    }
}

#[test]
#[serial_test::serial]
fn device_explicit_root_is_pinned_for_interrupted_recovery() {
    let fixture = Fixture::with_separate_data_root(true);
    let device = fixture._temporary.path().join("pinned-device");
    let old = {
        let current = fixture.db.secrets.read().unwrap();
        VaultContext::from_key(current.metadata().clone(), current.export_key()).unwrap()
    };
    for name in device_names() {
        let file = crate::secrets::owned_file::DeviceFile::registered(&name).unwrap();
        write_durable(
            &device.join(name),
            &file.encode(&old, DEVICE_CANARY).unwrap(),
        )
        .unwrap();
    }
    assert!(install_with_roots(
        &fixture.db,
        &fixture.store,
        |current| current
            .rotate_key()
            .and_then(|next| next.with_password(DEVICE_PASSWORD))
            .map_err(inventory::secret_error),
        false,
        |source, current, next| {
            let mut memory = Connection::open_in_memory().unwrap();
            vault::copy(source, &mut memory)?;
            inventory::transform_database(&memory, Some(current), next)?;
            vault::stamp(&memory, next)?;
            Ok(memory)
        },
        Replacements {
            skills: None,
            settings: None
        },
        &device,
        &mut |point| if point == Checkpoint::Intent {
            Err(invalid())
        } else {
            Ok(())
        }
    )
    .is_err());
    let public = std::fs::read_to_string(fixture.root.join(INTENT)).unwrap();
    assert!(!public.contains(device.to_str().unwrap()));
    let other_device = fixture._temporary.path().join("wrong-device");
    assert!(recover_with_device_root(
        &fixture.root,
        &other_device,
        &fixture.store,
        Some(DEVICE_PASSWORD)
    )
    .is_err());
    assert!(!other_device.exists());
    assert_old_key(&fixture, &old);

    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let dest = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }
    let other_data = fixture._temporary.path().join("wrong-data");
    copy_tree(&fixture.root, &other_data);
    let database = std::fs::read(other_data.join(crate::config::DB_FILE_NAME)).unwrap();
    assert!(
        recover_with_device_root(&other_data, &device, &fixture.store, Some(DEVICE_PASSWORD))
            .is_err()
    );
    assert_eq!(
        std::fs::read(other_data.join(crate::config::DB_FILE_NAME)).unwrap(),
        database
    );
    assert_old_key(&fixture, &old);
    recover_with_device_root(
        &fixture.root,
        &device,
        &fixture.store,
        Some(DEVICE_PASSWORD),
    )
    .unwrap();
    let next = VaultContext::from_password(
        read_metadata(&fixture.root).unwrap().metadata,
        DEVICE_PASSWORD,
    )
    .unwrap();
    for name in device_names() {
        let file = crate::secrets::owned_file::DeviceFile::registered(&name).unwrap();
        assert_eq!(
            &*file
                .decode(&next, &std::fs::read(device.join(name)).unwrap())
                .unwrap(),
            DEVICE_CANARY
        );
    }
    assert!(!fixture.root.join(INTENT).exists());
}

#[test]
#[serial_test::serial]
fn device_relative_root_is_rejected_before_staging() {
    let fixture = Fixture::with_separate_data_root(true);
    let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
    let entries = std::fs::read_dir(&fixture.root).unwrap().count();
    assert!(install_with_roots(
        &fixture.db,
        &fixture.store,
        |_| panic!("relative root must fail before key generation"),
        false,
        |_, _, _| panic!("relative root must fail before staging"),
        Replacements {
            skills: None,
            settings: None
        },
        Path::new("."),
        &mut |_| panic!("no publication")
    )
    .is_err());
    assert_eq!(
        std::fs::read(fixture.root.join("vault.json")).unwrap(),
        metadata
    );
    assert_eq!(std::fs::read_dir(&fixture.root).unwrap().count(), entries);
    assert!(fixture.db.secrets.read().is_ok());
}

#[test]
#[serial_test::serial]
#[cfg(unix)]
fn device_recovery_rejects_replaced_root_or_backup_ancestor() {
    use std::os::unix::fs::symlink;
    for backup in [false, true] {
        let fixture = Fixture::with_separate_data_root(true);
        let (root, old, original) = seed_device(&fixture);
        fixture.interrupt(Checkpoint::Intent);
        let path = if backup {
            root.join("backups")
        } else {
            root.clone()
        };
        let outside = fixture._temporary.path().join("moved-device");
        std::fs::rename(&path, &outside).unwrap();
        symlink(&outside, &path).unwrap();
        let database = std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
        assert!(recover(&fixture.root, &fixture.store, Some(DEVICE_PASSWORD)).is_err());
        assert_eq!(
            std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            database
        );
        for (name, bytes) in device_names().iter().zip(original) {
            assert_eq!(std::fs::read(root.join(name)).unwrap(), bytes);
        }
        assert_old_key(&fixture, &old);
    }
}

#[test]
#[serial_test::serial]
fn device_readback_blocks_metadata_publication() {
    let fixture = Fixture::with_separate_data_root(true);
    let (root, old, _) = seed_device(&fixture);
    let metadata = std::fs::read(fixture.root.join("vault.json")).unwrap();
    let mut last = None;
    assert!(rotate_with_hook(
        &fixture.db,
        &fixture.store,
        DEVICE_PASSWORD,
        false,
        &mut |point| {
            if point == Checkpoint::Intent {
                let (_, _, manifest) = interrupted_manifest(&fixture);
                last = manifest
                    .artifacts
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| matches!(a.destination, Destination::Device { .. }))
                    .map(|(i, _)| i)
                    .next_back();
            }
            if last.is_some_and(|i| point == Checkpoint::Artifact(i)) {
                write_durable(&root.join("live-state.json"), b"pre-metadata-change")?;
            }
            Ok(())
        }
    )
    .is_err());
    assert_eq!(
        std::fs::read(fixture.root.join("vault.json")).unwrap(),
        metadata
    );
    assert_eq!(
        std::fs::read(root.join("live-state.json")).unwrap(),
        b"pre-metadata-change"
    );
    assert_old_key(&fixture, &old);
}
