//! Explicit lifecycle ownership for the active credential vault.

use super::key_store::{save_verified, KeyStore};
use super::{VaultContext, VaultMetadata};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LocalVault {
    pub metadata: VaultMetadata,
    pub automatic_unlock: bool,
    pub migration_state: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase", deny_unknown_fields)]
enum MigrationState {
    Pending { import_legacy_json: bool },
    Complete,
}

pub struct SecretSession {
    root: PathBuf,
    _temporary_root: Option<tempfile::TempDir>,
    vault: RwLock<VaultContext>,
    blocked: AtomicBool,
}

impl SecretSession {
    pub fn open(
        root: &Path,
        store: &dyn KeyStore,
        password: Option<&str>,
    ) -> Result<Arc<Self>, AppError> {
        Self::open_inner(root, store, password, true)
    }

    pub(crate) fn open_existing(
        root: &Path,
        store: &dyn KeyStore,
        password: Option<&str>,
    ) -> Result<Arc<Self>, AppError> {
        Self::open_inner(root, store, password, false)
    }

    fn open_inner(
        root: &Path,
        store: &dyn KeyStore,
        password: Option<&str>,
        create: bool,
    ) -> Result<Arc<Self>, AppError> {
        if create {
            crate::config::ensure_private_directory(root)?;
        }
        super::bootstrap_restore::recover(root, store, password)?;
        super::transition::recover(root, store, password)?;
        super::rewrap::recover_with_password(root, store, password)?;
        let path = root.join("vault.json");
        let mut vault = if path.exists() {
            let saved = read_metadata(root)?;
            if let Some(password) = password {
                VaultContext::from_password(saved.metadata, password)
                    .map_err(super::inventory::secret_error)?
            } else if saved.automatic_unlock {
                let key = store
                    .load(&saved.metadata.vault_id, &saved.metadata.key_id)
                    .map_err(|_| AppError::Config("secret.store_unavailable".into()))?
                    .ok_or_else(|| AppError::Config("secret.key_missing".into()))?;
                VaultContext::from_key(saved.metadata, key)
                    .map_err(super::inventory::secret_error)?
            } else {
                return Err(AppError::Config("secret.locked".into()));
            }
        } else {
            if !create {
                return Err(AppError::Config("secret.migration_required".into()));
            }
            for name in [crate::config::DB_FILE_NAME, ".vault-migration.db"] {
                let database = root.join(name);
                if database.exists() {
                    let conn = rusqlite::Connection::open_with_flags(
                        &database,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .map_err(|e| AppError::Database(e.to_string()))?;
                    if crate::database::vault::stored_metadata(&conn)?.is_some() {
                        return Err(AppError::Config("secret.metadata_missing".into()));
                    }
                }
            }
            let generated = VaultContext::generate().map_err(super::inventory::secret_error)?;
            let (vault, automatic_unlock) = if let Some(password) = password {
                (
                    generated
                        .with_password(password)
                        .map_err(super::inventory::secret_error)?,
                    false,
                )
            } else {
                save_verified(
                    store,
                    &generated.metadata().vault_id,
                    &generated.metadata().key_id,
                    &generated.export_key(),
                )
                .map_err(|_| AppError::Config("secret.password_setup_required".into()))?;
                (generated, true)
            };
            let state = MigrationState::Pending {
                import_legacy_json: !root.join(crate::config::DB_FILE_NAME).exists()
                    && root.join("config.json").exists(),
            };
            let migration_state = seal_migration_state(&vault, &state)?;
            write_metadata(
                root,
                &LocalVault {
                    metadata: vault.metadata().clone(),
                    automatic_unlock,
                    migration_state,
                },
            )?;
            vault
        };
        super::rewrap::recover(root, &mut vault, store)?;
        if password.is_none() && !read_metadata(root)?.automatic_unlock {
            return Err(AppError::Config("secret.locked".into()));
        }
        Ok(Self::from_context(root.to_path_buf(), vault))
    }

    pub(crate) fn from_context(root: PathBuf, vault: VaultContext) -> Arc<Self> {
        Arc::new(Self {
            root,
            _temporary_root: None,
            vault: RwLock::new(vault),
            blocked: AtomicBool::new(false),
        })
    }

    pub(crate) fn ephemeral() -> Result<Arc<Self>, AppError> {
        let directory = tempfile::tempdir()
            .map_err(|_| AppError::Config("secret.temporary_storage_unavailable".into()))?;
        let vault = VaultContext::generate().map_err(super::inventory::secret_error)?;
        Ok(Arc::new(Self {
            root: directory.path().to_path_buf(),
            _temporary_root: Some(directory),
            vault: RwLock::new(vault),
            blocked: AtomicBool::new(false),
        }))
    }

    pub(crate) fn read(&self) -> Result<RwLockReadGuard<'_, VaultContext>, AppError> {
        let vault = self
            .vault
            .read()
            .map_err(|_| AppError::Config("secret.session_unavailable".into()))?;
        if self.blocked.load(Ordering::Acquire) {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
        Ok(vault)
    }

    pub(crate) fn write(&self) -> Result<RwLockWriteGuard<'_, VaultContext>, AppError> {
        let vault = self
            .vault
            .write()
            .map_err(|_| AppError::Config("secret.session_unavailable".into()))?;
        if self.blocked.load(Ordering::Acquire) {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
        Ok(vault)
    }

    /// Check lifecycle admission without acquiring the key lock. Database callers
    /// already hold their connection lock and must not invert session lock order.
    pub(crate) fn ensure_available(&self) -> Result<(), AppError> {
        if self.blocked.load(Ordering::Acquire) {
            Err(AppError::Config("secret.recovery_required".into()))
        } else {
            Ok(())
        }
    }

    pub(crate) fn set_blocked(&self, blocked: bool) {
        self.blocked.store(blocked, Ordering::Release);
    }

    /// Root of the application-owned credential storage for this session.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn migration_pending(&self) -> Result<bool, AppError> {
        Ok(matches!(
            self.migration_state()?,
            MigrationState::Pending { .. }
        ))
    }

    pub(crate) fn legacy_json_pending(&self) -> Result<bool, AppError> {
        Ok(matches!(
            self.migration_state()?,
            MigrationState::Pending {
                import_legacy_json: true
            }
        ))
    }

    fn migration_state(&self) -> Result<MigrationState, AppError> {
        let vault = self.read()?;
        let saved = read_metadata(&self.root)?;
        let state = vault
            .open(&["local", "migration-state"], &saved.migration_state)
            .map_err(super::inventory::secret_error)?;
        serde_json::from_slice(&state)
            .map_err(|_| AppError::Config("secret.invalid_metadata".into()))
    }

    pub(crate) fn complete_migration(&self) -> Result<(), AppError> {
        let vault = self.read()?;
        let mut saved = read_metadata(&self.root)?;
        saved.migration_state = seal_migration_state(&vault, &MigrationState::Complete)?;
        write_metadata(&self.root, &saved)
    }
}

pub(crate) fn completed_metadata(
    vault: &VaultContext,
    automatic_unlock: bool,
) -> Result<LocalVault, AppError> {
    Ok(LocalVault {
        metadata: vault.metadata().clone(),
        automatic_unlock,
        migration_state: seal_migration_state(vault, &MigrationState::Complete)?,
    })
}

fn seal_migration_state(vault: &VaultContext, state: &MigrationState) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(state)
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    vault
        .seal(&["local", "migration-state"], &bytes)
        .map_err(super::inventory::secret_error)
}

