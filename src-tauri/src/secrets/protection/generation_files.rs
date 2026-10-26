//! Private file preparation only; generation publication and cleanup stay with their owner.
use crate::{
    config_file_io,
    error::AppError,
    secrets::{
        files::{self, OwnedFile},
        owned_file::DeviceFile,
        storage::StorageContext,
    },
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GenerationFileRole {
    Owned,
    Device,
    Settings,
    Neutral,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GenerationSourceRoot {
    Data,
    Device,
}

/// Read-only provenance, not a cleanup permit or a verified generation constructor.
pub(crate) struct GenerationFileEvidence {
    role: GenerationFileRole,
    source_root: GenerationSourceRoot,
    relative: PathBuf,
    #[cfg(any(test, feature = "test-hooks"))]
    source: PathBuf,
    #[cfg(any(test, feature = "test-hooks"))]
    target: PathBuf,
    source_hash: [u8; 32],
    #[cfg(any(test, feature = "test-hooks"))]
    target_hash: [u8; 32],
    #[cfg(any(test, feature = "test-hooks"))]
    source_owner_execute: bool,
    #[cfg(any(test, feature = "test-hooks"))]
    target_owner_execute: bool,
}
impl GenerationFileEvidence {
    pub(crate) fn role(&self) -> GenerationFileRole {
        self.role
    }
    pub(crate) fn source_root(&self) -> GenerationSourceRoot {
        self.source_root
    }
    pub(crate) fn relative_path(&self) -> &Path {
        &self.relative
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn source_path(&self) -> &Path {
        &self.source
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn target_path(&self) -> &Path {
        &self.target
    }
    pub(crate) fn source_hash(&self) -> &[u8; 32] {
        &self.source_hash
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn target_hash(&self) -> &[u8; 32] {
        &self.target_hash
    }
    /// Captured Neutral behavior only; non-Neutral and non-Unix are always false.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn source_owner_execute(&self) -> bool {
        self.source_owner_execute
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn target_owner_execute(&self) -> bool {
        self.target_owner_execute
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Codec {
    Owned(OwnedFile),
    Device(DeviceFile),
    Settings,
    Neutral,
}
impl Codec {
    fn role(&self) -> GenerationFileRole {
        match self {
            Self::Owned(_) => GenerationFileRole::Owned,
            Self::Device(_) => GenerationFileRole::Device,
            Self::Settings => GenerationFileRole::Settings,
            Self::Neutral => GenerationFileRole::Neutral,
        }
    }
    fn transcode(
        &self,
        bytes: &[u8],
        source: &StorageContext,
        target: &StorageContext,
    ) -> Result<Zeroizing<Vec<u8>>, AppError> {
        let encoded = match self {
            Self::Owned(file) => {
                file.encode_storage(target, &file.decode_storage(source, bytes)?)?
            }
            Self::Device(file) => {
                file.encode_storage(target, &file.decode_storage(source, bytes)?)?
            }
            Self::Settings => {
                crate::settings::transcode_settings_with_storage(bytes, source, target)?
            }
            Self::Neutral => bytes.to_vec(),
        };
        Ok(Zeroizing::new(encoded))
    }
}
struct SourceFile {
    root: GenerationSourceRoot,
    relative: PathBuf,
    path: PathBuf,
    codec: Codec,
    bytes: Zeroizing<Vec<u8>>,
    owner_execute: bool,
}
struct SourceInventory {
    files: BTreeMap<(GenerationSourceRoot, PathBuf), SourceFile>,
    directories: BTreeSet<PathBuf>,
    data_exists: bool,
    device_exists: bool,
}
struct TargetFile {
    codec: Codec,
    bytes: Zeroizing<Vec<u8>>,
    owner_execute: bool,
}

/// A file-owner preparation, not permission to publish a selector or erase a source.
/// The caller retains the original session/DB lifecycle guards. Excluded DB,
/// metadata, scope-marker and diagnostics need their own owners' proofs.
pub(crate) struct PreparedGenerationFiles {
    data_root: PathBuf,
    device_root: PathBuf,
    target_root: PathBuf,
    source: StorageContext,
    target: StorageContext,
    source_image: SourceInventory,
    target_files: BTreeMap<PathBuf, TargetFile>,
    evidence: Vec<GenerationFileEvidence>,
}
impl PreparedGenerationFiles {
    pub(crate) fn stage(
        data_root: &Path,
        device_root: &Path,
        target_root: &Path,
        source: &StorageContext,
        target: &StorageContext,
    ) -> Result<Self, AppError> {
        validate_roots(data_root, device_root, target_root)?;
        validate_generation(source, target)?;
        let source_image = capture(data_root, device_root, source, target)?;
        let mut target_files: BTreeMap<PathBuf, TargetFile> = BTreeMap::new();
        let mut first_sources: BTreeMap<PathBuf, &SourceFile> = BTreeMap::new();
        let mut evidence = Vec::new();
        for file in source_image.files.values() {
            if let Some(first) = first_sources.get(&file.relative) {
                if first.codec != file.codec
                    || first.bytes != file.bytes
                    || first.owner_execute != file.owner_execute
                {
                    return Err(invalid("protection.generation_file_conflict"));
                }
            } else {
                let bytes = file.codec.transcode(&file.bytes, source, target)?;
                target_files.insert(
                    file.relative.clone(),
                    TargetFile {
                        codec: file.codec.clone(),
                        bytes,
                        owner_execute: file.owner_execute,
                    },
                );
                first_sources.insert(file.relative.clone(), file);
            }
            #[cfg(any(test, feature = "test-hooks"))]
            let encoded = &target_files[&file.relative].bytes;
            evidence.push(GenerationFileEvidence {
                role: file.codec.role(),
                source_root: file.root,
                relative: file.relative.clone(),
                #[cfg(any(test, feature = "test-hooks"))]
                source: file.path.clone(),
                #[cfg(any(test, feature = "test-hooks"))]
                target: target_root.join(&file.relative),
                source_hash: Sha256::digest(&*file.bytes).into(),
                #[cfg(any(test, feature = "test-hooks"))]
                target_hash: Sha256::digest(&**encoded).into(),
                #[cfg(any(test, feature = "test-hooks"))]
                source_owner_execute: file.owner_execute,
                #[cfg(any(test, feature = "test-hooks"))]
                target_owner_execute: target_files[&file.relative].owner_execute,
            });
        }
        drop(first_sources);
        let prepared = Self {
            data_root: data_root.into(),
            device_root: device_root.into(),
            target_root: target_root.into(),
            source: source.clone(),
            target: target.clone(),
            source_image,
            target_files,
            evidence,
        };
        // This preflight is stricter than verify_target: existing DB/metadata is
        // not evidence that it belongs to this new private staging operation.
        prepared.verify_empty_target()?;
        prepared.verify_source()?;
        crate::secrets::session::ensure_owned_directory(target_root, target_root)?;
        for relative in &prepared.source_image.directories {
            crate::secrets::session::ensure_owned_directory(
                target_root,
                &target_root.join(relative),
            )?;
        }
        for (relative, file) in &prepared.target_files {
            let path = target_root.join(relative);
            let parent = path
                .parent()
                .ok_or_else(|| invalid("secret.invalid_storage_path"))?;
            config_file_io::directory_exists_no_alias(parent)?;
            crate::secrets::session::ensure_owned_directory(target_root, parent)?;
            // Original private durable no-clobber writer, never copy-then-overwrite.
            config_file_io::write_durable_new(&path, &file.bytes)?;
            #[cfg(unix)]
            if file.owner_execute {
                preserve_owner_execute(&path, &file.bytes)?;
            }
        }
        prepared.verify_source()?;
        prepared.verify_target()?;
        Ok(prepared)
    }
    /// Read-only admission for an already authenticated private pending generation.
    /// The outer owner still verifies its signed intent and exact target digests.
    pub(crate) fn inspect_existing(
        data_root: &Path,
        device_root: &Path,
        target_root: &Path,
        source: &StorageContext,
        target: &StorageContext,
    ) -> Result<Self, AppError> {
        validate_roots(data_root, device_root, target_root)?;
        validate_generation(source, target)?;
        let source_image = capture(data_root, device_root, source, target)?;
        let target_image = capture(target_root, target_root, target, source)?;
        if !target_image.data_exists {
            return Err(invalid("protection.target_changed"));
        }
        let mut actual_targets = BTreeMap::new();
        for file in target_image.files.values() {
            if actual_targets.insert(file.relative.clone(), file).is_some() {
                return Err(invalid("protection.generation_file_conflict"));
            }
        }
        let mut target_files = BTreeMap::new();
        let mut first_sources: BTreeMap<PathBuf, &SourceFile> = BTreeMap::new();
        let mut evidence = Vec::new();
        for file in source_image.files.values() {
            let actual = actual_targets
                .get(&file.relative)
                .ok_or_else(|| invalid("protection.target_changed"))?;
            if let Some(first) = first_sources.get(&file.relative) {
                if first.codec != file.codec
                    || first.bytes != file.bytes
                    || first.owner_execute != file.owner_execute
                {
                    return Err(invalid("protection.generation_file_conflict"));
                }
            } else {
                if file.codec != actual.codec
                    || file.owner_execute != actual.owner_execute
                    || !same_content(&file.codec, &file.bytes, &actual.bytes, source, target)?
                {
                    return Err(invalid("protection.target_changed"));
                }
                // Retain the existing authenticated encoding, never a new random
                // nonce. The outer signed intent separately pins these exact bytes.
                target_files.insert(
                    file.relative.clone(),
                    TargetFile {
                        codec: file.codec.clone(),
                        bytes: Zeroizing::new(actual.bytes.to_vec()),
                        owner_execute: file.owner_execute,
                    },
                );
                first_sources.insert(file.relative.clone(), file);
            }
            evidence.push(GenerationFileEvidence {
                role: file.codec.role(),
                source_root: file.root,
                relative: file.relative.clone(),
                #[cfg(any(test, feature = "test-hooks"))]
                source: file.path.clone(),
                #[cfg(any(test, feature = "test-hooks"))]
                target: target_root.join(&file.relative),
                source_hash: Sha256::digest(&*file.bytes).into(),
                #[cfg(any(test, feature = "test-hooks"))]
                target_hash: Sha256::digest(&*actual.bytes).into(),
                #[cfg(any(test, feature = "test-hooks"))]
                source_owner_execute: file.owner_execute,
                #[cfg(any(test, feature = "test-hooks"))]
                target_owner_execute: actual.owner_execute,
            });
        }
        if target_files.len() != actual_targets.len() {
            return Err(invalid("protection.target_changed"));
        }
        drop(first_sources);
        let prepared = Self {
            data_root: data_root.into(),
            device_root: device_root.into(),
            target_root: target_root.into(),
            source: source.clone(),
            target: target.clone(),
            source_image,
            target_files,
            evidence,
        };
        prepared.verify_source()?;
        prepared.verify_target()?;
        Ok(prepared)
    }
    pub(crate) fn verify_source(&self) -> Result<(), AppError> {
        validate_roots(&self.data_root, &self.device_root, &self.target_root)?;
        let actual = capture(
            &self.data_root,
            &self.device_root,
            &self.source,
            &self.target,
        )?;
        if actual.data_exists != self.source_image.data_exists
            || actual.device_exists != self.source_image.device_exists
            || actual.directories != self.source_image.directories
            || actual.files.len() != self.source_image.files.len()
            || actual.files.iter().any(|(key, file)| {
                self.source_image.files.get(key).is_none_or(|original| {
                    original.path != file.path
                        || original.codec != file.codec
                        || original.bytes != file.bytes
                        || original.owner_execute != file.owner_execute
                })
            })
        {
            return Err(invalid("protection.source_changed"));
        }
        Ok(())
    }
    pub(crate) fn verify_target(&self) -> Result<(), AppError> {
        validate_roots(&self.data_root, &self.device_root, &self.target_root)?;
        let actual = capture(
            &self.target_root,
            &self.target_root,
            &self.target,
            &self.source,
        )?;
        if !actual.data_exists
            || actual.directories != self.source_image.directories
            || actual.files.len() != self.target_files.len()
            || actual.files.values().any(|file| {
                self.target_files
                    .get(&file.relative)
                    .is_none_or(|expected| {
                        expected.codec != file.codec || expected.bytes != file.bytes
                    })
            })
        {
            return Err(invalid("protection.target_changed"));
        }
        for (relative, expected) in &self.target_files {
            verify_target_permissions(&self.target_root.join(relative), expected.owner_execute)?;
        }
        Ok(())
    }
    pub(crate) fn evidence(&self) -> impl Iterator<Item = &GenerationFileEvidence> {
        self.evidence.iter()
    }
    fn verify_empty_target(&self) -> Result<(), AppError> {
        if !config_file_io::directory_exists_no_alias(&self.target_root)? {
            return Ok(());
        }
        for entry in walkdir::WalkDir::new(&self.target_root)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry.map_err(|_| invalid("secret.invalid_storage_path"))?;
            if entry.path() == self.target_root {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&self.target_root)
                .map_err(|_| invalid("secret.invalid_storage_path"))?;
            if entry.file_type().is_dir() {
                config_file_io::directory_exists_no_alias(entry.path())?;
                if !self.source_image.directories.contains(relative) {
                    return Err(invalid("protection.target_not_empty"));
                }
            } else if relative == Path::new(crate::app_store::ISOLATED_SCOPE_IDENTITY_FILE) {
                // Shape only: authenticity remains with the original scope owner.
                read_file(entry.path())?;
            } else {
                return Err(invalid("protection.target_not_empty"));
            }
        }
        Ok(())
    }
}
fn invalid(code: &str) -> AppError {
    AppError::Config(code.into())
}
fn validate_generation(source: &StorageContext, target: &StorageContext) -> Result<(), AppError> {
    for context in [source, target] {
        match context {
            StorageContext::Plaintext(identity) => identity.validate(),
            StorageContext::Encrypted(vault) => vault.metadata().validate(),
        }
        .map_err(crate::secrets::error::secret_error)?;
    }
    if source.identity().store_id() != target.identity().store_id()
        || source.identity().generation_id() == target.identity().generation_id()
    {
        return Err(invalid("protection.invalid_generation"));
    }
    Ok(())
}
fn validate_roots(data: &Path, device: &Path, target: &Path) -> Result<(), AppError> {
    for root in [data, device, target] {
        if !root.is_absolute() {
            return Err(invalid("secret.invalid_storage_path"));
        }
        config_file_io::directory_exists_no_alias(root)?;
    }
    for source in [data, device] {
        if target.starts_with(source) || source.starts_with(target) {
            return Err(invalid("protection.overlapping_generation_roots"));
        }
    }
    if data != device && (data.starts_with(device) || device.starts_with(data)) {
        return Err(invalid("protection.overlapping_generation_roots"));
    }
    Ok(())
}
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
fn read_file(path: &Path) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("secret.invalid_storage_path"))?;
    config_file_io::directory_exists_no_alias(parent)?;
    let meta = std::fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(invalid("secret.invalid_storage_path"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(invalid("secret.invalid_storage_path"));
        }
    }
    let bytes = config_file_io::read_regular_file(path, MAX_FILE_BYTES)
        .map_err(|e| AppError::io(path, e))?
        .ok_or_else(|| invalid("protection.source_changed"))?;
    Ok(Zeroizing::new(bytes))
}
fn excluded_database_paths(root: &Path) -> Result<BTreeSet<PathBuf>, AppError> {
    let mut files = crate::secrets::transition::database_backups(root)?;
    files.push(root.join(crate::config::DB_FILE_NAME));
    let mut excluded = BTreeSet::new();
    for path in files {
        excluded.insert(path.clone());
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut name = path.as_os_str().to_os_string();
            name.push(suffix);
            excluded.insert(name.into());
        }
    }
    Ok(excluded)
}
fn db_shaped(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "db")
        || path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| {
                [".db-wal", ".db-shm", ".db-journal"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
            })
}
fn excluded(
    relative: &Path,
    full: &Path,
    databases: &BTreeSet<PathBuf>,
    separate_device: bool,
) -> bool {
    databases.contains(full)
        || [
            "vault.json",
            super::FILE,
            crate::app_store::ISOLATED_SCOPE_IDENTITY_FILE,
        ]
        .iter()
        .any(|name| relative == Path::new(name))
        || crate::panic_hook::is_diagnostic_path(relative)
        || (separate_device
            && (DeviceFile::registered(relative).is_ok()
                || relative.starts_with(crate::secrets::owned_file::DEVICE_BACKUP_DIR)))
}
/// The original capture's passive data-tree membership, shared with cleanup
/// postflight. This preserves its exclusions and unclaimed lifecycle-artifact
/// refusals without decoding any source bytes or adding another walker.
fn source_membership(
    data: &Path,
    device: &Path,
) -> Result<(SourceInventory, BTreeSet<PathBuf>), AppError> {
    let data_exists = config_file_io::directory_exists_no_alias(data)?;
    let device_exists = config_file_io::directory_exists_no_alias(device)?;
    let databases = excluded_database_paths(data)?;
    let mut paths = BTreeSet::new();
    let mut directories = BTreeSet::new();
    if data_exists {
        let walker = walkdir::WalkDir::new(data)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| {
                entry.path() == data
                    || entry.path().strip_prefix(data).is_ok_and(|relative| {
                        !excluded(relative, entry.path(), &databases, data != device)
                    })
            });
        for entry in walker {
            let entry = entry.map_err(|_| invalid("secret.invalid_storage_path"))?;
            if entry.path() == data {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(data)
                .map_err(|_| invalid("secret.invalid_storage_path"))?;
            if relative.starts_with("backups/vault-recovery")
                && (db_shaped(relative)
                    || relative.file_name().is_some_and(|name| {
                        name == "vault.json"
                            || name == super::FILE
                            || name == crate::app_store::ISOLATED_SCOPE_IDENTITY_FILE
                    }))
            {
                return Err(invalid("protection.unclaimed_recovery_artifact"));
            }
            if relative.parent() == Some(Path::new("backups")) && db_shaped(relative) {
                // The original backup walker did not claim this orphan sidecar.
                return Err(invalid("protection.unclaimed_database_artifact"));
            }
            if entry.file_type().is_dir() {
                config_file_io::directory_exists_no_alias(entry.path())?;
                directories.insert(relative.to_path_buf());
            } else if entry.file_type().is_file() {
                let meta = entry
                    .metadata()
                    .map_err(|_| invalid("secret.invalid_storage_path"))?;
                if meta.len() > MAX_FILE_BYTES {
                    return Err(invalid("protection.generation_file_too_large"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.nlink() != 1 {
                        return Err(invalid("secret.invalid_storage_path"));
                    }
                }
                paths.insert(entry.path().to_path_buf());
            } else {
                return Err(invalid("secret.invalid_storage_path"));
            }
        }
    }
    let image = SourceInventory {
        files: BTreeMap::new(),
        directories,
        data_exists,
        device_exists,
    };
    Ok((image, paths))
}

fn capture(
    data: &Path,
    device: &Path,
    context: &StorageContext,
    peer: &StorageContext,
) -> Result<SourceInventory, AppError> {
    let (mut image, mut paths) = source_membership(data, device)?;
    let members = credential_membership(data, device, &image, &paths)?;
    if members.iter().any(|path| is_durable_staging_name(path)) {
        return Err(invalid("protection.interrupted_credential_write"));
    }
    for plan in files::stage_owned_files_with_storage(data, context, false)? {
        let bytes = Zeroizing::new(plan.ciphertext);
        if !paths.remove(&plan.source) || bytes != read_file(&plan.source)? {
            return Err(invalid("protection.source_changed"));
        }
        add_source(
            &mut image,
            GenerationSourceRoot::Data,
            data,
            plan.source,
            Codec::Owned(plan.file),
            bytes,
        )?;
    }
    for plan in files::stage_device_files_with_storage(device, context)? {
        let bytes = Zeroizing::new(plan.ciphertext);
        if (device == data && !paths.remove(&plan.source)) || bytes != read_file(&plan.source)? {
            return Err(invalid("protection.source_changed"));
        }
        add_source(
            &mut image,
            GenerationSourceRoot::Device,
            device,
            plan.source,
            Codec::Device(plan.file),
            bytes,
        )?;
    }
    if device != data
        && config_file_io::directory_exists_no_alias(
            &device.join(crate::secrets::owned_file::DEVICE_BACKUP_DIR),
        )?
    {
        add_parents(
            &mut image.directories,
            Path::new(crate::secrets::owned_file::DEVICE_BACKUP_DIR)
                .join("placeholder")
                .as_path(),
        );
    }
    for (root, base, path) in settings_sources(data, device) {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("secret.invalid_storage_path"))?;
        if !config_file_io::directory_exists_no_alias(parent)? {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(AppError::io(&path, error)),
            Ok(_) => {}
        }
        let bytes = read_file(&path)?;
        // Original RawValue owner validates duplicates and protected unknown fields.
        let _verified = Zeroizing::new(crate::settings::transcode_settings_with_storage(
            &bytes, context, peer,
        )?);
        if base == data && !paths.remove(&path) {
            return Err(invalid("protection.source_changed"));
        }
        add_source(&mut image, root, base, path, Codec::Settings, bytes)?;
    }
    for path in paths {
        let relative = path
            .strip_prefix(data)
            .map_err(|_| invalid("secret.invalid_storage_path"))?;
        let identity = relative
            .strip_prefix("backups/vault-recovery")
            .unwrap_or(relative);
        if OwnedFile::registered(identity).is_ok() || DeviceFile::registered(relative).is_ok() {
            return Err(invalid("protection.source_changed"));
        }
        let bytes = read_file(&path)?;
        add_source(
            &mut image,
            GenerationSourceRoot::Data,
            data,
            path,
            Codec::Neutral,
            bytes,
        )?;
    }
    Ok(image)
}
/// One physical settings membership table for capture and source postflight.
fn settings_sources<'a>(
    data: &'a Path,
    device: &'a Path,
) -> Vec<(GenerationSourceRoot, &'a Path, PathBuf)> {
    let mut settings = vec![
        (GenerationSourceRoot::Data, data, data.join("settings.json")),
        (
            GenerationSourceRoot::Data,
            data,
            data.join("backups/vault-recovery/settings.json"),
        ),
    ];
    if device != data {
        settings.push((
            GenerationSourceRoot::Device,
            device,
            device.join("settings.json"),
        ));
    }
    settings
}

fn is_durable_staging_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(config_file_io::staging_target_name)
        .is_some()
}

/// Only original registered member locations can claim a durable temporary name.
fn credential_membership(
    data: &Path,
    device: &Path,
    image: &SourceInventory,
    paths: &BTreeSet<PathBuf>,
) -> Result<BTreeSet<PathBuf>, AppError> {
    let mut members: BTreeSet<PathBuf> = files::owned_file_paths_with_staging(data)?
        .into_iter()
        .map(|(_, path)| path)
        .collect();
    members.extend(
        files::device_reset_paths(device)?
            .into_iter()
            .map(|(_, path)| path),
    );
    for (_, _, settings) in settings_sources(data, device) {
        let parent = settings
            .parent()
            .ok_or_else(|| invalid("secret.invalid_storage_path"))?;
        if !config_file_io::directory_exists_no_alias(parent)? {
            continue;
        }
        let filename = settings
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("secret.invalid_storage_path"))?;
        for entry in std::fs::read_dir(parent).map_err(|error| AppError::io(parent, error))? {
            let entry = entry.map_err(|error| AppError::io(parent, error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name != filename && config_file_io::staging_target_name(name) != Some(filename) {
                continue;
            }
            if !entry
                .file_type()
                .map_err(|error| AppError::io(entry.path(), error))?
                .is_file()
            {
                return Err(invalid("secret.invalid_storage_path"));
            }
            members.insert(entry.path());
        }
    }
    // With a separate fixed-device root, old device-named data files are not
    // migrated. But a registered adjacent temporary must not fall through to
    // the data tree's Neutral codec merely because its final target was excluded.
    for path in paths
        .iter()
        .map(PathBuf::as_path)
        .chain(image.directories.iter().map(|relative| relative.as_path()))
    {
        let relative = path.strip_prefix(data).unwrap_or(path);
        if relative
            .to_str()
            .and_then(config_file_io::staging_target_name)
            .is_some_and(|target| DeviceFile::registered(target).is_ok())
        {
            members.insert(data.join(relative));
        }
    }
    Ok(members)
}

fn add_source(
    image: &mut SourceInventory,
    root: GenerationSourceRoot,
    base: &Path,
    path: PathBuf,
    codec: Codec,
    bytes: Zeroizing<Vec<u8>>,
) -> Result<(), AppError> {
    let relative = path
        .strip_prefix(base)
        .map_err(|_| invalid("secret.invalid_storage_path"))?
        .to_path_buf();
    add_parents(&mut image.directories, &relative);
    let key = (root, relative.clone());
    let owner_execute = matches!(codec, Codec::Neutral) && source_owner_execute(&path)?;
    let file = SourceFile {
        root,
        relative,
        path,
        codec,
        bytes,
        owner_execute,
    };
    if image.files.insert(key, file).is_some() {
        return Err(invalid("protection.generation_file_conflict"));
    }
    Ok(())
}
fn add_parents(directories: &mut BTreeSet<PathBuf>, relative: &Path) {
    if let Some(parent) = relative.parent() {
        for directory in parent
            .ancestors()
            .filter(|path| !path.as_os_str().is_empty())
        {
            directories.insert(directory.to_path_buf());
        }
    }
}

/// Semantic comparison uses the original descriptor codecs. Settings are compared
/// through the original archive owner as RawValue fields, not AppSettings defaults.
fn same_content(
    codec: &Codec,
    original: &[u8],
    existing: &[u8],
    source: &StorageContext,
    target: &StorageContext,
) -> Result<bool, AppError> {
    match codec {
        Codec::Owned(file) => {
            Ok(file.decode_storage(source, original)? == file.decode_storage(target, existing)?)
        }
        Codec::Device(file) => {
            Ok(file.decode_storage(source, original)? == file.decode_storage(target, existing)?)
        }
        Codec::Neutral => Ok(original == existing),
        Codec::Settings => {
            crate::settings::settings_images_equal_with_storage(original, source, existing, target)
        }
    }
}

fn source_owner_execute(path: &Path) -> Result<bool, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            std::fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
        Ok(metadata.permissions().mode() & 0o100 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

#[cfg(unix)]
fn preserve_owner_execute(path: &Path, expected: &[u8]) -> Result<(), AppError> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| AppError::io(path, error))?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() != expected.len() as u64 {
        return Err(invalid("protection.target_changed"));
    }
    // Check bytes on the opened regular file, then keep this descriptor through
    // chmod/fsync. This is not inode-bound CAS against an external replacement;
    // the caller retains the original private-directory/lifecycle guard contract.
    let mut readback = Zeroizing::new(Vec::new());
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut readback)
        .map_err(|error| AppError::io(path, error))?;
    if readback.as_slice() != expected {
        return Err(invalid("protection.target_changed"));
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o700))
        .map_err(|error| AppError::io(path, error))?;
    file.sync_all().map_err(|error| AppError::io(path, error))
}
fn verify_target_permissions(path: &Path, owner_execute: bool) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            std::fs::symlink_metadata(path).map_err(|error| AppError::io(path, error))?;
        let expected = 0o600 | if owner_execute { 0o100 } else { 0 };
        if metadata.permissions().mode() & 0o7777 != expected {
            return Err(invalid("protection.target_changed"));
        }
    }
    #[cfg(not(unix))]
    let _ = (path, owner_execute);
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "generation_files_tests.rs"]
pub(crate) mod tests;

/// Passive original-owner membership check; this never reads or deletes payloads.
pub(crate) fn ensure_no_credential_files(data: &Path, device: &Path) -> Result<(), AppError> {
    let (image, paths) = source_membership(data, device)?;
    if !credential_membership(data, device, &image, &paths)?.is_empty() {
        return Err(invalid("protection.source_credentials_remaining"));
    }
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "generation_files_postflight_tests.rs"]
pub(crate) mod postflight_tests;
