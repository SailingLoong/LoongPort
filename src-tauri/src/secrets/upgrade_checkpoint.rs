//! Authenticated, device-local upgrade checkpoint and private staging only.
//! Publication remains with the startup and credential-generation owners.

use super::*;
use crate::config_file_io;
use crate::secrets::owned_file::DeviceFile;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::{Zeroize, Zeroizing};

pub(crate) use crate::secrets::owned_file::UPGRADE_CHECKPOINT_FILE as FILE;
pub(super) const MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Explicitly supported historical and current checkpoint identities. This is
/// only an admission bound; manifest/database checks still require exact equality.
pub(crate) fn supports_loongport_version(version: i32) -> bool {
    [24, 25, 26, 27].contains(&version)
}

fn legacy_archive_source_versions() -> SchemaVersions {
    SchemaVersions {
        upstream: database::UPSTREAM4_SOURCE_SCHEMA_VERSION,
        loongport: 24,
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedFile {
    path: PathBuf,
    revision: inspection::SourceRevision,
    bytes: Option<Vec<u8>>,
}
impl Drop for CapturedFile {
    fn drop(&mut self) {
        if let Some(bytes) = &mut self.bytes {
            bytes.zeroize();
        }
    }
}
impl CapturedFile {
    fn capture(path: &Path) -> Result<Self, AppError> {
        if !path.is_absolute() {
            return Err(invalid());
        }
        let revision = inspection::file_revision(path)?;
        let bytes = config_file_io::read_regular_file(path, MAX_BYTES)
            .map_err(|e| AppError::io(path, e))?;
        inspection::verify_unchanged(path, &revision)?;
        Ok(Self {
            path: path.to_owned(),
            revision,
            bytes,
        })
    }
    fn verify(&self) -> Result<(), AppError> {
        inspection::verify_unchanged(&self.path, &self.revision)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    id: String,
    root: PathBuf,
    device: PathBuf,
    metadata: VaultMetadata,
    source_versions: SchemaVersions,
    target_versions: SchemaVersions,
    database_revision: inspection::SourceRevision,
    database: String,
    database_digest: String,
    vault_file: CapturedFile,
    files: Vec<CapturedFile>,
    device_paths: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_database: Option<String>,
}
impl Drop for Manifest {
    fn drop(&mut self) {
        self.database.zeroize();
    }
}
fn invalid() -> AppError {
    AppError::Config("upgrade.invalid_checkpoint".into())
}
fn descriptor() -> Result<DeviceFile, AppError> {
    DeviceFile::registered(FILE)
}
fn path(device: &DeviceStore) -> PathBuf {
    device.root().join(FILE)
}
/// A passive candidate only; content and completed DB identity require authentication.
pub(crate) fn resume_candidate_bytes(device: &DeviceStore) -> Result<Option<Vec<u8>>, AppError> {
    if !super::super::files::device_directory_exists(device.root())? {
        return Ok(None);
    }
    config_file_io::read_regular_file(&path(device), MAX_BYTES)
        .map_err(|e| AppError::io(path(device), e))
}

fn device_paths(device: &DeviceStore) -> Result<Vec<PathBuf>, AppError> {
    Ok(super::super::files::device_file_paths(device.root())?
        .into_iter()
        .filter(|(file, _)| file.relative_path() != Path::new(FILE))
        .map(|(_, path)| path)
        .collect())
}

fn image(bytes: &[u8], root: &Path, device: &Path) -> Result<Connection, AppError> {
    let base = inspection::private_temp_base(root, device)?;
    let temporary = tempfile::Builder::new()
        .prefix("loongport-checkpoint-")
        .tempdir_in(base)
        .map_err(|_| invalid())?;
    config_file_io::ensure_private_directory(temporary.path())?;
    let file = temporary.path().join("checkpoint.db");
    let result = (|| {
        config_file_io::write_durable(&file, bytes)?;
        let copy = Connection::open_with_flags(&file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let integrity: String = copy.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(invalid());
        }
        let mut memory = Connection::open_in_memory()?;
        database::vault::copy(&copy, &mut memory)?;
        copy.close().map_err(|_| invalid())?;
        Ok(memory)
    })();
    temporary
        .close()
        .map_err(|_| AppError::Config("upgrade.temporary_cleanup_failed".into()))?;
    result
}

impl Manifest {
    fn validate_identity(
        &self,
        root: &Path,
        device: &DeviceStore,
        vault: &VaultContext,
        id: &str,
    ) -> Result<(), AppError> {
        if self.format != 1
            || self.id != id
            || uuid::Uuid::parse_str(id)
                .map(|id| id.to_string())
                .ok()
                .as_deref()
                != Some(id)
            || self.root != root
            || self.device != device.root()
            || self.metadata != *vault.metadata()
            || self.source_versions.upstream != database::UPSTREAM4_SOURCE_SCHEMA_VERSION
            || !supports_loongport_version(self.source_versions.loongport)
            || self.target_versions.upstream != database::UPSTREAM4_SCHEMA_VERSION
            || self.target_versions.loongport != self.source_versions.loongport
        {
            return Err(invalid());
        }
        if let Some(target) = &self.published_database {
            if target.len() != 64
                || !target
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    fn verify_source(
        &self,
        root: &Path,
        device: &DeviceStore,
        vault: &VaultContext,
        id: &str,
    ) -> Result<(), AppError> {
        if self.published_database.is_some() {
            return Err(invalid());
        }
        self.validate_identity(root, device, vault, id)?;
        if pending_generation(root)? {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
        crate::secrets::owned_file::ensure_no_pending_zcode_transaction(root)?;
        inspection::verify_unchanged(
            &root.join(crate::config::DB_FILE_NAME),
            &self.database_revision,
        )?;
        self.vault_file.verify()?;
        if device_paths(device)? != self.device_paths {
            return Err(changed());
        }
        for file in &self.files {
            file.verify()?;
        }
        Ok(())
    }
    fn database(&self, vault: &VaultContext) -> Result<Connection, AppError> {
        let bytes = Zeroizing::new(STANDARD.decode(&self.database).map_err(|_| invalid())?);
        let conn = image(&bytes, &self.root, &self.device)?;
        database::vault::check_identity(&conn, vault)?;
        crate::secrets::inventory::validate_database(&conn, vault)?;
        database::vault::preflight_connection(&conn)?;
        if Database::get_user_version(&conn)? != self.source_versions.upstream
            || database::loongport_schema::read_stored_version(&conn)?
                != self.source_versions.loongport
            || Database::content_digest(&conn)? != self.database_digest
        {
            return Err(invalid());
        }
        Ok(conn)
    }
}

/// Caller owns the sync mutex and startup admission. Client paths come from the
/// backend's affected-file plan, including absent files and catalog ownership.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Boundary {
    Authenticated,
    CaptureReady,
    Published,
}

pub(crate) fn create(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    clients: &[PathBuf],
) -> Result<String, AppError> {
    create_with_hook(root, device, vault, clients, &mut |_| Ok(()))
}

pub(super) fn create_with_hook(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    clients: &[PathBuf],
    hook: &mut dyn FnMut(Boundary) -> Result<(), AppError>,
) -> Result<String, AppError> {
    if config_file_io::read_regular_file(&path(device), MAX_BYTES)
        .map_err(|e| AppError::io(path(device), e))?
        .is_some()
    {
        return Err(AppError::Config("upgrade.checkpoint_pending".into()));
    }
    let inspected = inspect(root, device)?;
    inspected.validate_device_state(vault)?;
    inspected.validate_database(root, vault)?;
    crate::secrets::owned_file::ensure_no_pending_zcode_transaction(root)?;
    let UpgradeInspection::Stable(stable) = inspected else {
        return Err(invalid());
    };
    let source_versions = stable.source_versions.ok_or_else(invalid)?;
    if source_versions.upstream != database::UPSTREAM4_SOURCE_SCHEMA_VERSION
        || !supports_loongport_version(source_versions.loongport)
    {
        return Err(invalid());
    }
    hook(Boundary::Authenticated)?;
    stable.verify_unchanged(root)?;
    // This explicit backup action owns creating its private output directory.
    // Capture missing client revisions only after that expected directory exists,
    // otherwise our own publication changes their recorded ancestor inventory.
    config_file_io::ensure_private_directory(device.root())?;
    hook(Boundary::CaptureReady)?;
    let captured =
        inspection::capture(&root.join(crate::config::DB_FILE_NAME))?.ok_or_else(invalid)?;
    let paths = device_paths(device)?;
    let files = paths
        .iter()
        .chain(clients)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|p| CapturedFile::capture(p))
        .collect::<Result<Vec<_>, _>>()?;
    let id = uuid::Uuid::new_v4().to_string();
    let manifest = Manifest {
        format: 1,
        id: id.clone(),
        root: root.to_owned(),
        device: device.root().to_owned(),
        metadata: vault.metadata().clone(),
        source_versions,
        target_versions: SchemaVersions {
            upstream: database::UPSTREAM4_SCHEMA_VERSION,
            loongport: source_versions.loongport,
        },
        database_revision: captured.revision,
        database: STANDARD.encode(&*captured.image.serialize(rusqlite::MAIN_DB)?),
        database_digest: Database::content_digest(&captured.image)?,
        vault_file: CapturedFile::capture(&root.join("vault.json"))?,
        files,
        device_paths: paths,
        published_database: None,
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&manifest).map_err(|_| invalid())?);
    if plaintext.len() as u64 > MAX_BYTES / 2 {
        return Err(AppError::Config("upgrade.checkpoint_too_large".into()));
    }
    let ciphertext = descriptor()?.encode(vault, &plaintext)?;
    // Verify decryptability, full logical DB content and source before publishing.
    let decoded = descriptor()?.decode(vault, &ciphertext)?;
    let verified: Manifest = serde_json::from_slice(&decoded).map_err(|_| invalid())?;
    verified.database(vault)?;
    verified.verify_source(root, device, vault, &id)?;
    stable.verify_unchanged(root)?;
    config_file_io::ensure_private_directory(device.root())?;
    config_file_io::write_durable_new(&path(device), &ciphertext)?;
    hook(Boundary::Published)?;
    // Readback is part of checkpoint creation; failure retains the artifact.
    load(root, device, vault, &id)?.database(vault)?;
    Ok(id)
}

fn load(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    id: &str,
) -> Result<Manifest, AppError> {
    let bytes = config_file_io::read_regular_file(&path(device), MAX_BYTES)
        .map_err(|e| AppError::io(path(device), e))?
        .ok_or_else(invalid)?;
    let plaintext = descriptor()?.decode(vault, &bytes)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    manifest.verify_source(root, device, vault, id)?;
    Ok(manifest)
}

/// Explicit staging, never ordinary initialization. Nothing is published here.
pub(crate) fn stage(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    id: &str,
) -> Result<Connection, AppError> {
    Ok(stage_with_source_review(root, device, vault, id, |_| Ok(()))?.0)
}

/// Inspect the authenticated source image before schema setup can add defaults
/// or remove compatibility artifacts. Revalidate source after the callback.
pub(super) fn stage_with_source_review<T>(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    id: &str,
    review: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<(Connection, T), AppError> {
    let manifest = load(root, device, vault, id)?;
    let conn = manifest.database(vault)?;
    let source_facts = review(&conn)?;
    manifest.verify_source(root, device, vault, id)?;
    Database::create_tables_on_conn(&conn)?;
    Database::apply_upstream4_migrations_on_conn(&conn)?;
    database::vault::check_identity(&conn, vault)?;
    crate::secrets::inventory::validate_database(&conn, vault)?;
    if Database::get_user_version(&conn)? != manifest.target_versions.upstream
        || database::loongport_schema::read_stored_version(&conn)?
            != manifest.target_versions.loongport
    {
        return Err(invalid());
    }
    manifest.verify_source(root, device, vault, id)?;
    Ok((conn, source_facts))
}

/// Presence alone pauses both sync directions; corruption cannot lift admission.
pub(crate) fn ensure_sync_admitted(device: &DeviceStore) -> Result<(), AppError> {
    ensure_no_pending_checkpoint(device).map_err(|error| match error {
        AppError::Config(code) if code == "upgrade.checkpoint_pending" => {
            AppError::Config("upgrade.sync_paused".into())
        }
        other => other,
    })
}

/// New credential generations would invalidate the checkpoint's authenticated
/// source vault and nested database. Existing recovery keeps its original path.
pub(crate) fn ensure_no_pending_checkpoint(device: &DeviceStore) -> Result<(), AppError> {
    crate::secrets::files::device_directory_exists(device.root())?;
    match std::fs::symlink_metadata(path(device)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(AppError::Config("upgrade.checkpoint_pending".into())),
    }
}

/// Authenticated recovery after a caller loses the creation result.
pub(crate) fn existing_id(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
) -> Result<String, AppError> {
    Ok(existing_manifest(root, device, vault)?.0.id.clone())
}

/// Production readback also binds the caller's current backend-owned inventory.
/// Presence and authentication alone cannot prove a checkpoint captured its inputs.
pub(crate) fn existing_id_for_clients(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    clients: &[PathBuf],
) -> Result<String, AppError> {
    Ok(verified_checkpoint_for_clients(root, device, vault, clients)?.0)
}

pub(super) fn verified_checkpoint_for_clients(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    clients: &[PathBuf],
) -> Result<(String, Vec<u8>), AppError> {
    let (manifest, bytes) = existing_manifest(root, device, vault)?;
    let expected: std::collections::BTreeSet<_> = device_paths(device)?
        .into_iter()
        .chain(clients.iter().cloned())
        .collect();
    let captured: std::collections::BTreeSet<_> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect();
    if expected != captured {
        return Err(AppError::Config(
            "upgrade.checkpoint_inventory_changed".into(),
        ));
    }
    Ok((manifest.id.clone(), bytes))
}

fn existing_manifest(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
) -> Result<(Manifest, Vec<u8>), AppError> {
    let bytes = config_file_io::read_regular_file(&path(device), MAX_BYTES)
        .map_err(|e| AppError::io(path(device), e))?
        .ok_or_else(invalid)?;
    let plaintext = descriptor()?.decode(vault, &bytes)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    manifest.verify_source(root, device, vault, &manifest.id)?;
    manifest.database(vault)?;
    Ok((manifest, bytes))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CancellationBoundary {
    Verified,
    Removed,
}

/// Explicit pre-publication cancellation. The original authenticated review
/// retains the exact artifact across response loss; absence alone is no receipt.
pub(super) fn cancel_with_hook(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    clients: &[PathBuf],
    artifact: (&str, &[u8]),
    hook: &mut dyn FnMut(CancellationBoundary) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let (id, expected) = artifact;
    let plaintext = descriptor()?.decode(vault, expected)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    manifest.verify_source(root, device, vault, id)?;
    manifest.database(vault)?;
    let inventory: std::collections::BTreeSet<_> = device_paths(device)?
        .into_iter()
        .chain(clients.iter().cloned())
        .collect();
    if inventory != manifest.files.iter().map(|f| f.path.clone()).collect() {
        return Err(invalid());
    }
    let target = path(device);
    let revision = inspection::file_revision(&target)?;
    let current = config_file_io::read_regular_file(&target, MAX_BYTES)
        .map_err(|e| AppError::io(&target, e))?;
    if current.as_deref().is_some_and(|bytes| bytes != expected) {
        return Err(invalid());
    }
    hook(CancellationBoundary::Verified)?;
    manifest.verify_source(root, device, vault, id)?;
    inspection::verify_unchanged(&target, &revision)?;
    if current.is_some() {
        std::fs::remove_file(&target).map_err(|e| AppError::io(&target, e))?;
        hook(CancellationBoundary::Removed)?;
    }
    crate::live::engine::sync_parent(&target)?;
    if config_file_io::read_regular_file(&target, MAX_BYTES)
        .map_err(|e| AppError::io(&target, e))?
        .is_some()
    {
        return Err(invalid());
    }
    manifest.verify_source(root, device, vault, id)
}

/// Prepare the target handoff inside the original generation transaction. This
/// private seam does not publish runtime settings, establish mode or enable apps.
pub(super) fn publish_database_with_hook(
    db: &Database,
    device: &DeviceStore,
    store: &dyn crate::secrets::key_store::KeyStore,
    id: &str,
    hook: &mut dyn FnMut(crate::secrets::transition::Checkpoint) -> Result<(), AppError>,
    original_intent: Option<&mut Option<Vec<u8>>>,
) -> Result<(), AppError> {
    let root = db.secret_session().root();
    crate::secrets::transition::install_upgrade_database(
        db,
        store,
        device.root(),
        |source, current, _| {
            let manifest = load(root, device, current, id)?;
            if Database::content_digest(source)? != manifest.database_digest {
                return Err(changed());
            }
            stage(root, device, current, id)
        },
        &mut |target, current| {
            let (mut manifest, source_bytes) = existing_manifest(root, device, current)?;
            if manifest.id != id {
                return Err(changed());
            }
            if Database::get_user_version(target)? != manifest.target_versions.upstream
                || database::loongport_schema::read_stored_version(target)?
                    != manifest.target_versions.loongport
            {
                return Err(invalid());
            }
            manifest.published_database = Some(Database::content_digest(target)?);
            let plaintext = Zeroizing::new(serde_json::to_vec(&manifest).map_err(|_| invalid())?);
            Ok(crate::secrets::transition::UpgradeCheckpointReplacement {
                source_digest: crate::live::engine::digest(Some(&source_bytes))
                    .ok_or_else(invalid)?,
                ciphertext: descriptor()?.encode(current, &plaintext)?,
            })
        },
        hook,
        original_intent,
    )
}

/// Authenticate original checkpoint provenance and unchanged non-DB inputs on
/// either side of DB publication. The transition separately pins source/target DB.
pub(crate) fn validate_publication_payload(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    bytes: &[u8],
) -> Result<(String, String, SchemaVersions), AppError> {
    let plaintext = descriptor()?.decode(vault, bytes)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    let target = manifest.published_database.as_ref().ok_or_else(invalid)?;
    manifest.validate_identity(root, device, vault, &manifest.id)?;
    manifest.database(vault)?;
    manifest.vault_file.verify()?;
    if device_paths(device)? != manifest.device_paths {
        return Err(changed());
    }
    for file in &manifest.files {
        file.verify()?;
    }
    Ok((
        manifest.database_digest.clone(),
        target.clone(),
        manifest.target_versions,
    ))
}

/// DB-complete/app-not-started recognition only. This is not global completion,
/// and cannot authorize writes after apps have begun changing reviewed inputs.
pub(super) fn published_database_id(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
) -> Result<Option<String>, AppError> {
    if pending_generation(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let Some(bytes) = config_file_io::read_regular_file(&path(device), MAX_BYTES)
        .map_err(|e| AppError::io(path(device), e))?
    else {
        return Ok(None);
    };
    let plaintext = descriptor()?.decode(vault, &bytes)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    if manifest.published_database.is_none() {
        manifest.verify_source(root, device, vault, &manifest.id)?;
        manifest.database(vault)?;
        return Ok(None);
    }
    let (_, target, _) = validate_publication_payload(root, device, vault, &bytes)?;
    verify_current_database(root, vault, manifest.target_versions, Some(&target))?;
    Ok(Some(manifest.id.clone()))
}

fn verify_current_database(
    root: &Path,
    vault: &VaultContext,
    versions: SchemaVersions,
    digest: Option<&str>,
) -> Result<(), AppError> {
    let path = root.join(crate::config::DB_FILE_NAME);
    let captured = inspection::capture(&path)?.ok_or_else(invalid)?;
    database::vault::check_identity(&captured.image, vault)?;
    crate::secrets::inventory::validate_database(&captured.image, vault)?;
    database::vault::preflight_connection(&captured.image)?;
    if Database::get_user_version(&captured.image)? != versions.upstream
        || database::loongport_schema::read_stored_version(&captured.image)? != versions.loongport
    {
        return Err(changed());
    }
    if let Some(expected) = digest {
        if Database::content_digest(&captured.image)? != expected {
            return Err(changed());
        }
    }
    inspection::verify_unchanged(&path, &captured.revision)
}

/// The authenticated target checkpoint is installed after the DB. The original
/// transition removes its intent only after final on-disk target readback. That
/// completed boundary survives later per-app changes; no second ack is needed.
/// Per-app admission still needs its own reviewed inputs and operation readback.
pub(crate) fn verified_database_id(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
) -> Result<Option<String>, AppError> {
    if pending_generation(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let revision = inspection::file_revision(&path(device))?;
    let Some(bytes) = config_file_io::read_regular_file(&path(device), MAX_BYTES)
        .map_err(|e| AppError::io(path(device), e))?
    else {
        inspection::verify_unchanged(&path(device), &revision)?;
        return Ok(None);
    };
    let plain = descriptor()?.decode(vault, &bytes)?;
    let manifest: Manifest = serde_json::from_slice(&plain).map_err(|_| invalid())?;
    manifest.validate_identity(root, device, vault, &manifest.id)?;
    manifest.database(vault)?;
    if manifest.published_database.is_none() {
        manifest.verify_source(root, device, vault, &manifest.id)?;
        inspection::verify_unchanged(&path(device), &revision)?;
        return Ok(None);
    }
    manifest.vault_file.verify()?;
    verify_current_database(root, vault, manifest.target_versions, None)?;
    inspection::verify_unchanged(&path(device), &revision)?;
    if pending_generation(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    Ok(Some(manifest.id.clone()))
}

/// Completed recovery material uses the existing device-file/key lifecycle.
/// Its source DB uses the existing encrypted database backup lifecycle. Neither
/// container retains the old key or nests an old encrypted checkpoint.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionArchive {
    format: u32,
    id: String,
    root: PathBuf,
    device: PathBuf,
    database_backup: String,
    database_digest: String,
    // Original format-1 archives predate this field and exclusively used 17/24.
    // New archives carry the authenticated source tuple, never the current constant.
    #[serde(default = "legacy_archive_source_versions")]
    source_versions: SchemaVersions,
    files: Vec<ArchivedFile>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArchivedKind {
    Native,
    Device,
    Settings,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchivedFile {
    path: PathBuf,
    kind: ArchivedKind,
    bytes: Option<Vec<u8>>,
}
impl Drop for ArchivedFile {
    fn drop(&mut self) {
        if let Some(bytes) = &mut self.bytes {
            bytes.zeroize();
        }
    }
}
fn archive_descriptor() -> Result<DeviceFile, AppError> {
    DeviceFile::registered(crate::secrets::owned_file::UPGRADE_ARCHIVE_FILE)
}
fn archive_path(device: &DeviceStore) -> PathBuf {
    device
        .root()
        .join(crate::secrets::owned_file::UPGRADE_ARCHIVE_FILE)
}
fn read_archive(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    id: &str,
) -> Result<CompletionArchive, AppError> {
    if pending_generation(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let path = archive_path(device);
    let revision = inspection::file_revision(&path)?;
    let bytes = config_file_io::read_regular_file(&path, MAX_BYTES)
        .map_err(|e| AppError::io(&path, e))?
        .ok_or_else(invalid)?;
    let plain = archive_descriptor()?.decode(vault, &bytes)?;
    let archive: CompletionArchive = serde_json::from_slice(&plain).map_err(|_| invalid())?;
    if archive.format != 1
        || archive.root != root
        || archive.device != device.root()
        || archive.id != id
        || uuid::Uuid::parse_str(id)
            .map(|v| v.to_string())
            .ok()
            .as_deref()
            != Some(id)
        || archive.database_backup != format!("upgrade_{id}.db")
        || archive.source_versions.upstream != database::UPSTREAM4_SOURCE_SCHEMA_VERSION
        || !supports_loongport_version(archive.source_versions.loongport)
    {
        return Err(invalid());
    }
    let mut paths = std::collections::BTreeSet::new();
    for file in &archive.files {
        if !file.path.is_absolute()
            || !paths.insert(&file.path)
            || file.path == archive_path(device)
            || file.path == self::path(device)
        {
            return Err(invalid());
        }
        match file.kind {
            ArchivedKind::Device => {
                DeviceFile::registered(
                    file.path
                        .strip_prefix(device.root())
                        .map_err(|_| invalid())?,
                )?;
            }
            ArchivedKind::Settings if file.path != crate::settings::settings_path() => {
                return Err(invalid())
            }
            _ => {}
        }
    }
    inspection::verify_unchanged(&path, &revision)?;
    Ok(archive)
}

/// Authenticates the retained source DB under the current key, including after
/// the original rotation owner has rewrapped its metadata and protected fields.
pub(crate) fn archived_database(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    id: &str,
) -> Result<Connection, AppError> {
    let archive = read_archive(root, device, vault, id)?;
    let path = root.join("backups").join(&archive.database_backup);
    let captured = inspection::capture(&path)?.ok_or_else(invalid)?;
    // Password rewrap changes the current wrapper, not the archive's data key.
    // Authenticate the backup's own wrapper with that same current key.
    let metadata = database::vault::stored_metadata(&captured.image)?.ok_or_else(invalid)?;
    Database::validate_sync_source(vault, &metadata)?;
    let source = VaultContext::from_key(metadata, vault.export_key())
        .map_err(crate::secrets::inventory::secret_error)?;
    database::vault::check_identity(&captured.image, &source)?;
    crate::secrets::inventory::validate_database(&captured.image, &source)?;
    database::vault::preflight_connection(&captured.image)?;
    if Database::get_user_version(&captured.image)? != archive.source_versions.upstream
        || database::loongport_schema::read_stored_version(&captured.image)?
            != archive.source_versions.loongport
    {
        return Err(invalid());
    }
    if crate::secrets::inventory::upgrade_archive_content_digest(&captured.image, &source)?
        != archive.database_digest
    {
        return Err(changed());
    }
    inspection::verify_unchanged(&path, &captured.revision)?;
    Ok(captured.image)
}

type RestoredArchiveFiles = Vec<(PathBuf, Option<Zeroizing<Vec<u8>>>)>;

/// Reconstruct source files for selective recovery using their original owners.
/// This is a read, not an automatic rollback of current configuration.
pub(crate) fn restored_archive_files(
    root: &Path,
    device: &DeviceStore,
    vault: &VaultContext,
    id: &str,
) -> Result<RestoredArchiveFiles, AppError> {
    let archive = read_archive(root, device, vault, id)?;
    archived_database(root, device, vault, id)?;
    archive
        .files
        .iter()
        .map(|file| {
            let bytes = file
                .bytes
                .as_ref()
                .map(|bytes| match file.kind {
                    ArchivedKind::Native => Ok(Zeroizing::new(bytes.clone())),
                    ArchivedKind::Device => DeviceFile::registered(
                        file.path
                            .strip_prefix(device.root())
                            .map_err(|_| invalid())?,
                    )?
                    .encode(vault, bytes)
                    .map(Zeroizing::new),
                    ArchivedKind::Settings => {
                        crate::settings::restore_settings_archive_with_vault(bytes, vault)
                            .map(Zeroizing::new)
                    }
                })
                .transpose()?;
            Ok((file.path.clone(), bytes))
        })
        .collect()
}

/// Query an existing completed result without repeating archive publication.
/// A pause record can precede active-checkpoint release after an interruption.
pub(crate) fn read_completed_result(
    db: &Database,
    device: &DeviceStore,
    vault: &std::sync::RwLockReadGuard<'_, VaultContext>,
) -> Result<Option<crate::settings::UpgradeCompletion>, AppError> {
    let active = path(device);
    let revision = inspection::file_revision(&active)?;
    if config_file_io::read_regular_file(&active, MAX_BYTES)
        .map_err(|e| AppError::io(&active, e))?
        .is_some()
    {
        return Ok(None);
    }
    let record = crate::settings::read_upgrade_completion_with_vault(db.secret_session(), vault)?;
    if let Some(record) = &record {
        restored_archive_files(
            db.secret_session().root(),
            device,
            vault,
            &record.checkpoint_id,
        )?;
    }
    inspection::verify_unchanged(&active, &revision)?;
    Ok(record)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionBoundary {
    DatabaseArchived,
    ArchiveVerified,
    PausePersisted,
    BeforeRelease,
}

/// Caller holds the existing sync/app/lifecycle owners and freshly proves all
/// relevant applications. Hooks keep the original fault-injection convention.
pub(crate) fn archive_and_release(
    db: &Database,
    device: &DeviceStore,
    vault: &std::sync::RwLockReadGuard<'_, VaultContext>,
    id: &str,
    hook: &mut dyn FnMut(CompletionBoundary) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let root = db.secret_session().root();
    let active_path = path(device);
    let revision = inspection::file_revision(&active_path)?;
    let active = config_file_io::read_regular_file(&active_path, MAX_BYTES)
        .map_err(|e| AppError::io(&active_path, e))?;
    let Some(active) = active else {
        let record = read_completed_result(db, device, vault)?.ok_or_else(changed)?;
        if record.checkpoint_id != id {
            return Err(changed());
        }
        inspection::verify_unchanged(&active_path, &revision)?;
        return db.migrate_released_checkpoint_schema(device, vault);
    };
    if verified_database_id(root, device, vault)?.as_deref() != Some(id) {
        return Err(changed());
    }
    let plain = descriptor()?.decode(vault, &active)?;
    let manifest: Manifest = serde_json::from_slice(&plain).map_err(|_| invalid())?;
    let source = manifest.database(vault)?;
    let backup = db.archive_upgrade_database(&source, id, vault)?;
    hook(CompletionBoundary::DatabaseArchived)?;
    let archived = archive_path(device);
    {
        let files = manifest
            .files
            .iter()
            .map(|file| {
                let kind = if file.path == crate::settings::settings_path() {
                    ArchivedKind::Settings
                } else if manifest.device_paths.contains(&file.path) {
                    ArchivedKind::Device
                } else {
                    ArchivedKind::Native
                };
                let bytes = file
                    .bytes
                    .as_ref()
                    .map(|bytes| match kind {
                        ArchivedKind::Settings => {
                            crate::settings::export_settings_archive_with_vault(bytes, vault)
                                .map(|v| v.to_vec())
                        }
                        ArchivedKind::Device => DeviceFile::registered(
                            file.path
                                .strip_prefix(device.root())
                                .map_err(|_| invalid())?,
                        )?
                        .decode(vault, bytes)
                        .map(|v| v.to_vec()),
                        ArchivedKind::Native => Ok(bytes.clone()),
                    })
                    .transpose()?;
                Ok(ArchivedFile {
                    path: file.path.clone(),
                    kind,
                    bytes,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let archive = CompletionArchive {
            format: 1,
            id: id.into(),
            root: root.into(),
            device: device.root().into(),
            database_backup: backup
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(invalid)?
                .into(),
            database_digest: crate::secrets::inventory::upgrade_archive_content_digest(
                &source, vault,
            )?,
            source_versions: manifest.source_versions,
            files,
        };
        let plain = Zeroizing::new(serde_json::to_vec(&archive).map_err(|_| invalid())?);
        if plain.len() as u64 > MAX_BYTES / 2 {
            return Err(AppError::Config("upgrade.checkpoint_too_large".into()));
        }
        let bytes = archive_descriptor()?.encode(vault, &plain)?;
        match config_file_io::read_regular_file(&archived, MAX_BYTES)
            .map_err(|e| AppError::io(&archived, e))?
        {
            Some(existing) => {
                let decoded = archive_descriptor()?.decode(vault, &existing)?;
                let recorded: CompletionArchive =
                    serde_json::from_slice(&decoded).map_err(|_| invalid())?;
                // Normalize only the historical absent version field in memory.
                // The authenticated archive itself remains byte-for-byte untouched.
                if serde_json::to_vec(&recorded).map_err(|_| invalid())? != *plain {
                    return Err(changed());
                }
            }
            None => config_file_io::write_durable_new(&archived, &bytes)?,
        }
    }
    restored_archive_files(root, device, vault, id)?;
    hook(CompletionBoundary::ArchiveVerified)?;
    crate::settings::set_upgrade_completion_with_vault(id, db.secret_session(), vault)?;
    hook(CompletionBoundary::PausePersisted)?;
    hook(CompletionBoundary::BeforeRelease)?;
    crate::settings::set_upgrade_completion_with_vault(id, db.secret_session(), vault)?;
    if verified_database_id(root, device, vault)?.as_deref() != Some(id) {
        return Err(changed());
    }
    inspection::verify_unchanged(&active_path, &revision)?;
    std::fs::remove_file(&active_path).map_err(|e| AppError::io(&active_path, e))?;
    crate::live::engine::sync_parent(&active_path)?;
    if resume_candidate_bytes(device)?.is_some() {
        return Err(changed());
    }
    restored_archive_files(root, device, vault, id)?;
    db.migrate_released_checkpoint_schema(device, vault)
}

#[cfg(test)]
mod paperclip_schema_lifecycle_tests {
    use super::*;

    fn historical_source() -> super::super::checkpoint_tests::Fixture {
        let fixture = super::super::checkpoint_tests::Fixture::new();
        let conn = Connection::open(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
        conn.execute_batch("ALTER TABLE providers DROP COLUMN qoder_http_secret; DROP TABLE IF EXISTS qoder_campaign_operations; DROP TABLE IF EXISTS paperclip_run_bindings; UPDATE loongport_schema_version SET version=24 WHERE id=1;").unwrap();
        fixture
    }

    fn write_archive(
        fixture: &super::super::checkpoint_tests::Fixture,
        version: i32,
        recorded_version: Option<i32>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let path = fixture.root.join(crate::config::DB_FILE_NAME);
        let conn = Connection::open(&path).unwrap();
        if version == 25 {
            conn.execute_batch("CREATE TABLE paperclip_run_bindings(owner TEXT NOT NULL,app TEXT NOT NULL,company TEXT NOT NULL,run TEXT NOT NULL,scope TEXT NOT NULL,binding TEXT NOT NULL,phase TEXT NOT NULL CHECK(phase IN ('bound','accepted','completed','failed','unknown')),receipt TEXT,PRIMARY KEY(owner,app,company,run)); UPDATE loongport_schema_version SET version=25 WHERE id=1;").unwrap();
        }
        let digest =
            crate::secrets::inventory::upgrade_archive_content_digest(&conn, &fixture.vault)
                .unwrap();
        drop(conn);
        let backups = fixture.root.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::copy(&path, backups.join(format!("upgrade_{id}.db"))).unwrap();
        let mut archive = serde_json::json!({"format":1,"id":id,"root":fixture.root,"device":fixture.device.root(),"database_backup":format!("upgrade_{id}.db"),"database_digest":digest,"files":[]});
        if let Some(version) = recorded_version {
            archive["source_versions"] = serde_json::json!({"upstream":17,"loongport":version});
        }
        let ciphertext = archive_descriptor()
            .unwrap()
            .encode(&fixture.vault, &serde_json::to_vec(&archive).unwrap())
            .unwrap();
        crate::config_file_io::ensure_private_directory(fixture.device.root()).unwrap();
        crate::config_file_io::write_durable(&archive_path(&fixture.device), &ciphertext).unwrap();
        id
    }

    #[test]
    fn paperclip_native_v25_checkpoint_preserves_recorded_identity() {
        let f = historical_source();
        let source = Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        // Fixed historical v25, not whatever current apply() produces.
        crate::paperclip::bindings::ensure_schema(&source).unwrap();
        source
            .execute(
                "UPDATE loongport_schema_version SET version=25 WHERE id=1",
                [],
            )
            .unwrap();
        drop(source);
        let id = create(&f.root, &f.device, &f.vault, &[]).unwrap();
        let staged = stage(&f.root, &f.device, &f.vault, &id).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&staged).unwrap(),
            25
        );
        let bytes = std::fs::read(path(&f.device)).unwrap();
        let decoded = descriptor().unwrap().decode(&f.vault, &bytes).unwrap();
        let mut manifest: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        manifest["target_versions"]["loongport"] = serde_json::json!(24);
        let ciphertext = descriptor()
            .unwrap()
            .encode(&f.vault, &serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        crate::config_file_io::write_durable(&path(&f.device), &ciphertext).unwrap();
        assert!(stage(&f.root, &f.device, &f.vault, &id).is_err());
    }

    #[test]
    fn qoder_historical_v26_checkpoint_remains_exact_without_http_column() {
        let f = historical_source();
        let source = Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        crate::paperclip::bindings::ensure_schema(&source).unwrap();
        crate::database::create_qoder_campaign_operations_schema(&source).unwrap();
        source
            .execute_batch("UPDATE loongport_schema_version SET version=26 WHERE id=1")
            .unwrap();
        let before = Database::content_digest(&source).unwrap();
        let id = create(&f.root, &f.device, &f.vault, &[]).unwrap();
        let staged = stage(&f.root, &f.device, &f.vault, &id).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&staged).unwrap(),
            26
        );
        assert!(!crate::database::has_qoder_http_auth_column(&staged).unwrap());
        assert_eq!(Database::content_digest(&source).unwrap(), before);
    }

    #[test]
    fn qoder_http_current_v27_checkpoint_keeps_recorded_identity() {
        let f = historical_source();
        let source = Connection::open(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        database::loongport_schema::apply(&source).unwrap();
        drop(source);
        let id = create(&f.root, &f.device, &f.vault, &[]).unwrap();
        let staged = stage(&f.root, &f.device, &f.vault, &id).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&staged).unwrap(),
            27
        );
        // The existing read-only owner validates the receipt schema and rows.
        database::vault::preflight_connection(&staged).unwrap();
        assert!(supports_loongport_version(26));
        assert!(!supports_loongport_version(28));
    }

    #[test]
    fn paperclip_native_v24_checkpoint_keeps_original_schema_and_digest() {
        let f = historical_source();
        let before = std::fs::read(f.root.join(crate::config::DB_FILE_NAME)).unwrap();
        let id = create(&f.root, &f.device, &f.vault, &[]).unwrap();
        let checkpoint = std::fs::read(path(&f.device)).unwrap();
        let staged = stage(&f.root, &f.device, &f.vault, &id).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&staged).unwrap(),
            24
        );
        assert!(!Database::table_exists(&staged, "paperclip_run_bindings").unwrap());
        assert_eq!(
            std::fs::read(f.root.join(crate::config::DB_FILE_NAME)).unwrap(),
            before
        );
        assert_eq!(std::fs::read(path(&f.device)).unwrap(), checkpoint);
        assert!(ensure_sync_admitted(&f.device).is_err());
    }

    #[test]
    fn paperclip_native_v24_completed_archive_migrates_only_authenticated_working_copy() {
        let f = historical_source();
        let id = write_archive(&f, 24, None);
        let backup = f.root.join("backups").join(format!("upgrade_{id}.db"));
        let before = std::fs::read(&backup).unwrap();
        let original = archived_database(&f.root, &f.device, &f.vault, &id).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&original).unwrap(),
            24
        );
        let mut working = Connection::open_in_memory().unwrap();
        database::vault::copy(&original, &mut working).unwrap();
        database::vault::upgrade_staging_for_target(
            &working,
            &f.vault,
            database::UPSTREAM4_SOURCE_SCHEMA_VERSION,
        )
        .unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&working).unwrap(),
            database::loongport_schema::LOONGPORT_SCHEMA_VERSION
        );
        assert_eq!(
            database::loongport_schema::read_stored_version(&original).unwrap(),
            24
        );
        assert_eq!(std::fs::read(&backup).unwrap(), before);
        archived_database(&f.root, &f.device, &f.vault, &id).unwrap();
    }

    #[test]
    fn paperclip_native_completed_archive_requires_exact_recorded_version() {
        let f = historical_source();
        let id = write_archive(&f, 25, Some(25));
        let original = archived_database(&f.root, &f.device, &f.vault, &id).unwrap();
        assert_eq!(
            database::loongport_schema::read_stored_version(&original).unwrap(),
            25
        );
        let bytes = std::fs::read(archive_path(&f.device)).unwrap();
        let decoded = archive_descriptor()
            .unwrap()
            .decode(&f.vault, &bytes)
            .unwrap();
        let mut archive: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        archive["source_versions"]["loongport"] = serde_json::json!(24);
        let ciphertext = archive_descriptor()
            .unwrap()
            .encode(&f.vault, &serde_json::to_vec(&archive).unwrap())
            .unwrap();
        crate::config_file_io::write_durable(&archive_path(&f.device), &ciphertext).unwrap();
        assert!(archived_database(&f.root, &f.device, &f.vault, &id).is_err());
    }
}
