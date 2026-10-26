//! Registered credential files and their persistence boundary.
use std::path::PathBuf;
use zeroize::Zeroizing;

use super::{session::SecretSession, storage::StorageContext, VaultContext};
use crate::error::AppError;

use super::owned_file::BACKUP_PATTERNS;
pub(crate) use super::owned_file::{CredentialFile, OwnedFile, AUTH_FILES};
use super::owned_file::{DeviceFile, DEVICE_BACKUP_DIR, DEVICE_FILES};

impl CredentialFile {
    pub(crate) fn path(self, session: &SecretSession) -> PathBuf {
        session.root().join(self.filename())
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn encode(
        self,
        vault: &VaultContext,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        OwnedFile::registered(self.filename())?.encode(vault, plaintext)
    }

    pub(crate) fn decode(
        self,
        vault: &VaultContext,
        bytes: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        OwnedFile::registered(self.filename())?.decode(vault, bytes)
    }

    pub(crate) fn encode_storage(
        self,
        storage: &StorageContext,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        OwnedFile::registered(self.filename())?.encode_storage(storage, plaintext)
    }

    pub(crate) fn decode_storage(
        self,
        storage: &StorageContext,
        bytes: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        OwnedFile::registered(self.filename())?.decode_storage(storage, bytes)
    }

    pub(crate) fn read(
        self,
        session: &SecretSession,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        let vault = session.read()?;
        let path = self.path(session);
        match std::fs::read(&path) {
            Ok(bytes) => self.decode_storage(&vault, &bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AppError::io(&path, error)),
        }
    }

    pub(crate) fn write(self, session: &SecretSession, plaintext: &[u8]) -> Result<(), AppError> {
        // Keep the same key generation through encryption, atomic publication and sync.
        let vault = session.read()?;
        let encrypted = self.encode_storage(&vault, plaintext)?;
        let path = self.path(session);
        ensure_plaintext_destination(&vault, &path)?;
        super::session::ensure_owned_directory(
            session.root(),
            path.parent()
                .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?,
        )?;
        super::session::write_durable(&path, &encrypted)
    }

    pub(crate) fn remove(self, session: &SecretSession) -> Result<(), AppError> {
        let storage = session.read()?;
        let path = self.path(session);
        ensure_plaintext_destination(&storage, &path)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::io(&path, error)),
        }
    }
}

