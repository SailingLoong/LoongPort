//! Startup owns passive inspection before any recovery or runtime publication.

use super::{session, VaultContext, VaultMetadata};
use crate::{
    database::{self, inspection, Database},
    error::AppError,
    live::engine::DeviceStore,
};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SchemaVersions {
    pub upstream: i32,
    pub loongport: i32,
}

pub(crate) enum UpgradeInspection {
    Stable(Box<StableInspection>),
    RecoveryRequired(RecoveryEvidence),
}

/// Private journal captures bind a user action to the inspected operation.
/// Only the digest is exposed; each original owner authenticates its own bytes.
pub(crate) struct RecoveryEvidence {
    records: [Option<Vec<u8>>; 4],
    device: DeviceStore,
}
impl RecoveryEvidence {
    fn capture(root: &Path, device: &DeviceStore) -> Result<Self, AppError> {
        Ok(Self {
            records: Self::records(root)?,
            device: device.clone(),
        })
    }
    fn records(root: &Path) -> Result<[Option<Vec<u8>>; 4], AppError> {
        Ok([
            super::reset::pending_record(root)?,
            super::bootstrap_restore::pending_record(root)?,
            super::transition::pending_record(root)?,
            super::rewrap::pending_record(root)?,
        ])
    }
    pub(crate) fn token(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        for record in &self.records {
            match record {
                Some(bytes) => {
                    digest.update([1]);
                    digest.update((bytes.len() as u64).to_le_bytes());
                    digest.update(bytes);
                }
                None => digest.update([0]),
            }
        }
        hex::encode(digest.finalize())
    }
    pub(crate) fn verify_unchanged(&self, root: &Path, token: &str) -> Result<(), AppError> {
        if token != self.token() || Self::records(root)? != self.records {
            return Err(changed());
        }
        Ok(())
    }
    pub(crate) fn recover(
        &self,
        root: &Path,
        token: &str,
        store: &dyn super::key_store::KeyStore,
        password: &str,
    ) -> Result<UpgradeInspection, AppError> {
        self.verify_unchanged(root, token)?;
        if let Some(record) = &self.records[0] {
            super::reset::recover_inspected(root, Some(password), record)?;
        }
        if let Some(record) = &self.records[1] {
            super::bootstrap_restore::recover_inspected(root, store, Some(password), record)?;
        }
        // Bootstrap recovery may have settled its child transition itself.
        if let Some(record) = &self.records[2] {
            if self.records[1].is_none() || super::transition::pending_record(root)?.is_some() {
                super::transition::recover_inspected(
                    root,
                    self.device.root(),
                    store,
                    Some(password),
                    record,
                )?;
            }
        }
        if let Some(record) = &self.records[3] {
            super::rewrap::recover_password_inspected(root, store, Some(password), record)?;
        }
        let inspected = inspect(root, &self.device)?;
        inspected.verify_unchanged(root)?;
        if inspected.future_version().is_some() {
            return Err(AppError::Config("upgrade.future_version".into()));
        }
        let vault = session::authenticate_existing(root, store, Some(password))?;
        inspected.validate_device_state(&vault)?;
        inspected.validate_database(root, &vault)?;
        inspected.verify_unchanged(root)?;
        Ok(inspected)
    }
}

pub(crate) struct StableInspection {
    pub source_versions: Option<SchemaVersions>,
    revision: Option<inspection::SourceRevision>,
    vault: Option<(VaultMetadata, bool, String)>,
    device: DeviceStore,
    device_files: Vec<(super::owned_file::DeviceFile, std::path::PathBuf, Vec<u8>)>,
}

