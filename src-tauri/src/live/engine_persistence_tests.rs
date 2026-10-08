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