/// A plaintext publication/removal must never discard an existing encrypted image.
/// The caller retains its original session guard through this read and publication.
/// This is not a claim of CAS against external processes; original writers own that.
pub(crate) fn ensure_plaintext_destination(
    storage: &StorageContext,
    path: &std::path::Path,
) -> Result<(), AppError> {
    if !storage.is_plaintext() {
        return Ok(());
    }
    if let Some(bytes) = crate::config_file_io::read_regular_file(path, 64 * 1024 * 1024)
        .map_err(|error| AppError::io(path, error))?
    {
        storage
            .decode_bytes(&["plaintext-destination"], &bytes)
            .map_err(crate::secrets::error::secret_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_removes_plaintext_and_binds_the_registered_file() {
        let vault = VaultContext::generate().unwrap();
        for file in AUTH_FILES {
            let encrypted = file
                .encode(&vault, br#"{"token":"credential-canary"}"#)
                .unwrap();
            assert!(!String::from_utf8_lossy(&encrypted).contains("credential-canary"));
            assert_eq!(
                &*file.decode(&vault, &encrypted).unwrap(),
                br#"{"token":"credential-canary"}"#
            );
            for other in AUTH_FILES.into_iter().filter(|other| *other != file) {
                assert!(other.decode(&vault, &encrypted).is_err());
            }
        }
    }

    #[test]
    fn file_store_reads_only_authenticated_data_and_writes_private_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let session = SecretSession::from_context(
            dir.path().to_path_buf(),
            VaultContext::generate().unwrap(),
        );
        let file = CredentialFile::Codex;
        assert!(file.read(&session).unwrap().is_none());
        file.write(&session, b"credential-canary").unwrap();
        let raw = std::fs::read(file.path(&session)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("credential-canary"));
        assert_eq!(
            &*file.read(&session).unwrap().unwrap(),
            b"credential-canary"
        );
        let unrelated = SecretSession::from_context(
            dir.path().to_path_buf(),
            VaultContext::generate().unwrap(),
        );
        assert!(file.read(&unrelated).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(file.path(&session))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::write(file.path(&session), b"plaintext-canary").unwrap();
        assert!(file.read(&session).is_err());
        file.remove(&session).unwrap();
        assert!(file.read(&session).unwrap().is_none());
    }
}

pub(crate) struct OwnedFileMigration {
    pub file: OwnedFile,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub ciphertext: Vec<u8>,
}

pub(crate) struct DeviceFileMigration {
    pub file: DeviceFile,
    pub source: PathBuf,
    pub ciphertext: Vec<u8>,
}

/// Read-only authenticated inventory of fixed-device files and first-write backups.
/// These files are not part of ordinary credential exports or vault-recovery copies.
pub(crate) fn stage_device_files_with_vault(
    device_root: &std::path::Path,
    vault: &VaultContext,
) -> Result<Vec<DeviceFileMigration>, AppError> {
    stage_device_file_inventory(device_root, |file, bytes| {
        file.decode(vault, bytes).map(|_| ())
    })
}

fn stage_device_file_inventory(
    device_root: &std::path::Path,
    decode: impl Fn(&DeviceFile, &[u8]) -> Result<(), AppError>,
) -> Result<Vec<DeviceFileMigration>, AppError> {
    device_file_paths(device_root)?
        .into_iter()
        .map(|(file, source)| {
            let ciphertext =
                std::fs::read(&source).map_err(|error| AppError::io(&source, error))?;
            decode(&file, &ciphertext)?;
            Ok(DeviceFileMigration {
                file,
                source,
                ciphertext,
            })
        })
        .collect()
}

/// Guarded, deterministic membership only. This does not authenticate bytes;
/// transition recovery must separately verify each captured source/target generation.
pub(super) fn device_file_paths(
    device_root: &std::path::Path,
) -> Result<Vec<(DeviceFile, PathBuf)>, AppError> {
    device_paths(device_root, false)
}

/// Key-loss reset archives opaque bytes, including interrupted adjacent writes.
/// Only registered targets in the fixed device root grant staging membership.
pub(super) fn device_reset_paths(
    device_root: &std::path::Path,
) -> Result<Vec<(DeviceFile, PathBuf)>, AppError> {
    device_paths(device_root, true)
}

pub(super) fn device_reset_file(relative: &str) -> Result<DeviceFile, AppError> {
    DeviceFile::registered(relative).or_else(|_| {
        crate::config_file_io::staging_target_name(relative)
            .ok_or_else(|| AppError::Config("secret.unregistered_file".into()))
            .and_then(DeviceFile::registered)
    })
}

fn device_paths(
    device_root: &std::path::Path,
    include_staging: bool,
) -> Result<Vec<(DeviceFile, PathBuf)>, AppError> {
    if !device_directory_exists(device_root)? {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for name in DEVICE_FILES {
        let source = device_root.join(name);
        match std::fs::symlink_metadata(&source) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(AppError::io(&source, error)),
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                return Err(AppError::Config("secret.invalid_storage_path".into()));
            }
            Ok(_) => paths.push((DeviceFile::registered(name)?, source)),
        }
    }
    if include_staging {
        for entry in
            std::fs::read_dir(device_root).map_err(|error| AppError::io(device_root, error))?
        {
            let entry = entry.map_err(|error| AppError::io(device_root, error))?;
            let name = entry.file_name();
            let Some(target) = name
                .to_str()
                .and_then(crate::config_file_io::staging_target_name)
            else {
                continue;
            };
            if !DEVICE_FILES.contains(&target) {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|error| AppError::io(entry.path(), error))?
                .is_file()
            {
                return Err(AppError::Config("secret.invalid_storage_path".into()));
            }
            paths.push((DeviceFile::registered(target)?, entry.path()));
        }
    }
    let backup = device_root.join(DEVICE_BACKUP_DIR);
    if device_directory_exists(&backup)? {
        for entry in std::fs::read_dir(&backup).map_err(|error| AppError::io(&backup, error))? {
            let entry = entry.map_err(|error| AppError::io(&backup, error))?;
            // This dedicated directory has no unrelated members to silently skip.
            let name = entry.file_name();
            let file = if include_staging {
                let name = name
                    .to_str()
                    .ok_or_else(|| AppError::Config("secret.unregistered_file".into()))?;
                device_reset_file(&format!("{DEVICE_BACKUP_DIR}/{name}"))?
            } else {
                device_backup_file(&name)?
            };
            if !entry
                .file_type()
                .map_err(|error| AppError::io(entry.path(), error))?
                .is_file()
            {
                return Err(AppError::Config("secret.invalid_storage_path".into()));
            }
            paths.push((file, entry.path()));
        }
    }
    paths.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(paths)
}

fn device_backup_file(name: &std::ffi::OsStr) -> Result<DeviceFile, AppError> {
    let name = name
        .to_str()
        .ok_or_else(|| AppError::Config("secret.unregistered_file".into()))?;
    // PathBuf::join would introduce an alternate separator on Windows. Device AAD
    // always uses the exact portable registry spelling, not an OS path alias.
    DeviceFile::registered(format!("{DEVICE_BACKUP_DIR}/{name}"))
}

/// Validate existing ancestors without creating directories, tightening permissions,
/// or resolving a symlink into permission to read a different owned root.
pub(crate) fn device_directory_exists(path: &std::path::Path) -> Result<bool, AppError> {
    crate::config_file_io::directory_exists_no_alias(path)
}

impl OwnedFile {
    pub(crate) fn at_path(
        session: &SecretSession,
        path: &std::path::Path,
    ) -> Result<Self, AppError> {
        Self::registered(
            path.strip_prefix(session.root())
                .map_err(|_| AppError::Config("secret.unregistered_file".into()))?,
        )
    }

    pub(crate) fn path(&self, session: &SecretSession) -> PathBuf {
        session.root().join(&self.relative)
    }

    pub(crate) fn read(&self, session: &SecretSession) -> Result<Zeroizing<Vec<u8>>, AppError> {
        let vault = session.read()?;
        let path = self.path(session);
        let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
        self.decode_storage(&vault, &bytes)
    }

    pub(crate) fn write(&self, session: &SecretSession, plaintext: &[u8]) -> Result<(), AppError> {
        let vault = session.read()?;
        let bytes = self.encode_storage(&vault, plaintext)?;
        let path = self.path(session);
        ensure_plaintext_destination(&vault, &path)?;
        super::session::ensure_owned_directory(
            session.root(),
            path.parent()
                .ok_or_else(|| AppError::Config("secret.invalid_path".into()))?,
        )?;
        super::session::write_durable(&path, &bytes)
    }
}

pub(crate) fn stage_owned_files_with_vault(
    root: &std::path::Path,
    vault: &VaultContext,
    legacy_allowed: bool,
) -> Result<Vec<OwnedFileMigration>, AppError> {
    let paths = owned_file_paths(root)?;
    let mut plans = Vec::new();
    for (file, source) in paths {
        let bytes = Zeroizing::new(std::fs::read(&source).map_err(|e| AppError::io(&source, e))?);
        let recovery = source.starts_with(root.join("backups/vault-recovery"));
        let ciphertext = if bytes.starts_with(b"lpenc")
            || !legacy_allowed
            || recovery
            || !file.allows_legacy_plaintext()
        {
            file.decode(vault, &bytes)?;
            bytes.to_vec()
        } else {
            validate_legacy_document(&file, &bytes)?;
            file.encode(vault, &bytes)?
        };
        plans.push(OwnedFileMigration {
            file,
            destination: source.clone(),
            source,
            ciphertext,
        });
    }
    Ok(plans)
}

fn owned_file_paths(root: &std::path::Path) -> Result<Vec<(OwnedFile, PathBuf)>, AppError> {
    owned_paths(root, false)
}

/// Passive membership, including interrupted files from the original durable writer.
/// The ordinary codec inventory deliberately does not decode these partial writes.
pub(super) fn owned_file_paths_with_staging(
    root: &std::path::Path,
) -> Result<Vec<(OwnedFile, PathBuf)>, AppError> {
    owned_paths(root, true)
}

fn owned_paths(
    root: &std::path::Path,
    include_staging: bool,
) -> Result<Vec<(OwnedFile, PathBuf)>, AppError> {
    let mut paths = Vec::new();
    let directories = std::iter::once("")
        .chain(BACKUP_PATTERNS.iter().map(|entry| entry.0))
        .flat_map(|directory| {
            [
                PathBuf::from(directory),
                PathBuf::from("backups/vault-recovery").join(directory),
            ]
        })
        .collect::<std::collections::BTreeSet<_>>();
    for directory in directories {
        let path = root.join(&directory);
        if !device_directory_exists(&path)? {
            continue;
        }
        for entry in std::fs::read_dir(&path).map_err(|e| AppError::io(&path, e))? {
            let entry = entry.map_err(|e| AppError::io(&path, e))?;
            let relative = directory.join(entry.file_name());
            let identity = relative
                .strip_prefix("backups/vault-recovery")
                .unwrap_or(&relative);
            let file = OwnedFile::registered(identity).or_else(|error| {
                if !include_staging {
                    return Err(error);
                }
                identity
                    .to_str()
                    .and_then(crate::config_file_io::staging_target_name)
                    .ok_or(error)
                    .and_then(OwnedFile::registered)
            });
            if let Ok(file) = file {
                if !entry
                    .file_type()
                    .map_err(|e| AppError::io(entry.path(), e))?
                    .is_file()
                {
                    return Err(AppError::Config("secret.invalid_storage_path".into()));
                }
                paths.push((file, entry.path()));
            }
        }
    }
    paths.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(paths)
}

fn validate_legacy_document(file: &OwnedFile, bytes: &[u8]) -> Result<(), AppError> {
    let invalid = || AppError::Config("secret.invalid_legacy_file".into());
    let extension = file
        .relative
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("");
    let valid = match extension {
        "yaml" | "yml" => serde_yaml::from_slice::<serde_yaml::Value>(bytes)
            .map(|v| v.is_mapping())
            .map_err(|_| invalid())?,
        "json5" => {
            json5::from_str::<serde_json::Value>(std::str::from_utf8(bytes).map_err(|_| invalid())?)
                .map(|v| v.is_object())
                .map_err(|_| invalid())?
        }
        _ => serde_json::from_slice::<serde_json::Value>(bytes)
            .map(|v| v.is_object())
            .map_err(|_| invalid())?,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod owned_tests {
    use super::*;
    #[test]
    fn account_files_never_take_the_legacy_plaintext_upgrade_path() {
        for name in [
            super::super::owned_file::PROFILE_FILE,
            super::super::owned_file::JOURNAL_FILE,
            super::super::owned_file::RECOVERY_FILE,
            super::super::owned_file::OPERATION_FILE,
            super::super::owned_file::KEY_INTENT_FILE,
            super::super::owned_file::BUNDLE_EXPORT_FILE,
        ] {
            let root = tempfile::tempdir().unwrap();
            let vault = VaultContext::generate().unwrap();
            std::fs::write(root.path().join(name), b"{}").unwrap();
            assert!(stage_owned_files_with_vault(root.path(), &vault, true).is_err());
            assert_eq!(std::fs::read(root.path().join(name)).unwrap(), b"{}");
            let file = OwnedFile::registered(name).unwrap();
            std::fs::write(root.path().join(name), file.encode(&vault, b"{}").unwrap()).unwrap();
            assert_eq!(
                stage_owned_files_with_vault(root.path(), &vault, true)
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    #[test]
    fn migration_stages_registered_credentials_and_rejects_plaintext_after_upgrade() {
        let root = tempfile::tempdir().unwrap();
        let vault = VaultContext::generate().unwrap();
        std::fs::create_dir_all(root.path().join("backups/hermes")).unwrap();
        std::fs::write(
            root.path()
                .join("backups/hermes/hermes_20260914_010203.yaml"),
            "key: backup-canary\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("config.json.bak"),
            r#"{"key":"backup-canary"}"#,
        )
        .unwrap();
        std::fs::write(root.path().join("backups/unrelated.json"), "unrelated").unwrap();
        assert!(stage_owned_files_with_vault(root.path(), &vault, false).is_err());
        let plans = stage_owned_files_with_vault(root.path(), &vault, true).unwrap();
        assert_eq!(plans.len(), 2);
        for plan in plans {
            assert!(!String::from_utf8_lossy(&plan.ciphertext).contains("backup-canary"));
            assert!(
                String::from_utf8_lossy(&plan.file.decode(&vault, &plan.ciphertext).unwrap())
                    .contains("backup-canary")
            );
            assert!(
                String::from_utf8_lossy(&std::fs::read(&plan.source).unwrap())
                    .contains("backup-canary")
            );
            std::fs::write(&plan.destination, &plan.ciphertext).unwrap();
        }
        assert_eq!(
            stage_owned_files_with_vault(root.path(), &vault, false)
                .unwrap()
                .len(),
            2
        );
        let recovery = root.path().join("backups/vault-recovery");
        std::fs::create_dir_all(&recovery).unwrap();
        std::fs::write(recovery.join("config.json.bak"), b"lpenc1.invalid").unwrap();
        assert!(stage_owned_files_with_vault(root.path(), &vault, true).is_err());
        assert_eq!(
            std::fs::read_to_string(root.path().join("backups/unrelated.json")).unwrap(),
            "unrelated"
        );
        assert!(OwnedFile::registered("../config.json").is_err());
        assert!(OwnedFile::registered("backups/unrelated.json").is_err());
    }
}

#[cfg(test)]
#[path = "files/device_inventory_tests.rs"]
mod device_inventory_tests;

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) mod plaintext_runtime_tests {
    use super::*;
    use crate::secrets::storage::{PlaintextIdentity, StorageContext};

    #[cfg_attr(test, test)]
    fn plaintext_credential_writes_and_removal_preserve_existing_ciphertext() {
        let root = tempfile::tempdir().unwrap();
        let session = SecretSession::from_storage_context(
            root.path().into(),
            StorageContext::Plaintext(PlaintextIdentity {
                format_version: 1,
                store_id: "11111111-1111-4111-8111-111111111111".into(),
                generation_id: "22222222-2222-4222-8222-222222222222".into(),
                revision: 1,
            }),
        );
        let file = CredentialFile::Codex;
        let opaque = b"lpenc:synthetic-unavailable-key";
        std::fs::write(file.path(&session), opaque).unwrap();
        assert!(file.write(&session, br#"{"token":"new"}"#).is_err());
        assert!(file.remove(&session).is_err());
        assert_eq!(std::fs::read(file.path(&session)).unwrap(), opaque);
        let owned = OwnedFile::registered(crate::secrets::owned_file::WORKBUDDY_FILE).unwrap();
        std::fs::write(owned.path(&session), opaque).unwrap();
        assert!(owned.write(&session, br#"{"accounts":[]}"#).is_err());
        assert_eq!(std::fs::read(owned.path(&session)).unwrap(), opaque);
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn run() {
        plaintext_credential_writes_and_removal_preserve_existing_ciphertext();
    }
}

/// Strict inventory for an explicitly selected storage generation. No legacy adoption.
pub(crate) fn stage_owned_files_with_storage(
    root: &std::path::Path,
    storage: &StorageContext,
    legacy_allowed: bool,
) -> Result<Vec<OwnedFileMigration>, AppError> {
    if legacy_allowed {
        return Err(AppError::Config(
            "protection.legacy_plaintext_not_allowed".into(),
        ));
    }
    validate_storage_inventory_context(storage)?;
    owned_file_paths(root)?
        .into_iter()
        .map(|(file, source)| {
            let bytes = Zeroizing::new(
                std::fs::read(&source).map_err(|error| AppError::io(&source, error))?,
            );
            file.decode_storage(storage, &bytes)?;
            Ok(OwnedFileMigration {
                file,
                destination: source.clone(),
                source,
                // The established plan field holds the unchanged source encoding.
                ciphertext: bytes.to_vec(),
            })
        })
        .collect()
}

pub(crate) fn stage_device_files_with_storage(
    device_root: &std::path::Path,
    storage: &StorageContext,
) -> Result<Vec<DeviceFileMigration>, AppError> {
    validate_storage_inventory_context(storage)?;
    stage_device_file_inventory(device_root, |file, bytes| {
        file.decode_storage(storage, bytes).map(|_| ())
    })
}

fn validate_storage_inventory_context(storage: &StorageContext) -> Result<(), AppError> {
    if let StorageContext::Plaintext(identity) = storage {
        identity.validate().map_err(super::error::secret_error)?;
    }
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) mod explicit_storage_inventory_tests {
    use super::*;
    use crate::secrets::storage::PlaintextIdentity;

    fn plaintext() -> StorageContext {
        StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: "11111111-1111-4111-8111-111111111111".into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        })
    }

    #[cfg_attr(test, test)]
    fn explicit_storage_inventory_keeps_exact_plaintext_source_and_registered_membership() {
        let root = tempfile::tempdir().unwrap();
        let recovery = root.path().join("backups/vault-recovery");
        std::fs::create_dir_all(&recovery).unwrap();
        let original = br#"{"synthetic":"credential-canary","foreign":18446744073709551615}"#;
        std::fs::write(root.path().join("workbuddy_accounts.json"), original).unwrap();
        std::fs::write(recovery.join("config.json.bak"), original).unwrap();
        std::fs::write(root.path().join("unregistered.json"), b"lpenc:untouched").unwrap();
        let plans = stage_owned_files_with_storage(root.path(), &plaintext(), false).unwrap();
        assert_eq!(plans.len(), 2);
        for plan in plans {
            assert_eq!(plan.source, plan.destination);
            assert_eq!(plan.ciphertext, original);
            assert_eq!(std::fs::read(plan.source).unwrap(), original);
        }
        assert_eq!(
            std::fs::read(root.path().join("unregistered.json")).unwrap(),
            b"lpenc:untouched"
        );
        assert!(!root.path().join("vault.json").exists());
    }

    #[cfg_attr(test, test)]
    fn explicit_storage_inventory_keeps_authenticated_encrypted_source_and_refuses_mixed_modes() {
        let root = tempfile::tempdir().unwrap();
        let encrypted = StorageContext::Encrypted(VaultContext::generate().unwrap());
        let file = OwnedFile::registered("workbuddy_accounts.json").unwrap();
        let original = file
            .encode_storage(&encrypted, br#"{"accounts":[]}"#)
            .unwrap();
        let path = root.path().join(file.relative_path());
        std::fs::write(&path, &original).unwrap();
        let plans = stage_owned_files_with_storage(root.path(), &encrypted, false).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].ciphertext, original);
        assert!(stage_owned_files_with_storage(root.path(), &plaintext(), false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::write(&path, br#"{"accounts":[]}"#).unwrap();
        assert!(stage_owned_files_with_storage(root.path(), &encrypted, false).is_err());
        assert!(stage_owned_files_with_storage(root.path(), &plaintext(), true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"accounts":[]}"#);
    }

    #[cfg_attr(test, test)]
    fn explicit_storage_device_inventory_preserves_binary_backups_and_original_guards() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let relative = format!("{DEVICE_BACKUP_DIR}/{}.backup", "a".repeat(64));
        let path = root.path().join(&relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = [0xff, 0xfe, 0, 7];
        std::fs::write(&path, original).unwrap();
        let plans = stage_device_files_with_storage(root.path(), &plaintext()).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].file.relative_path(),
            std::path::Path::new(&relative)
        );
        assert_eq!(plans[0].ciphertext, original);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::write(&path, b"lpenc:unavailable-key").unwrap();
        assert!(stage_device_files_with_storage(root.path(), &plaintext()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"lpenc:unavailable-key");
        #[cfg(unix)]
        {
            std::fs::remove_file(&path).unwrap();
            let other = root.path().join("outside");
            std::fs::write(&other, b"synthetic").unwrap();
            std::os::unix::fs::symlink(&other, &path).unwrap();
            assert!(stage_device_files_with_storage(root.path(), &plaintext()).is_err());
            assert_eq!(std::fs::read(&other).unwrap(), b"synthetic");
        }
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn run_case(case: &str) {
        match case {
            "inventory_plaintext_membership" => explicit_storage_inventory_keeps_exact_plaintext_source_and_registered_membership(),
            "inventory_encrypted_strict" => explicit_storage_inventory_keeps_authenticated_encrypted_source_and_refuses_mixed_modes(),
            "device_inventory_binary_guards" => explicit_storage_device_inventory_preserves_binary_backups_and_original_guards(),
            _ => panic!("unknown explicit storage verification case: {case}"),
        }
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn run() {
        run_case("inventory_plaintext_membership");
        run_case("inventory_encrypted_strict");
        run_case("device_inventory_binary_guards");
    }
}