impl UpgradeInspection {
    pub(crate) fn ensure_runtime_admitted(&self) -> Result<(), AppError> {
        let device = match self {
            Self::Stable(stable) => &stable.device,
            Self::RecoveryRequired(evidence) => &evidence.device,
        };
        checkpoint::ensure_sync_admitted(device)
    }
    pub(crate) fn is_recovery_required(&self) -> bool {
        matches!(self, Self::RecoveryRequired(_))
    }
    pub(crate) fn future_version(&self) -> Option<(i32, i32)> {
        match self {
            Self::Stable(inspected) => inspected.future_version(),
            Self::RecoveryRequired(_) => None,
        }
    }
    pub(crate) fn has_vault(&self) -> bool {
        matches!(self, Self::Stable(inspected) if inspected.vault.is_some())
    }
    pub(crate) fn has_device_files(&self) -> bool {
        matches!(self, Self::Stable(inspected) if !inspected.device_files.is_empty())
    }
    pub(crate) fn verify_unchanged(&self, root: &Path) -> Result<(), AppError> {
        match self {
            Self::Stable(inspected) => inspected.verify_unchanged(root),
            Self::RecoveryRequired(_) => Err(AppError::Config("secret.recovery_required".into())),
        }
    }
    pub(crate) fn validate_device_state(&self, vault: &VaultContext) -> Result<(), AppError> {
        match self {
            Self::Stable(inspected) => inspected.validate_device_state(vault),
            Self::RecoveryRequired(_) => Err(AppError::Config("secret.recovery_required".into())),
        }
    }
    pub(crate) fn validate_database(
        &self,
        root: &Path,
        vault: &VaultContext,
    ) -> Result<(), AppError> {
        self.verify_unchanged(root)?;
        if let Some(db) = inspection::capture(&root.join(crate::config::DB_FILE_NAME))? {
            database::vault::check_identity(&db.image, vault)?;
            super::inventory::validate_database(&db.image, vault)?;
        }
        self.verify_unchanged(root)
    }
}

impl StableInspection {
    pub(crate) fn future_version(&self) -> Option<(i32, i32)> {
        let versions = self.source_versions?;
        if versions.upstream > database::SCHEMA_VERSION {
            Some((versions.upstream, database::SCHEMA_VERSION))
        } else if versions.loongport > database::loongport_schema::LOONGPORT_SCHEMA_VERSION {
            Some((
                versions.loongport,
                database::loongport_schema::LOONGPORT_SCHEMA_VERSION,
            ))
        } else {
            None
        }
    }

    pub(crate) fn verify_unchanged(&self, root: &Path) -> Result<(), AppError> {
        if pending_generation(root)? {
            return Err(changed());
        }
        let path = root.join(crate::config::DB_FILE_NAME);
        match &self.revision {
            Some(revision) => inspection::verify_unchanged(&path, revision)?,
            None if inspection::capture(&path)?.is_some() => return Err(changed()),
            None => {}
        }
        if read_vault(root)? != self.vault || read_device(&self.device)? != self.device_files {
            return Err(changed());
        }
        Ok(())
    }

    pub(crate) fn validate_device_state(&self, vault: &VaultContext) -> Result<(), AppError> {
        for (file, path, bytes) in &self.device_files {
            let plaintext = file.decode(vault, bytes)?;
            if path == &self.device.state_path() {
                crate::mode::state::decode(&plaintext)
                    .map_err(|_| AppError::Config("upgrade.invalid_mode_state".into()))?;
            }
        }
        Ok(())
    }
}

fn changed() -> AppError {
    AppError::Config("upgrade.source_changed".into())
}

fn pending_generation(root: &Path) -> Result<bool, AppError> {
    Ok(super::reset::pending(root)?
        || super::bootstrap_restore::pending(root)?
        || super::rewrap::pending(root)?)
}

