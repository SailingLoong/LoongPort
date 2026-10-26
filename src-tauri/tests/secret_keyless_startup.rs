//! Read-only startup custody diagnostics over the existing public session owner.
//! All keys are synthetic and held in this in-memory store. No OS credential store,
//! account login, restore/import journal, migration, reset or real user file is used.
use cc_switch_lib::{KeyStore, KeyStoreError, SecretSession};
use std::{collections::BTreeMap, path::Path, sync::Mutex};
use zeroize::Zeroizing;

#[derive(Default)]
struct MemoryKeys {
    keys: Mutex<BTreeMap<(String, String), Vec<u8>>>,
    loads_denied: Mutex<bool>,
    saves_denied: Mutex<bool>,
    saves: Mutex<usize>,
}
impl KeyStore for MemoryKeys {
    fn load(&self, vault: &str, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        if *self.loads_denied.lock().unwrap() {
            return Err(KeyStoreError::Unavailable);
        }
        Ok(self
            .keys
            .lock()
            .unwrap()
            .get(&(vault.into(), key.into()))
            .cloned()
            .map(Zeroizing::new))
    }
    fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
        *self.saves.lock().unwrap() += 1;
        if *self.saves_denied.lock().unwrap() {
            return Err(KeyStoreError::Unavailable);
        }
        self.keys
            .lock()
            .unwrap()
            .insert((vault.into(), key.into()), bytes.to_vec());
        Ok(())
    }
    fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
        self.keys
            .lock()
            .unwrap()
            .remove(&(vault.into(), key.into()));
        Ok(())
    }
}
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            assert!(p.is_file(), "no recovery directories in this fixture");
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(p).unwrap(),
            )
        })
        .collect()
}
fn existing_automatic() -> (tempfile::TempDir, MemoryKeys) {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryKeys::default();
    drop(SecretSession::open(root.path(), &store, None).expect("fresh synthetic custody setup"));
    // A closed, explicitly synthetic SQLite file witnesses that failure cannot
    // replace an existing data file. This is not a production account database.
    let conn = rusqlite::Connection::open(root.path().join("loongport.db")).unwrap();
    conn.execute_batch("CREATE TABLE diagnostic_preserved(payload BLOB NOT NULL); INSERT INTO diagnostic_preserved VALUES (X'0102030405');").unwrap();
    drop(conn);
    (root, store)
}
fn failure(root: &Path, store: &MemoryKeys, expected: &str) {
    let before = snapshot(root);
    let saves = *store.saves.lock().unwrap();
    let result = SecretSession::open(root, store, None);
    let error = match result {
        Ok(_) => panic!("no key must not publish a session"),
        Err(e) => e.to_string(),
    };
    assert!(
        error.contains(expected),
        "unexpected secret-free error: {error}"
    );
    assert_eq!(
        snapshot(root),
        before,
        "all old bytes and file names must be unchanged"
    );
    assert_eq!(
        *store.saves.lock().unwrap(),
        saves,
        "no replacement key publication"
    );
}
#[test]
fn existing_automatic_key_reopens_without_password_or_new_database() {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryKeys::default();
    drop(SecretSession::open(root.path(), &store, None).unwrap());
    let before = snapshot(root.path());
    let saves = *store.saves.lock().unwrap();
    drop(SecretSession::open(root.path(), &store, None).unwrap());
    assert_eq!(snapshot(root.path()), before);
    assert_eq!(*store.saves.lock().unwrap(), saves);
    assert!(!root.path().join("loongport.db").exists());
}
#[test]
fn missing_os_key_preserves_existing_vault_and_database_bytes() {
    let (root, store) = existing_automatic();
    store.keys.lock().unwrap().clear();
    failure(root.path(), &store, "secret.key_missing");
    assert!(store.keys.lock().unwrap().is_empty());
}
#[test]
fn denied_os_key_preserves_existing_vault_and_database_bytes() {
    let (root, store) = existing_automatic();
    *store.loads_denied.lock().unwrap() = true;
    failure(root.path(), &store, "secret.store_unavailable");
}
#[test]
fn wrong_os_key_is_not_replaced_or_treated_as_an_empty_install() {
    let (root, store) = existing_automatic();
    for key in store.keys.lock().unwrap().values_mut() {
        key.fill(0);
    }
    failure(root.path(), &store, "secret.key_rejected");
}
#[test]
fn closed_password_only_vault_has_no_automatic_key_and_no_empty_database() {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryKeys::default();
    drop(
        SecretSession::open(
            root.path(),
            &store,
            Some("synthetic-diagnostic-password-only"),
        )
        .unwrap(),
    );
    assert!(store.keys.lock().unwrap().is_empty());
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("vault.json")).unwrap()).unwrap();
    assert_eq!(metadata["automaticUnlock"], false);
    assert!(metadata["metadata"]["wrappedKey"].is_object());
    failure(root.path(), &store, "secret.locked");
    assert!(!root.path().join("loongport.db").exists());
}
#[test]
fn new_install_store_failure_does_not_publish_vault_or_empty_database() {
    let root = tempfile::tempdir().unwrap();
    let store = MemoryKeys::default();
    *store.saves_denied.lock().unwrap() = true;
    let error = match SecretSession::open(root.path(), &store, None) {
        Ok(_) => panic!("unverified custody must not publish a session"),
        Err(e) => e.to_string(),
    };
    // Baseline diagnosis only: this old code is exactly the unwanted password
    // setup fallback; observing it does not implement or endorse that UI.
    assert!(error.contains("secret.password_setup_required"));
    assert!(snapshot(root.path()).is_empty());
    assert!(store.keys.lock().unwrap().is_empty());
}
