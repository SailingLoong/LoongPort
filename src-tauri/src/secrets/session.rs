//! Explicit lifecycle ownership for the active credential vault.

use super::key_store::{save_verified, KeyStore};
use super::storage::StorageContext;
use super::{VaultContext, VaultMetadata};
pub(crate) use crate::config_file_io::write_durable;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
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
    vault: RwLock<StorageContext>,
    blocked: AtomicBool,
    // Volatile lease invalidation only; not a persistent account revision.
    lifecycle_epoch: AtomicU64,
}

impl SecretSession {
    /// Internal authenticated key material, never a user-text IPC parser.
    /// New choices are explicit; this entry never generates a replacement key.
    pub(crate) fn open_protected(
        root: &Path,
        device: &Path,
        store: &dyn KeyStore,
        provided_key: Option<zeroize::Zeroizing<Vec<u8>>>,
    ) -> Result<Arc<Self>, AppError> {
        super::files::device_directory_exists(root)?;
        super::files::device_directory_exists(device)?;
        if super::reset::pending(root)?
            || super::bootstrap_restore::pending_record(root)?.is_some()
            || super::transition::pending_record(root)?.is_some()
            || super::rewrap::pending_record(root)?.is_some()
        {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
        crate::database::storage::preflight_runtime(&root.join(crate::config::DB_FILE_NAME))?;
        let record = super::protection::read(root)?;
        let local = read_metadata_with_bytes(root)?.map(|(local,_)|local);
        if let Some(local) = &local {
            local
                .metadata
                .validate()
                .map_err(super::inventory::secret_error)?;
        }
        let context = if let Some(record) = record {
            super::protection::open_context(
                &record,
                local.as_ref().map(|local| &local.metadata),
                provided_key,
                |vault_id, key_id| {
                    store
                        .load(vault_id, key_id)
                        .map_err(|_| AppError::Config("secret.store_unavailable".into()))
                },
            )?
        } else if let Some(local) = local {
            if local.metadata.user_text_key.is_some() {
                return Err(AppError::Config("protection.record_missing".into()));
            }
            let vault = if let Some(key) = provided_key {
                VaultContext::from_key(local.metadata, key)
                    .map_err(super::inventory::secret_error)?
            } else {
                authenticate_existing(root, store, None)?
            };
            StorageContext::Encrypted(vault)
        } else {
            if provided_key.is_some() {
                return Err(AppError::Config("protection.configuration_required".into()));
            }
            let proof = super::protection::FreshScope::capture(
                root,
                device,
                &crate::panic_hook::diagnostic_entries(),
            )?;
            let record = proof.publish_initial_plaintext()?;
            super::protection::open_context(&record, None, None, |_, _| {
                unreachable!("plaintext has no cache")
            })?
        };
        let session = Self::from_storage_context(root.to_path_buf(), context);
        // Authenticate the original stored state before handing out a session;
        // this is read-only and never starts or completes recovery/migration.
        session.migration_pending()?;
        Ok(session)
    }

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
            authenticate_existing(root, store, password)?
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
        Self::from_storage_context(root, vault.into())
    }

    pub(crate) fn from_storage_context(root: PathBuf, context: StorageContext) -> Arc<Self> {
        Arc::new(Self {
            root,
            _temporary_root: None,
            vault: RwLock::new(context),
            blocked: AtomicBool::new(false),
            lifecycle_epoch: AtomicU64::new(0),
        })
    }

    pub(crate) fn ephemeral() -> Result<Arc<Self>, AppError> {
        let base = std::env::temp_dir()
            .canonicalize()
            .map_err(|_| AppError::Config("secret.temporary_storage_unavailable".into()))?;
        let directory = tempfile::tempdir_in(base)
            .map_err(|_| AppError::Config("secret.temporary_storage_unavailable".into()))?;
        let vault = VaultContext::generate().map_err(super::inventory::secret_error)?;
        Ok(Arc::new(Self {
            root: directory.path().to_path_buf(),
            _temporary_root: Some(directory),
            vault: RwLock::new(vault.into()),
            blocked: AtomicBool::new(false),
            lifecycle_epoch: AtomicU64::new(0),
        }))
    }

    pub(crate) fn read(&self) -> Result<RwLockReadGuard<'_, StorageContext>, AppError> {
        let vault = self
            .vault
            .read()
            .map_err(|_| AppError::Config("secret.session_unavailable".into()))?;
        if self.blocked.load(Ordering::Acquire) {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
        Ok(vault)
    }

