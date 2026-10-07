use super::super::owned_file::{DeviceFile, DEVICE_BACKUP_DIR};
use super::*;
use std::path::Path;

const CANARY: &[u8] = b"device-transition-canary\0\xff";

fn temp_tree() -> tempfile::TempDir {
    // Use a resolved test base so platform temp-dir aliases (e.g. /var on macOS)
    // do not turn valid fixtures into the symlink-under-test cases below.
    // Windows canonicalization adds a verbatim prefix, whose joins normalize
    // parent components before the path validator can inspect the fixture.
    #[cfg(windows)]
    let base = std::env::temp_dir();
    #[cfg(not(windows))]
    let base = std::env::temp_dir().canonicalize().unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn device_names() -> Vec<String> {
    vec![
        "live-state.json".into(),
        "codex-login-stash.json".into(),
        "codex-catalog-history.json".into(),
        format!("{DEVICE_BACKUP_DIR}/{}.backup", "a".repeat(64)),
        format!("{DEVICE_BACKUP_DIR}/{}.source", "a".repeat(64)),
    ]
}

fn install_device_fixture(root: &Path, vault: &VaultContext) -> Vec<(PathBuf, Vec<u8>)> {
    std::fs::create_dir_all(root.join(DEVICE_BACKUP_DIR)).unwrap();
    device_names()
        .into_iter()
        .rev()
        .map(|name| {
            let path = root.join(&name);
            let bytes = DeviceFile::registered(name)
                .unwrap()
                .encode(vault, CANARY)
                .unwrap();
            std::fs::write(&path, &bytes).unwrap();
            (path, bytes)
        })
        .collect()
}

#[test]
fn device_inventory_is_read_only_and_complete() {
    let tree = temp_tree();
    let root = tree.path().join("home/.loongport");
    let vault = VaultContext::generate().unwrap();
    let absent = tree.path().join("missing/device/root");
    assert!(stage_device_files_with_vault(&absent, &vault)
        .unwrap()
        .is_empty());
    assert!(!tree.path().join("missing").exists());
    std::fs::create_dir_all(&root).unwrap();
    assert!(stage_device_files_with_vault(&root, &vault)
        .unwrap()
        .is_empty());
    assert!(!root.join("backups").exists());
    let originals = install_device_fixture(&root, &vault);
    let plans = stage_device_files_with_vault(&root, &vault).unwrap();
    assert_eq!(plans.len(), 5);
    let mut names = device_names();
    names.sort();
    assert_eq!(
        plans
            .iter()
            .map(|plan| plan.file.relative_path().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        names
    );
    for plan in plans {
        assert_eq!(plan.source, root.join(plan.file.relative_path()));
        assert_eq!(std::fs::read(&plan.source).unwrap(), plan.ciphertext);
        assert_eq!(
            &*plan.file.decode(&vault, &plan.ciphertext).unwrap(),
            CANARY
        );
    }
    for (path, original) in originals {
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
    assert!(!root.join("backups/vault-recovery").exists());
}

#[test]
fn device_inventory_never_adopts_owned_or_plaintext() {
    let tree = temp_tree();
    let root = tree.path().join("shared-data");
    let vault = VaultContext::generate().unwrap();
    let originals = install_device_fixture(&root, &vault);
    let excluded = [
        "config.json",
        "codex_oauth_auth.json",
        "zcode_account_profiles.json",
        "backups/backup_123.json",
        "backups/vault-recovery/live-state.json",
        "notes/live-state.json",
    ];
    for name in excluded {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"ordinary-or-unrelated-canary").unwrap();
    }
    assert_eq!(
        stage_device_files_with_vault(&root, &vault).unwrap().len(),
        5
    );
    assert_eq!(
        AUTH_FILES.map(CredentialFile::filename),
        [
            "copilot_auth.json",
            "codex_oauth_auth.json",
            "xai_oauth_auth.json"
        ]
    );
    for (path, original) in originals {
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
    for name in excluded {
        assert_eq!(
            std::fs::read(root.join(name)).unwrap(),
            b"ordinary-or-unrelated-canary"
        );
    }
    let file = DeviceFile::registered("live-state.json").unwrap();
    let path = root.join(file.relative_path());
    let wrong_vault = VaultContext::generate().unwrap();
    let wrong_namespace = vault
        .seal(&["file", "live-state.json", "content"], CANARY)
        .unwrap()
        .into_bytes();
    let wrong_identity = DeviceFile::registered("codex-login-stash.json")
        .unwrap()
        .encode(&vault, CANARY)
        .unwrap();
    let invalid = [
        b"{}".to_vec(),
        CANARY.to_vec(),
        b"lpenc1.invalid".to_vec(),
        file.encode(&wrong_vault, CANARY).unwrap(),
        wrong_namespace,
        wrong_identity,
    ];
    for bytes in invalid {
        std::fs::write(&path, &bytes).unwrap();
        assert!(stage_device_files_with_vault(&root, &vault).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn device_inventory_preserves_unpaired_backup_evidence() {
    for suffix in ["backup", "source"] {
        let tree = temp_tree();
        let root = tree.path().join("device");
        let vault = VaultContext::generate().unwrap();
        std::fs::create_dir_all(root.join(DEVICE_BACKUP_DIR)).unwrap();
        let name = format!("{DEVICE_BACKUP_DIR}/{}.{}", "b".repeat(64), suffix);
        let file = DeviceFile::registered(&name).unwrap();
        let ciphertext = file.encode(&vault, CANARY).unwrap();
        std::fs::write(root.join(&name), &ciphertext).unwrap();
        let plans = stage_device_files_with_vault(&root, &vault).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].file, file);
        assert_eq!(plans[0].ciphertext, ciphertext);
        assert_eq!(std::fs::read(root.join(name)).unwrap(), ciphertext);
    }
}

#[test]
fn device_inventory_rejects_invalid_dedicated_backup_members() {
    for name in [
        "notes.json".to_string(),
        "abc.backup".to_string(),
        format!("{}.backup", "A".repeat(64)),
        format!("{}.source", "g".repeat(64)),
        format!("{}.source", "a".repeat(63)),
        format!("{}.tmp", "a".repeat(64)),
    ] {
        let tree = temp_tree();
        let vault = VaultContext::generate().unwrap();
        let backup = tree.path().join(DEVICE_BACKUP_DIR);
        std::fs::create_dir_all(&backup).unwrap();
        let path = backup.join(&name);
        std::fs::write(&path, b"recovery-evidence").unwrap();
        assert!(
            stage_device_files_with_vault(tree.path(), &vault).is_err(),
            "ignored {name}"
        );
        assert_eq!(std::fs::read(path).unwrap(), b"recovery-evidence");
    }
}

#[test]
fn device_inventory_rejects_registered_nonregular_members_and_file_ancestors() {
    for relative in [
        "live-state.json".to_string(),
        format!("{DEVICE_BACKUP_DIR}/{}.backup", "a".repeat(64)),
    ] {
        let tree = temp_tree();
        let vault = VaultContext::generate().unwrap();
        let path = tree.path().join(relative);
        std::fs::create_dir_all(&path).unwrap();
        assert!(stage_device_files_with_vault(tree.path(), &vault).is_err());
        assert!(path.is_dir());
    }
    for relative in [
        "device",
        "device/backups",
        "device/backups/live-first-write",
    ] {
        let tree = temp_tree();
        let vault = VaultContext::generate().unwrap();
        let path = tree.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not-a-directory").unwrap();
        assert!(stage_device_files_with_vault(&tree.path().join("device"), &vault).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"not-a-directory");
    }
}

#[cfg(unix)]
#[test]
fn device_inventory_rejects_symlinked_backup_ancestor() {
    use std::os::unix::fs::symlink;
    for location in [
        "parent",
        "root",
        "backups",
        "dedicated",
        "top-member",
        "backup-member",
    ] {
        let tree = temp_tree();
        let vault = VaultContext::generate().unwrap();
        let outside = tree.path().join("outside");
        let originals = install_device_fixture(&outside, &vault);
        let root = tree.path().join("home/device");
        std::fs::create_dir_all(root.parent().unwrap()).unwrap();
        match location {
            "parent" => {
                std::fs::remove_dir(root.parent().unwrap()).unwrap();
                let parent_target = tree.path().join("real-home");
                std::fs::create_dir(&parent_target).unwrap();
                std::fs::rename(&outside, parent_target.join("device")).unwrap();
                symlink(&parent_target, root.parent().unwrap()).unwrap();
                assert!(
                    stage_device_files_with_vault(&root, &vault).is_err(),
                    "followed {location}"
                );
                for (path, original) in originals {
                    let relative = path.strip_prefix(&outside).unwrap();
                    assert_eq!(
                        std::fs::read(parent_target.join("device").join(relative)).unwrap(),
                        original
                    );
                }
                continue;
            }
            "root" => symlink(&outside, &root).unwrap(),
            "backups" => {
                std::fs::create_dir(&root).unwrap();
                symlink(outside.join("backups"), root.join("backups")).unwrap();
            }
            "dedicated" => {
                std::fs::create_dir_all(root.join("backups")).unwrap();
                symlink(
                    outside.join(DEVICE_BACKUP_DIR),
                    root.join(DEVICE_BACKUP_DIR),
                )
                .unwrap();
            }
            "top-member" => {
                std::fs::create_dir(&root).unwrap();
                symlink(
                    outside.join("live-state.json"),
                    root.join("live-state.json"),
                )
                .unwrap();
            }
            "backup-member" => {
                std::fs::create_dir_all(root.join(DEVICE_BACKUP_DIR)).unwrap();
                let name = format!("{DEVICE_BACKUP_DIR}/{}.backup", "a".repeat(64));
                symlink(outside.join(&name), root.join(name)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            stage_device_files_with_vault(&root, &vault).is_err(),
            "followed {location}"
        );
        for (path, original) in originals {
            assert_eq!(std::fs::read(path).unwrap(), original);
        }
    }
}

#[cfg(unix)]
#[test]
fn device_inventory_rejects_dangling_symlinks() {
    use std::os::unix::fs::symlink;
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    let root = tree.path().join("device");
    std::fs::create_dir_all(root.join(DEVICE_BACKUP_DIR)).unwrap();
    let member = root.join("live-state.json");
    symlink("absent", &member).unwrap();
    assert!(stage_device_files_with_vault(&root, &vault).is_err());
    assert!(std::fs::symlink_metadata(&member)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn device_inventory_rejects_nonutf8_backup_names() {
    use std::os::unix::ffi::OsStringExt;
    let invalid = std::ffi::OsString::from_vec(vec![b'a', 0xff]);
    assert!(device_backup_file(&invalid).is_err());
}

// Linux filesystems admit these names; macOS rejects them before enumeration.
#[cfg(target_os = "linux")]
#[test]
fn device_inventory_rejects_nonutf8_backup_members() {
    use std::os::unix::ffi::OsStringExt;
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    let root = tree.path().join("device");
    std::fs::create_dir_all(root.join(DEVICE_BACKUP_DIR)).unwrap();
    let invalid = root
        .join(DEVICE_BACKUP_DIR)
        .join(std::ffi::OsString::from_vec(vec![b'a', 0xff]));
    std::fs::write(&invalid, b"recovery-evidence").unwrap();
    assert!(stage_device_files_with_vault(&root, &vault).is_err());
    assert_eq!(std::fs::read(invalid).unwrap(), b"recovery-evidence");
}

#[cfg(unix)]
#[test]
fn device_inventory_does_not_tighten_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    let root = tree.path().join("device");
    let originals = install_device_fixture(&root, &vault);
    let directories = [&root, &root.join("backups"), &root.join(DEVICE_BACKUP_DIR)];
    for directory in directories {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (path, _) in &originals {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    stage_device_files_with_vault(&root, &vault).unwrap();
    for directory in directories {
        assert_eq!(
            std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    for (path, original) in originals {
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}

#[test]
fn device_inventory_is_not_an_ordinary_owned_inventory() {
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    install_device_fixture(tree.path(), &vault);
    assert_eq!(
        stage_device_files_with_vault(tree.path(), &vault)
            .unwrap()
            .len(),
        5
    );
    assert!(stage_owned_files_with_vault(tree.path(), &vault, false)
        .unwrap()
        .is_empty());
    assert!(stage_owned_files_with_vault(tree.path(), &vault, true)
        .unwrap()
        .is_empty());
}

#[test]
fn device_inventory_authenticates_every_member_before_returning() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    for name in device_names() {
        let tree = temp_tree();
        let vault = VaultContext::generate().unwrap();
        let originals = install_device_fixture(tree.path(), &vault);
        let path = tree.path().join(&name);
        let sealed = std::fs::read_to_string(&path).unwrap();
        let body = URL_SAFE_NO_PAD
            .decode(sealed.strip_prefix("lpenc1.").unwrap())
            .unwrap();
        let mut envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let mut payload = URL_SAFE_NO_PAD
            .decode(envelope["payload"]["ciphertext"].as_str().unwrap())
            .unwrap();
        payload[0] ^= 1;
        envelope["payload"]["ciphertext"] =
            serde_json::Value::String(URL_SAFE_NO_PAD.encode(payload));
        let corrupt = format!(
            "lpenc1.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).unwrap())
        )
        .into_bytes();
        std::fs::write(&path, &corrupt).unwrap();
        assert!(
            matches!(stage_device_files_with_vault(tree.path(), &vault), Err(AppError::Config(code)) if code == "secret.authentication_failed"),
            "did not authenticate {name}"
        );
        for (original_path, original) in originals {
            assert_eq!(
                std::fs::read(&original_path).unwrap(),
                if original_path == path {
                    corrupt.clone()
                } else {
                    original
                }
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn device_inventory_rejects_registered_fifo_without_reading_it() {
    use std::os::unix::ffi::OsStrExt;
    for name in [
        "live-state.json".to_string(),
        format!("{DEVICE_BACKUP_DIR}/{}.source", "a".repeat(64)),
    ] {
        let tree = temp_tree();
        let vault = VaultContext::generate().unwrap();
        let path = tree.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a valid NUL-terminated path owned by this test.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert!(stage_device_files_with_vault(tree.path(), &vault).is_err());
        use std::os::unix::fs::FileTypeExt;
        assert!(std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_fifo());
    }
}

#[test]
fn device_path_inventory_is_guarded_but_does_not_authenticate_a_generation() {
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    let root = tree.path().join("device");
    let absent = tree.path().join("missing/device");
    assert!(device_file_paths(&absent).unwrap().is_empty());
    assert!(!tree.path().join("missing").exists());
    let originals = install_device_fixture(&root, &vault);
    for (path, _) in &originals {
        std::fs::write(path, b"unauthenticated-evidence").unwrap();
    }
    let paths = device_file_paths(&root).unwrap();
    assert_eq!(paths.len(), 5);
    assert!(paths.windows(2).all(|pair| pair[0].1 < pair[1].1));
    for (file, path) in paths {
        assert_eq!(path, root.join(file.relative_path()));
        assert_eq!(std::fs::read(path).unwrap(), b"unauthenticated-evidence");
    }
    assert!(stage_device_files_with_vault(&root, &vault).is_err());
    let invalid = root.join(DEVICE_BACKUP_DIR).join("unknown.backup");
    std::fs::write(&invalid, b"recovery-evidence").unwrap();
    assert!(device_file_paths(&root).is_err());
    assert_eq!(std::fs::read(invalid).unwrap(), b"recovery-evidence");
}

#[test]
fn device_backup_identity_uses_portable_spelling_without_relaxing_the_registry() {
    for suffix in ["backup", "source"] {
        let name = format!("{}.{}", "a".repeat(64), suffix);
        let file = device_backup_file(std::ffi::OsStr::new(&name)).unwrap();
        assert_eq!(
            file.relative_path().to_str().unwrap(),
            format!("{DEVICE_BACKUP_DIR}/{name}")
        );
    }
    for name in [
        format!("../{}.source", "a".repeat(64)),
        format!("sub/{}.source", "a".repeat(64)),
        format!("sub\\{}.source", "a".repeat(64)),
        format!("{}.source:stream", "a".repeat(64)),
        "notes.json".into(),
    ] {
        assert!(device_backup_file(std::ffi::OsStr::new(&name)).is_err());
    }
}

#[test]
fn device_inventory_rejects_invalid_root_empty_before_authentication() {
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    let canary = tree.path().join("live-state.json");
    std::fs::write(&canary, CANARY).unwrap();
    let empty = Path::new("");
    assert!(
        matches!(device_file_paths(empty), Err(AppError::Config(code)) if code == "secret.invalid_storage_path")
    );
    assert!(
        matches!(stage_device_files_with_vault(empty, &vault), Err(AppError::Config(code)) if code == "secret.invalid_storage_path")
    );
    assert_eq!(std::fs::read(canary).unwrap(), CANARY);
    assert!(!tree.path().join("backups").exists());
}

#[test]
fn device_inventory_rejects_invalid_root_parent_dir_before_authentication() {
    let tree = temp_tree();
    let vault = VaultContext::generate().unwrap();
    let root = tree.path().join("device");
    std::fs::create_dir(&root).unwrap();
    let canary = root.join("live-state.json");
    // Intentionally not an envelope: a guard regression must fail before decoding it.
    std::fs::write(&canary, CANARY).unwrap();
    for alias in [root.join("../device"), root.join("../missing-device")] {
        assert!(
            matches!(device_file_paths(&alias), Err(AppError::Config(code)) if code == "secret.invalid_storage_path")
        );
        assert!(
            matches!(stage_device_files_with_vault(&alias, &vault), Err(AppError::Config(code)) if code == "secret.invalid_storage_path")
        );
        assert_eq!(std::fs::read(&canary).unwrap(), CANARY);
        assert!(!root.join("backups").exists());
        assert!(!tree.path().join("missing-device").exists());
    }
}
