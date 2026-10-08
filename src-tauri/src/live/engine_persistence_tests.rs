use super::*;
use std::sync::RwLock;

#[test]
fn first_write_backup_keeps_encrypted_original_once_with_full_identity() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(dir.path().join("device"));
    let path = dir.path().join("client.json");
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    ensure_first_write_backup(&store, &vault, &path, Some(b"original-secret")).unwrap();
    ensure_first_write_backup(&store, &vault, &path, Some(b"later-secret")).unwrap();
    let identity = sha256_hex(path.to_str().unwrap().as_bytes());
    let backup = DeviceFile::registered(format!("{DEVICE_BACKUP_DIR}/{identity}.backup")).unwrap();
    assert_eq!(
        &**store
            .read_device(&vault, &backup)
            .unwrap()
            .as_ref()
            .unwrap(),
        b"original-secret"
    );
    for entry in fs::read_dir(store.first_write_backup_dir()).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        assert_eq!(name.split('.').next().unwrap().len(), 64);
        let bytes = fs::read(entry.path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("original-secret"));
        assert!(!String::from_utf8_lossy(&bytes).contains("client.json"));
    }
    assert_eq!(
        fs::read_dir(store.first_write_backup_dir())
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn absent_first_write_is_a_durable_encrypted_tombstone() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(dir.path().join("device"));
    let path = dir.path().join("new.json");
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    ensure_first_write_backup(&store, &vault, &path, None).unwrap();
    ensure_first_write_backup(&store, &vault, &path, Some(b"later")).unwrap();
    assert_eq!(
        fs::read_dir(store.first_write_backup_dir())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn incomplete_backup_never_overwrites_original() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(dir.path().join("device"));
    let path = dir.path().join("client.json");
    let key = RwLock::new(VaultContext::generate().unwrap());
    let vault = key.read().unwrap();
    let identity = sha256_hex(path.to_str().unwrap().as_bytes());
    let backup = DeviceFile::registered(format!("{DEVICE_BACKUP_DIR}/{identity}.backup")).unwrap();
    store.create_device(&vault, &backup, b"original").unwrap();
    assert!(ensure_first_write_backup(&store, &vault, &path, Some(b"later")).is_err());
    assert_eq!(
        &**store
            .read_device(&vault, &backup)
            .unwrap()
            .as_ref()
            .unwrap(),
        b"original"
    );
    ensure_first_write_backup(&store, &vault, &path, Some(b"original")).unwrap();
}

#[cfg(unix)]
#[test]
fn linked_device_root_is_rejected_without_touching_destination() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let other = dir.path().join("other");
    fs::create_dir(&other).unwrap();
    let linked = dir.path().join("device");
    std::os::unix::fs::symlink(&other, &linked).unwrap();
    let store = DeviceStore::at(&linked);
    let key = RwLock::new(VaultContext::generate().unwrap());
    let file = DeviceFile::registered(DEVICE_STATE_FILE).unwrap();
    assert!(store
        .write_device(&key.read().unwrap(), &file, b"secret")
        .is_err());
    assert_eq!(fs::read_dir(&other).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn windows_canonical_fixture_and_drive_roots_are_admitted() {
    use std::path::Prefix;
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let file = dir.path().join("synthetic.json");
    fs::write(&file, b"synthetic").unwrap();
    let canonical = file.canonicalize().unwrap();
    let Component::Prefix(prefix) = canonical.components().next().unwrap() else {
        panic!("Windows canonical fixture must have a prefix");
    };
    let Prefix::VerbatimDisk(drive) = prefix.kind() else {
        panic!("Windows canonical fixture must use its extended drive prefix");
    };
    validate_file_path(&canonical).unwrap();
    validate_path(Path::new(&format!("{}:\\", drive as char)), true).unwrap();
    validate_path(Path::new(&format!("\\\\?\\{}:\\", drive as char)), true).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_drive_and_unc_prefixes_are_joined_to_root_before_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let directory_metadata = fs::symlink_metadata(dir.path()).unwrap();
    for (path, root) in [
        (r"C:\", r"C:\"),
        (r"\\?\C:\", r"\\?\C:\"),
        (r"C:\synthetic\leaf", r"C:\"),
        (r"\\?\C:\synthetic\leaf", r"\\?\C:\"),
        (
            r"\\fixture-server\fixture-share\",
            r"\\fixture-server\fixture-share\",
        ),
        (
            r"\\?\UNC\fixture-server\fixture-share\",
            r"\\?\UNC\fixture-server\fixture-share\",
        ),
        (
            r"\\fixture-server\fixture-share\synthetic\leaf",
            r"\\fixture-server\fixture-share\",
        ),
        (
            r"\\?\UNC\fixture-server\fixture-share\synthetic\leaf",
            r"\\?\UNC\fixture-server\fixture-share\",
        ),
    ] {
        let mut inspected = Vec::new();
        // Windows std::path parses the real prefixes; no SMB share is queried.
        validate_path_with_metadata(Path::new(path), true, |walked| {
            inspected.push(walked.to_path_buf());
            Ok(directory_metadata.clone())
        })
        .unwrap();
        assert_eq!(
            inspected.first().unwrap().as_os_str(),
            Path::new(root).as_os_str(),
            "{path}"
        );
        assert_eq!(
            inspected.last().unwrap().as_os_str(),
            Path::new(path).as_os_str(),
            "{path}"
        );
        assert_eq!(inspected.len(), if path == root { 1 } else { 3 }, "{path}");
    }
}

#[cfg(windows)]
#[test]
fn windows_drive_relative_and_prefixless_rooted_paths_remain_rejected() {
    for path in [
        r"C:",
        r"C:synthetic\leaf",
        r"\synthetic\leaf",
        r"synthetic\leaf",
    ] {
        assert!(validate_path_with_metadata(Path::new(path), false, |_| {
            panic!("relative paths must be rejected before metadata: {path}")
        })
        .is_err());
    }
}

#[cfg(windows)]
#[test]
fn windows_junction_ancestor_remains_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::create_dir(&target).unwrap();
    let target_file = target.join("synthetic.json");
    fs::write(&target_file, b"unchanged").unwrap();
    validate_file_path(&target_file.canonicalize().unwrap()).unwrap();
    let junction = dir.path().join("junction");
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "create fixture junction: {output:?}"
    );
    assert!(validate_file_path(&junction.join("synthetic.json")).is_err());
    assert_eq!(fs::read(&target_file).unwrap(), b"unchanged");
}

#[test]
fn path_validation_still_rejects_parent_aliases_and_directory_leaves() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(validate_file_path(&nested).is_err());
    // PathBuf::join normalizes .. for Windows verbatim prefixes. Preserve the
    // raw input so this regression actually reaches the ParentDir admission gate.
    let mut raw = nested.as_os_str().to_owned();
    raw.push(std::path::MAIN_SEPARATOR_STR);
    raw.push("..");
    raw.push(std::path::MAIN_SEPARATOR_STR);
    raw.push("synthetic.json");
    let alias = PathBuf::from(raw);
    assert!(alias
        .components()
        .any(|part| matches!(part, Component::ParentDir)));
    assert!(validate_file_path(&alias).is_err());
    assert!(validate_file_path(Path::new("relative.json")).is_err());
}

#[cfg(unix)]
#[test]
fn linked_client_leaf_and_hardlink_are_still_rejected() {
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let original = dir.path().join("original");
    let linked = dir.path().join("linked");
    fs::write(&original, b"unchanged").unwrap();
    std::os::unix::fs::symlink(&original, &linked).unwrap();
    assert!(validate_file_path(&linked).is_err());
    fs::remove_file(&linked).unwrap();
    fs::hard_link(&original, &linked).unwrap();
    assert!(validate_file_path(&linked).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"unchanged");
}

#[cfg(windows)]
#[test]
fn windows_bare_verbatim_prefixes_cannot_bypass_root_admission() {
    for raw in [
        r"\\?\C:",
        r"\\?\UNC\fixture-server\fixture-share",
        r"\\?\fixture-volume",
    ] {
        let path = Path::new(raw);
        assert!(path.is_absolute());
        assert!(!path
            .components()
            .any(|part| matches!(part, Component::RootDir)));
        assert!(validate_path_with_metadata(path, true, |_| {
            panic!("a bare verbatim prefix is not an inspectable root: {raw}")
        })
        .is_err());
    }
}

#[cfg(windows)]
#[test]
fn windows_device_namespaces_remain_rejected_before_metadata() {
    for raw in [
        r"\\.\C:\synthetic\leaf",
        r"\\.\fixture-device\synthetic\leaf",
        r"\\?\Volume{fixture-volume}\synthetic\leaf",
        r"\\?\fixture-volume\synthetic\leaf",
    ] {
        let path = Path::new(raw);
        assert!(path
            .components()
            .any(|part| matches!(part, Component::RootDir)));
        assert!(matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::DeviceNS(_) | std::path::Prefix::Verbatim(_))
        ));
        assert!(validate_path_with_metadata(path, true, |_| {
            panic!("device namespaces are not client-file roots: {raw}")
        })
        .is_err());
    }
}