    pub(crate) fn write(&self) -> Result<RwLockWriteGuard<'_, StorageContext>, AppError> {
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
        if blocked {
            self.lifecycle_epoch.fetch_add(1, Ordering::AcqRel);
        }
        self.blocked.store(blocked, Ordering::Release);
    }

    /// Capture and recheck while holding the session read guard around the
    /// protected operation. Unblocking must never revive an earlier lease.
    pub(crate) fn lifecycle_epoch(&self) -> u64 {
        self.lifecycle_epoch.load(Ordering::Acquire)
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
        if vault.is_plaintext() {
            return Ok(MigrationState::Complete);
        }
        let saved = read_metadata(&self.root)?;
        decode_migration_state(vault.encrypted()?, &saved)
    }

    pub(crate) fn complete_migration(&self) -> Result<(), AppError> {
        if !self.migration_pending()? {
            return Ok(());
        }
        let vault = self.read()?;
        let mut saved = read_metadata(&self.root)?;
        saved.migration_state =
            seal_migration_state(vault.encrypted()?, &MigrationState::Complete)?;
        write_metadata(&self.root, &saved)
    }
}

/// Authenticate the current vault without recovering, creating or publishing it.
/// The startup owner must inspect storage and admit any subsequent mutation.
pub(crate) fn authenticate_existing(
    root: &Path,
    store: &dyn KeyStore,
    password: Option<&str>,
) -> Result<VaultContext, AppError> {
    let saved = read_metadata(root)?;
    if let Some(password) = password {
        VaultContext::from_password(saved.metadata, password)
            .map_err(super::inventory::secret_error)
    } else if saved.automatic_unlock {
        let key = store
            .load(&saved.metadata.vault_id, &saved.metadata.key_id)
            .map_err(|_| AppError::Config("secret.store_unavailable".into()))?
            .ok_or_else(|| AppError::Config("secret.key_missing".into()))?;
        super::key_store::user_text::authenticate(saved.metadata, key)
            .map_err(super::inventory::secret_error)
    } else {
        Err(AppError::Config("secret.locked".into()))
    }
}