fn read_vault(root: &Path) -> Result<Option<(VaultMetadata, bool, String)>, AppError> {
    super::files::device_directory_exists(root)?;
    match std::fs::symlink_metadata(root.join("vault.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::io(root.join("vault.json"), e)),
        Ok(_) => {
            let saved = session::read_metadata(root)?;
            saved
                .metadata
                .validate()
                .map_err(super::error::secret_error)?;
            Ok(Some((
                saved.metadata,
                saved.automatic_unlock,
                saved.migration_state,
            )))
        }
    }
}

fn read_device(
    device: &DeviceStore,
) -> Result<Vec<(super::owned_file::DeviceFile, std::path::PathBuf, Vec<u8>)>, AppError> {
    super::files::device_file_paths(device.root())?
        .into_iter()
        .map(|(file, path)| {
            let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
            Ok((file, path, bytes))
        })
        .collect()
}

pub(crate) fn inspect(root: &Path, device: &DeviceStore) -> Result<UpgradeInspection, AppError> {
    super::files::device_directory_exists(root)?;
    // A generation journal belongs to its authenticated recovery owner. Its
    // database, local metadata and device files may be between generations.
    if pending_generation(root)? {
        return Ok(UpgradeInspection::RecoveryRequired(
            RecoveryEvidence::capture(root, device)?,
        ));
    }
    let captured = inspection::capture(&root.join(crate::config::DB_FILE_NAME))?;
    let source_versions = captured
        .as_ref()
        .map(|db| {
            Ok::<_, AppError>(SchemaVersions {
                upstream: Database::get_user_version(&db.image)?,
                loongport: database::loongport_schema::read_stored_version(&db.image)?,
            })
        })
        .transpose()?;
    let mut result = StableInspection {
        source_versions,
        revision: captured.as_ref().map(|db| db.revision.clone()),
        vault: None,
        device: device.clone(),
        device_files: Vec::new(),
    };
    // An older binary must still present its existing newer-database recovery.
    if result.future_version().is_some() {
        return Ok(UpgradeInspection::Stable(Box::new(result)));
    }
    result.vault = read_vault(root)?;
    result.device_files = read_device(device)?;
    if let Some(db) = captured.as_ref() {
        database::vault::preflight_connection(&db.image)?;
        let stored = database::vault::stored_metadata(&db.image)?;
        match (&stored, &result.vault) {
            (Some(_), None) => return Err(AppError::Config("secret.metadata_missing".into())),
            (Some(stored), Some((local, _, _))) if stored != local => {
                return Err(AppError::Config("secret.identity_mismatch".into()))
            }
            _ => {}
        }
    }
    result.verify_unchanged(root)?;
    Ok(UpgradeInspection::Stable(Box::new(result)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_inspection_reads_both_version_domains_and_rejects_malformed_counter() {
        let dir = super::super::testing::tempdir().unwrap();
        let path = dir.path().join(crate::config::DB_FILE_NAME);
        let device = DeviceStore::at(dir.path().join("device"));
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version=17; CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER)").unwrap();
        conn.execute(
            "INSERT INTO loongport_schema_version VALUES(1,?1)",
            [database::loongport_schema::LOONGPORT_SCHEMA_VERSION + 1],
        )
        .unwrap();
        drop(conn);
        let original = std::fs::read(&path).unwrap();
        let inspected = inspect(dir.path(), &device).unwrap();
        assert_eq!(
            inspected.future_version(),
            Some((
                database::loongport_schema::LOONGPORT_SCHEMA_VERSION + 1,
                database::loongport_schema::LOONGPORT_SCHEMA_VERSION
            ))
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("UPDATE loongport_schema_version SET version='malformed'")
            .unwrap();
        drop(conn);
        let original = std::fs::read(&path).unwrap();
        assert!(inspect(dir.path(), &device).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn startup_inspection_neither_creates_fresh_storage_nor_migrates_existing_tables() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("source");
        let device = DeviceStore::at(dir.path().join("device"));
        let inspected = inspect(&root, &device).unwrap();
        assert!(!inspected.has_vault());
        assert!(!root.exists());
        assert!(!device.root().exists());
        crate::config_file_io::ensure_private_directory(&root).unwrap();
        let path = root.join(crate::config::DB_FILE_NAME);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE unrelated(id INTEGER)")
            .unwrap();
        drop(conn);
        let original = std::fs::read(&path).unwrap();
        let inspected = inspect(&root, &device).unwrap();
        assert!(
            matches!(inspected, UpgradeInspection::Stable(ref stable) if stable.source_versions == Some(SchemaVersions { upstream: 0, loongport: 0 }))
        );
        inspected.verify_unchanged(&root).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn startup_inspection_accepts_matching_vault_and_leaves_encrypted_database_unchanged() {
        let dir = super::super::testing::tempdir().unwrap();
        let path = dir.path().join(crate::config::DB_FILE_NAME);
        let device = DeviceStore::at(dir.path().join("device"));
        let vault = VaultContext::generate().unwrap();
        session::write_metadata(
            dir.path(),
            &session::completed_metadata(&vault, false).unwrap(),
        )
        .unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version=17; CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER); INSERT INTO loongport_schema_version VALUES(1,24)").unwrap();
        database::vault::stamp(&conn, &vault).unwrap();
        drop(conn);
        let original = std::fs::read(&path).unwrap();
        let inspected = inspect(dir.path(), &device).unwrap();
        assert!(inspected.has_vault());
        inspected.validate_device_state(&vault).unwrap();
        inspected.verify_unchanged(dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn startup_inspection_defers_interrupted_generation_to_its_recovery_owner() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("source");
        let device = DeviceStore::at(dir.path().join("device"));
        crate::config_file_io::ensure_private_directory(&root).unwrap();
        let marker = root.join(super::super::transition::INTENT);
        let initial = inspect(&root, &device).unwrap();
        std::fs::write(&marker, b"pending-generation-fixture").unwrap();
        assert!(
            matches!(initial.verify_unchanged(&root), Err(AppError::Config(code)) if code == "upgrade.source_changed")
        );
        let path = root.join(crate::config::DB_FILE_NAME);
        std::fs::write(&path, b"database-between-generations").unwrap();
        std::fs::write(root.join("vault.json"), b"metadata-between-generations").unwrap();
        let inspected = inspect(&root, &device).unwrap();
        assert!(matches!(inspected, UpgradeInspection::RecoveryRequired(_)));
        assert!(
            matches!(inspected.verify_unchanged(&root), Err(AppError::Config(code)) if code == "secret.recovery_required")
        );
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"pending-generation-fixture"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"database-between-generations"
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 3);
    }

    #[test]
    fn startup_inspection_keeps_future_version_recovery_before_vault_parsing() {
        let dir = super::super::testing::tempdir().unwrap();
        let path = dir.path().join(crate::config::DB_FILE_NAME);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", database::SCHEMA_VERSION + 1)
            .unwrap();
        drop(conn);
        std::fs::write(dir.path().join("vault.json"), b"future-format-fixture").unwrap();
        let original = std::fs::read(&path).unwrap();
        let inspected = inspect(dir.path(), &DeviceStore::at(dir.path().join("device"))).unwrap();
        assert_eq!(
            inspected.future_version(),
            Some((database::SCHEMA_VERSION + 1, database::SCHEMA_VERSION))
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn startup_inspection_is_passive_and_blocks_changed_vault_or_device_membership() {
        let dir = super::super::testing::tempdir().unwrap();
        let root = dir.path().join("source");
        let device = DeviceStore::at(dir.path().join("device"));
        crate::config_file_io::ensure_private_directory(&root).unwrap();
        let vault = VaultContext::generate().unwrap();
        session::write_metadata(&root, &session::completed_metadata(&vault, false).unwrap())
            .unwrap();
        let before = std::fs::read(root.join("vault.json")).unwrap();
        let inspected = inspect(&root, &device).unwrap();
        assert!(inspected.has_vault());
        assert!(!inspected.has_device_files());
        inspected.verify_unchanged(&root).unwrap();
        assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), before);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        crate::config_file_io::ensure_private_directory(device.root()).unwrap();
        std::fs::write(device.state_path(), b"device-fixture").unwrap();
        assert!(
            matches!(inspected.verify_unchanged(&root), Err(AppError::Config(code)) if code == "upgrade.source_changed")
        );
        std::fs::remove_file(device.state_path()).unwrap();
        let saved = session::completed_metadata(&vault, true).unwrap();
        session::write_metadata(&root, &saved).unwrap();
        assert!(
            matches!(inspected.verify_unchanged(&root), Err(AppError::Config(code)) if code == "upgrade.source_changed")
        );
    }

    #[test]
    fn startup_inspection_refuses_encrypted_database_without_local_vault() {
        let dir = super::super::testing::tempdir().unwrap();
        let path = dir.path().join(crate::config::DB_FILE_NAME);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA user_version=17; CREATE TABLE loongport_schema_version(id INTEGER PRIMARY KEY,version INTEGER); INSERT INTO loongport_schema_version VALUES(1,24)").unwrap();
        database::vault::stamp(&conn, &super::super::VaultContext::generate().unwrap()).unwrap();
        drop(conn);
        let original = std::fs::read(&path).unwrap();
        assert!(inspect(dir.path(), &DeviceStore::at(dir.path().join("device"))).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[cfg(test)]
#[path = "upgrade_checkpoint_tests.rs"]
mod checkpoint_tests;

#[allow(dead_code)]
#[path = "upgrade_checkpoint.rs"]
pub(crate) mod checkpoint;
