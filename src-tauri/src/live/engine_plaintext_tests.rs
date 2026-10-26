use super::*;
use std::sync::RwLock;

#[cfg_attr(test, test)]
fn plaintext_device_write_refuses_to_replace_existing_encrypted_state() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(root.path().to_path_buf());
    let key = RwLock::new(StorageContext::Plaintext(
        crate::secrets::storage::PlaintextIdentity {
            format_version: 1,
            store_id: "11111111-1111-4111-8111-111111111111".into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        },
    ));
    let file = DeviceFile::registered(DEVICE_STATE_FILE).unwrap();
    let opaque = b"lpenc:synthetic-unavailable-key";
    fs::write(store.path_for(&file), opaque).unwrap();
    assert!(store
        .write_device(&key.read().unwrap(), &file, b"{}")
        .is_err());
    assert_eq!(fs::read(store.path_for(&file)).unwrap(), opaque);
}

#[cfg_attr(test, test)]
fn plaintext_first_write_backup_preserves_original_binary_once_without_a_key() {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = DeviceStore::at(root.path().join("device"));
    let path = root.path().join("synthetic-client.bin");
    let key = RwLock::new(StorageContext::Plaintext(
        crate::secrets::storage::PlaintextIdentity {
            format_version: 1,
            store_id: "11111111-1111-4111-8111-111111111111".into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        },
    ));
    let storage = key.read().unwrap();
    let original = [0xff, 0xfe, 0, 7];
    ensure_first_write_backup(&store, &storage, &path, Some(&original)).unwrap();
    ensure_first_write_backup(&store, &storage, &path, Some(b"later")).unwrap();
    let identity = sha256_hex(path.to_str().unwrap().as_bytes());
    let file = DeviceFile::registered(format!("{DEVICE_BACKUP_DIR}/{identity}.backup")).unwrap();
    assert_eq!(fs::read(store.path_for(&file)).unwrap(), original);
    assert_eq!(
        store
            .read_device(&storage, &file)
            .unwrap()
            .unwrap()
            .as_slice(),
        original
    );
    assert!(!store.root().join("vault.json").exists());
}

#[cfg(feature = "test-hooks")]
pub(crate) fn run() {
    plaintext_device_write_refuses_to_replace_existing_encrypted_state();
    plaintext_first_write_backup_preserves_original_binary_once_without_a_key();
}