fn decode_migration_state(vault:&VaultContext,saved:&LocalVault)->Result<MigrationState,AppError>{
    let state=vault.open(&["local","migration-state"],&saved.migration_state).map_err(super::inventory::secret_error)?;
    serde_json::from_slice(&state).map_err(|_|AppError::Config("secret.invalid_metadata".into()))
}
/// Read-only pending-generation admission through the original local-state codec.
pub(crate) fn validate_completed_metadata(root:&Path,context:&StorageContext)->Result<(),AppError>{
    if context.is_plaintext(){if read_metadata_with_bytes(root)?.is_some(){return Err(AppError::Config("protection.identity_mismatch".into()))}return Ok(())}
    let vault=context.encrypted()?;let saved=read_metadata(root)?;
    if saved.metadata!=*vault.metadata()||!matches!(decode_migration_state(vault,&saved)?,MigrationState::Complete){return Err(AppError::Config("secret.migration_required".into()))}Ok(())
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

/// Original local-vault parser, optionally retaining exact nonsecret/ciphertext bytes for review CAS.
pub(crate) fn read_metadata_with_bytes(root: &Path) -> Result<Option<(LocalVault, Vec<u8>)>, AppError> {
    let path=root.join("vault.json");
    crate::config_file_io::read_regular_file(&path,64*1024)
        .map_err(|error| if error.kind()==std::io::ErrorKind::InvalidData {AppError::Config("secret.invalid_metadata".into())}else{AppError::io(&path,error)})?
        .map(|bytes|serde_json::from_slice::<LocalVault>(&bytes).map(|local|(local,bytes)).map_err(|_|AppError::Config("secret.invalid_metadata".into()))).transpose()
}
pub(crate) fn read_metadata(root: &Path) -> Result<LocalVault, AppError> {
    read_metadata_with_bytes(root)?.map(|(local,_)|local).ok_or_else(||AppError::Config("secret.metadata_missing".into()))
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

#[cfg(test)]
mod tests {
    use super::super::key_store::KeyStoreError;
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use zeroize::Zeroizing;

    #[test]
    fn lifecycle_epoch_invalidates_old_leases_without_reviving_on_unblock() {
        let session = SecretSession::ephemeral().expect("session");
        let original = {
            let _guard = session.read().expect("initial read");
            assert_eq!(session.lifecycle_epoch(), 0);
            session.lifecycle_epoch()
        };
        {
            let _guard = session.read().expect("unchanged read");
            assert_eq!(session.lifecycle_epoch(), original);
        }
        session.set_blocked(false);
        assert_eq!(session.lifecycle_epoch(), original);
        session.set_blocked(true);
        assert!(session.read().is_err());
        let first_block = session.lifecycle_epoch();
        assert_eq!(first_block, original + 1);
        session.set_blocked(true);
        assert_eq!(session.lifecycle_epoch(), first_block + 1);
        session.set_blocked(false);
        let _guard = session.read().expect("unblocked read");
        assert_ne!(session.lifecycle_epoch(), original);
        assert_eq!(session.lifecycle_epoch(), first_block + 1);
    }

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
    fn migration_completion_publishes_pending_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoryKeys::default();
        let session = SecretSession::open(dir.path(), &store, None).unwrap();
        assert!(session.migration_pending().unwrap());
        session.complete_migration().unwrap();
        assert!(!session.migration_pending().unwrap());
        let completed = std::fs::read(dir.path().join("vault.json")).unwrap();
        session.complete_migration().unwrap();
        assert!(std::fs::read(dir.path().join("vault.json")).unwrap() == completed);
        authenticate_existing(dir.path(), &store, None).unwrap();
    }

    #[test]
    fn migration_completion_rejects_unverified_state_without_writes() {
        for case in ["damaged", "unknown", "wrong_identity"] {
            let dir = tempfile::tempdir().unwrap();
            let store = MemoryKeys::default();
            let session = SecretSession::open(dir.path(), &store, None).unwrap();
            let mut saved = read_metadata(dir.path()).unwrap();
            saved.migration_state = match case {
                "damaged" => "synthetic-invalid-envelope".into(),
                "unknown" => session
                    .read()
                    .unwrap()
                    .seal(&["local", "migration-state"], br#"{"phase":"future"}"#)
                    .unwrap(),
                "wrong_identity" => session
                    .read()
                    .unwrap()
                    .seal(&["synthetic-other-owner"], br#"{"phase":"complete"}"#)
                    .unwrap(),
                _ => unreachable!(),
            };
            write_metadata(dir.path(), &saved).unwrap();
            let before = std::fs::read(dir.path().join("vault.json")).unwrap();
            assert!(session.complete_migration().is_err(), "{case}");
            assert!(std::fs::read(dir.path().join("vault.json")).unwrap() == before);
        }
    }

    #[test]
    fn inspection_unlock_authenticates_without_recovering_pending_operations() {
        let dir = super::super::testing::tempdir().unwrap();
        let store = MemoryKeys::default();
        let original = SecretSession::open(dir.path(), &store, None).unwrap();
        let ciphertext = original
            .read()
            .unwrap()
            .seal(&["fixture"], b"fixture-value")
            .unwrap();
        let path = dir.path().join(super::super::transition::INTENT);
        std::fs::write(&path, b"pending-operation-fixture").unwrap();
        let metadata = std::fs::read(dir.path().join("vault.json")).unwrap();
        let unlocked = authenticate_existing(dir.path(), &store, None).unwrap();
        assert_eq!(
            &**unlocked.open(&["fixture"], &ciphertext).unwrap(),
            b"fixture-value"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"pending-operation-fixture");
        assert_eq!(
            std::fs::read(dir.path().join("vault.json")).unwrap(),
            metadata
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn inspection_unlock_preserves_password_policy_and_never_creates_storage() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("absent");
        let store = MemoryKeys::default();
        assert!(authenticate_existing(&root, &store, None).is_err());
        assert!(!root.exists());
        let vault = VaultContext::generate()
            .unwrap()
            .with_password("fixture-password")
            .unwrap();
        let cipher = vault.seal(&["fixture"], b"fixture-value").unwrap();
        write_metadata(dir.path(), &completed_metadata(&vault, false).unwrap()).unwrap();
        let before = std::fs::read(dir.path().join("vault.json")).unwrap();
        assert!(
            matches!(authenticate_existing(dir.path(), &store, None), Err(AppError::Config(code)) if code == "secret.locked")
        );
        assert!(authenticate_existing(dir.path(), &store, Some("wrong-fixture-password")).is_err());
        let authenticated =
            authenticate_existing(dir.path(), &store, Some("fixture-password")).unwrap();
        assert_eq!(
            &**authenticated.open(&["fixture"], &cipher).unwrap(),
            b"fixture-value"
        );
        assert_eq!(
            std::fs::read(dir.path().join("vault.json")).unwrap(),
            before
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(store.0.lock().unwrap().is_empty());
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
        let metadata = session
            .read()
            .unwrap()
            .encrypted()
            .unwrap()
            .metadata()
            .clone();
        let before = std::fs::read(dir.path().join("vault.json")).unwrap();
        store.remove(&metadata.vault_id, &metadata.key_id).unwrap();
        assert!(SecretSession::open(dir.path(), &store, None).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("vault.json")).unwrap(),
            before
        );
    }
}

/// Shell visibility is independent from authenticated credential runtime publication.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAvailability {
    pub credential_runtime_available: bool,
}

/// A read-only observation; never acquires an OS key or creates a session.
pub fn credential_runtime_availability(session: Option<&SecretSession>) -> RuntimeAvailability {
    RuntimeAvailability {
        credential_runtime_available: session.is_some_and(|session| session.read().is_ok()),
    }
}

#[cfg(test)]
mod runtime_availability_tests {
    #[test]
    fn blocked_session_is_not_an_available_credential_runtime() {
        let session = super::SecretSession::ephemeral().unwrap();
        assert!(
            super::credential_runtime_availability(Some(&session)).credential_runtime_available
        );
        session.set_blocked(true);
        assert!(
            !super::credential_runtime_availability(Some(&session)).credential_runtime_available
        );
    }
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "session_protection_tests.rs"]
pub(crate) mod protection_startup_tests;