pub(crate) fn read_metadata(root: &Path) -> Result<LocalVault, AppError> {
    let path = root.join("vault.json");
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| AppError::io(&path, e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(AppError::Config("secret.invalid_metadata".into()));
    }
    let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::Config("secret.invalid_metadata".into()))
}

pub(crate) fn ensure_owned_directory(root: &Path, directory: &Path) -> Result<(), AppError> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| AppError::Config("secret.invalid_path".into()))?;
    crate::config::ensure_private_directory(root)?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(AppError::Config("secret.invalid_path".into()));
        }
        current.push(part);
        crate::config::ensure_private_directory(&current)?;
    }
    Ok(())
}

pub(crate) fn write_metadata(root: &Path, saved: &LocalVault) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(saved)
        .map_err(|_| AppError::Config("secret.invalid_metadata".into()))?;
    write_durable(&root.join("vault.json"), &bytes)
}

pub(crate) fn write_durable(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    crate::config::atomic_write_private(path, bytes)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| AppError::io(path, e))?;
    file.sync_all().map_err(|e| AppError::io(path, e))?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| AppError::io(parent, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::key_store::KeyStoreError;
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct MemoryKeys(Mutex<HashMap<String, Vec<u8>>>);
    impl KeyStore for MemoryKeys {
        fn load(
            &self,
            vault: &str,
            key: &str,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&format!("{vault}/{key}"))
                .cloned()
                .map(Zeroizing::new))
        }
        fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
            self.0
                .lock()
                .unwrap()
                .insert(format!("{vault}/{key}"), bytes.to_vec());
            Ok(())
        }
        fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
            self.0.lock().unwrap().remove(&format!("{vault}/{key}"));
            Ok(())
        }
    }

    #[test]
    fn first_open_persists_public_metadata_and_reopens_with_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeys::default();
        let first = SecretSession::open(dir.path(), &store, None).unwrap();
        let ciphertext = first
            .read()
            .unwrap()
            .seal(&["fixture"], b"canary-secret")
            .unwrap();
        let restored = SecretSession::open(dir.path(), &store, None).unwrap();
        assert_eq!(
            &**restored
                .read()
                .unwrap()
                .open(&["fixture"], &ciphertext)
                .unwrap(),
            b"canary-secret"
        );
        let raw = std::fs::read_to_string(dir.path().join("vault.json")).unwrap();
        assert!(!raw.contains("canary-secret"));
        let metadata: LocalVault = serde_json::from_str(&raw).unwrap();
        assert!(metadata.automatic_unlock);
    }

    #[test]
    fn missing_key_does_not_replace_existing_vault_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeys::default();
        let session = SecretSession::open(dir.path(), &store, None).unwrap();
        let metadata = session.read().unwrap().metadata().clone();
        let before = std::fs::read(dir.path().join("vault.json")).unwrap();
        store.remove(&metadata.vault_id, &metadata.key_id).unwrap();
        assert!(SecretSession::open(dir.path(), &store, None).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("vault.json")).unwrap(),
            before
        );
    }
}
