use super::{
    key_store::{KeyStore, KeyStoreError},
    session::SecretSession,
};
use crate::{database::Database, error::AppError};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};
use zeroize::Zeroizing;

type StoredKeys = HashMap<(String, String), Zeroizing<Vec<u8>>>;

#[derive(Default)]
pub(crate) struct MemoryKeyStore(Mutex<StoredKeys>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, vault: &str, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(vault.into(), key.into()))
            .cloned())
    }
    fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .unwrap()
            .insert((vault.into(), key.into()), Zeroizing::new(bytes.to_vec()));
        Ok(())
    }
    fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
        self.0.lock().unwrap().remove(&(vault.into(), key.into()));
        Ok(())
    }
}

pub(crate) fn initialize_database() -> Result<Database, AppError> {
    assert!(
        std::env::var_os("CC_SWITCH_TEST_HOME").is_some(),
        "disk tests must select an isolated home"
    );
    static STORE: OnceLock<MemoryKeyStore> = OnceLock::new();
    let session = SecretSession::open(
        &crate::config::get_app_config_dir(),
        STORE.get_or_init(MemoryKeyStore::default),
        None,
    )?;
    crate::settings::unlock_settings_for_test(session.clone())?;
    Database::init_with_secrets(session)
}
