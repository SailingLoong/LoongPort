//! Platform key custody. Callers retain recovery material until verified publication.

use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("credential_store_unavailable")]
    Unavailable,
    #[error("credential_store_verification_failed")]
    Verification,
}

pub trait KeyStore: Send + Sync {
    fn load(
        &self,
        vault_id: &str,
        key_id: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError>;
    fn save(&self, vault_id: &str, key_id: &str, key: &[u8]) -> Result<(), KeyStoreError>;
    fn remove(&self, vault_id: &str, key_id: &str) -> Result<(), KeyStoreError>;
}

pub(crate) fn save_verified(
    store: &dyn KeyStore,
    vault_id: &str,
    key_id: &str,
    key: &[u8],
) -> Result<(), KeyStoreError> {
    if key.len() != 32 {
        return Err(KeyStoreError::Verification);
    }
    if let Some(existing) = store.load(vault_id, key_id)? {
        if existing.as_slice() != key {
            return Err(KeyStoreError::Verification);
        }
        return Ok(());
    }
    store.save(vault_id, key_id, key)?;
    match store.load(vault_id, key_id)? {
        Some(stored) if stored.as_slice() == key => Ok(()),
        _ => Err(KeyStoreError::Verification),
    }
}

/// Explicit supported-platform implementation; never use keyring's mock fallback.
pub(crate) struct SystemKeyStore;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn entry(vault_id: &str, key_id: &str) -> Result<keyring::Entry, KeyStoreError> {
    let vault = uuid::Uuid::parse_str(vault_id).map_err(|_| KeyStoreError::Verification)?;
    let key = uuid::Uuid::parse_str(key_id).map_err(|_| KeyStoreError::Verification)?;
    keyring::Entry::new("LoongPort credential vault", &format!("{vault}/{key}"))
        .map_err(|_| KeyStoreError::Unavailable)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl KeyStore for SystemKeyStore {
    fn load(
        &self,
        vault_id: &str,
        key_id: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        match entry(vault_id, key_id)?.get_secret() {
            Ok(key) => Ok(Some(Zeroizing::new(key))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(KeyStoreError::Unavailable),
        }
    }

    fn save(&self, vault_id: &str, key_id: &str, key: &[u8]) -> Result<(), KeyStoreError> {
        entry(vault_id, key_id)?
            .set_secret(key)
            .map_err(|_| KeyStoreError::Unavailable)
    }

    fn remove(&self, vault_id: &str, key_id: &str) -> Result<(), KeyStoreError> {
        match entry(vault_id, key_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(KeyStoreError::Unavailable),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
impl KeyStore for SystemKeyStore {
    fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Err(KeyStoreError::Unavailable)
    }
    fn save(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeyStoreError> {
        Err(KeyStoreError::Unavailable)
    }
    fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        Err(KeyStoreError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    #[ignore = "requires an available operating-system credential store; uses a fresh test-only account"]
    fn platform_store_round_trip_uses_an_isolated_random_account() {
        let vault_id = uuid::Uuid::new_v4().to_string();
        let key_id = uuid::Uuid::new_v4().to_string();
        let vault = crate::secrets::VaultContext::generate().unwrap();
        let result = (|| {
            assert!(SystemKeyStore.load(&vault_id, &key_id)?.is_none());
            save_verified(&SystemKeyStore, &vault_id, &key_id, &vault.export_key())?;
            assert_eq!(
                SystemKeyStore.load(&vault_id, &key_id)?.unwrap().as_slice(),
                vault.export_key().as_slice()
            );
            Ok::<_, KeyStoreError>(())
        })();
        let cleanup = SystemKeyStore.remove(&vault_id, &key_id);
        result.unwrap();
        cleanup.unwrap();
        assert!(SystemKeyStore.load(&vault_id, &key_id).unwrap().is_none());
    }

    struct TestStore {
        stored: Mutex<Option<Vec<u8>>>,
        discard_writes: bool,
    }

    impl KeyStore for TestStore {
        fn load(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
            Ok(self.stored.lock().unwrap().clone().map(Zeroizing::new))
        }
        fn save(&self, _: &str, _: &str, key: &[u8]) -> Result<(), KeyStoreError> {
            if !self.discard_writes {
                *self.stored.lock().unwrap() = Some(key.to_vec());
            }
            Ok(())
        }
        fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
            *self.stored.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn publication_requires_key_to_be_read_back_before_source_is_discarded() {
        let store = TestStore {
            stored: Mutex::new(None),
            discard_writes: true,
        };
        assert!(save_verified(&store, "vault", "key", &[42; 32]).is_err());
    }

    #[test]
    fn verified_publication_preserves_existing_different_key() {
        let store = TestStore {
            stored: Mutex::new(Some(vec![7; 32])),
            discard_writes: false,
        };
        assert!(save_verified(&store, "vault", "key", &[42; 32]).is_err());
        assert_eq!(
            &**store.load("vault", "key").unwrap().as_ref().unwrap(),
            &[7; 32]
        );
    }

    #[test]
    fn verified_publication_can_be_repeated_after_interruption() {
        let store = TestStore {
            stored: Mutex::new(None),
            discard_writes: false,
        };
        save_verified(&store, "vault", "key", &[42; 32]).unwrap();
        save_verified(&store, "vault", "key", &[42; 32]).unwrap();
        assert_eq!(
            &**store.load("vault", "key").unwrap().as_ref().unwrap(),
            &[42; 32]
        );
    }
}
