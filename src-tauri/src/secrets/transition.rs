//! Authenticated, resumable installation of a whole credential generation.
use super::{
    files::{self, OwnedFile},
    inventory,
    key_store::{save_verified, KeyStore},
    session::{read_metadata, write_durable, write_metadata, LocalVault},
    VaultContext,
};
use crate::{
    database::{vault, Database},
    error::AppError,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

pub(crate) const INTENT: &str = ".vault-transition";
const FORMAT: u32 = 1;

pub(crate) struct SkillsReplacement {
    pub source: PathBuf,
    pub location: crate::services::skill::SkillStorageLocation,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    version: u32,
    id: String,
    next: LocalVault,
    manifest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    previous: LocalVault,
    previous_key: Vec<u8>,
    next: LocalVault,
    artifacts: Vec<Artifact>,
    skills: Option<SkillsTree>,
}
impl Drop for Manifest {
    fn drop(&mut self) {
        self.previous_key.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Artifact {
    destination: Destination,
    digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Destination {
    Database,
    DatabaseBackup { name: String },
    Owned { relative: String, recovery: bool },
    Settings { recovery: bool },
    SkillFile { relative: String },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillsTree {
    location: crate::services::skill::SkillStorageLocation,
    directories: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Checkpoint {
    Staged,
    Intent,
    Database,
    Artifact(usize),
    Metadata,
    Keys,
}

fn invalid() -> AppError {
    AppError::Config("secret.invalid_transition".into())
}
fn db_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn stage_root(root: &Path, id: &str) -> Result<PathBuf, AppError> {
    if uuid::Uuid::parse_str(id)
        .map(|v| v.to_string())
        .ok()
        .as_deref()
        != Some(id)
    {
        return Err(invalid());
    }
    Ok(root.join(format!(".vault-transition-{id}")))
}
fn stage_file(root: &Path, id: &str, index: usize) -> Result<PathBuf, AppError> {
    Ok(stage_root(root, id)?.join(format!("{index}.stage")))
}
fn safe_relative(value: &str) -> Result<&Path, AppError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid());
    }
    Ok(path)
}
#[cfg_attr(not(unix), allow(unused_variables))]
fn sync_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| AppError::io(path, e))?;
    Ok(())
}
fn remove_durable(path: &Path) -> Result<(), AppError> {
    std::fs::remove_file(path).map_err(|e| AppError::io(path, e))?;
    sync_directory(path.parent().ok_or_else(invalid)?)
}
fn regular_file(path: &Path) -> Result<bool, AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(path, e)),
    }
}
fn directory(path: &Path) -> Result<bool, AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(path, e)),
    }
}
fn directory_chain(boundary: &Path, path: &Path) -> Result<(), AppError> {
    let relative = path.strip_prefix(boundary).map_err(|_| invalid())?;
    let mut current = boundary.to_owned();
    directory(&current)?;
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid());
        }
        current.push(component);
        directory(&current)?;
    }
    Ok(())
}
fn validate_destination(root: &Path, target: &Destination) -> Result<(), AppError> {
    if let Some(path) = destination(root, target)? {
        let home = crate::config::get_home_dir();
        let boundary = if matches!(target, Destination::Settings { recovery: false }) {
            home.as_path()
        } else {
            root
        };
        directory_chain(boundary, path.parent().ok_or_else(invalid)?)?;
        regular_file(&path)?;
    }
    Ok(())
}
fn validate_skills_paths(root: &Path, id: &str, skills: &SkillsTree) -> Result<(), AppError> {
    let destination = skills_destination(root, skills);
    let home = crate::config::get_home_dir();
    let boundary = match skills.location {
        crate::services::skill::SkillStorageLocation::LoongPort => root,
        crate::services::skill::SkillStorageLocation::Unified => home.as_path(),
    };
    directory_chain(boundary, destination.parent().ok_or_else(invalid)?)?;
    directory(&destination)?;
    let staging = skills_staging(root, id, skills)?;
    directory(&staging)?;
    directory(&staging.join("ready"))?;
    directory(&staging.join("previous"))?;
    Ok(())
}
fn same_local(left: &LocalVault, right: &LocalVault) -> bool {
    left.metadata == right.metadata
        && left.automatic_unlock == right.automatic_unlock
        && left.migration_state == right.migration_state
}

/// The caller must own the shared sync operation mutex.
pub(crate) fn rotate(
    db: &Database,
    store: &dyn KeyStore,
    password: &str,
    automatic_unlock: bool,
) -> Result<(), AppError> {
    rotate_with_hook(db, store, password, automatic_unlock, &mut |_| Ok(()))
}
fn rotate_with_hook(
    db: &Database,
    store: &dyn KeyStore,
    password: &str,
    automatic_unlock: bool,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    install_with_hook(
        db,
        store,
        |current| {
            current
                .rotate_key()
                .and_then(|next| next.with_password(password))
                .map_err(inventory::secret_error)
        },
        automatic_unlock,
        |source, current, next| {
            let mut memory = Connection::open_in_memory().map_err(db_error)?;
            vault::copy(source, &mut memory)?;
            inventory::transform_database(&memory, Some(current), next)?;
            vault::stamp(&memory, next)?;
            Ok(memory)
        },
        Replacements {
            skills: None,
            settings: None,
        },
        hook,
    )
}

/// Build the replacement under the same session / database locks as local capture.
/// The prepared database must contain ciphertext authenticated by `next`.
/// `skills.source` must remain alive until this synchronous method returns.
pub(crate) fn install_generation<F>(
    db: &Database,
    store: &dyn KeyStore,
    next: VaultContext,
    automatic_unlock: bool,
    prepare_database: F,
    skills: Option<SkillsReplacement>,
) -> Result<(), AppError>
where
    F: FnOnce(&Connection, &VaultContext, &VaultContext) -> Result<Connection, AppError>,
{
    install_with_hook(
        db,
        store,
        |_| Ok(next),
        automatic_unlock,
        prepare_database,
        Replacements {
            skills,
            settings: None,
        },
        &mut |_| Ok(()),
    )
}

pub(crate) fn install_generation_with_settings<F>(
    db: &Database,
    store: &dyn KeyStore,
    next: VaultContext,
    automatic_unlock: bool,
    prepare_database: F,
    skills: Option<SkillsReplacement>,
    settings: crate::settings::AppSettings,
) -> Result<(), AppError>
where
    F: FnOnce(&Connection, &VaultContext, &VaultContext) -> Result<Connection, AppError>,
{
    install_with_hook(
        db,
        store,
        |_| Ok(next),
        automatic_unlock,
        prepare_database,
        Replacements {
            skills,
            settings: Some(settings),
        },
        &mut |_| Ok(()),
    )
}

/// Replacement assets written when a prepared generation is installed.
pub(crate) struct Replacements {
    pub(crate) skills: Option<SkillsReplacement>,
    pub(crate) settings: Option<crate::settings::AppSettings>,
}

fn install_with_hook<N, F>(
    db: &Database,
    store: &dyn KeyStore,
    make_next: N,
    automatic_unlock: bool,
    prepare_database: F,
    replacements: Replacements,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError>
where
    N: FnOnce(&VaultContext) -> Result<VaultContext, AppError>,
    F: FnOnce(&Connection, &VaultContext, &VaultContext) -> Result<Connection, AppError>,
{
    let session = &db.secrets;
    let mut current = session.write()?;
    let root = session.root();
    for name in [INTENT, ".vault-rewrap"] {
        if regular_file(&root.join(name))? {
            return Err(AppError::Config("secret.recovery_required".into()));
        }
    }
    let mut conn = db
        .conn
        .lock()
        .map_err(|_| AppError::Config("secret.session_unavailable".into()))?;
    vault::check_identity(&conn, &current)?;
    let previous = read_metadata(root)?;
    if previous.metadata != *current.metadata() {
        return Err(invalid());
    }
    let migration = current
        .open(&["local", "migration-state"], &previous.migration_state)
        .map_err(inventory::secret_error)?;
    let phase: serde_json::Value = serde_json::from_slice(&migration).map_err(|_| invalid())?;
    if phase.get("phase").and_then(|v| v.as_str()) != Some("complete") {
        return Err(AppError::Config("secret.migration_required".into()));
    }
    let next = make_next(&current)?;
    if next.metadata().wrapped_key.is_none()
        && !(automatic_unlock && previous.automatic_unlock && next.metadata() == current.metadata())
    {
        return Err(AppError::Config("secret.password_required".into()));
    }
    if current.metadata().vault_id == next.metadata().vault_id
        && next.metadata().revision < current.metadata().revision
    {
        return Err(AppError::Config("secret.stale_generation".into()));
    }
    let next_local = LocalVault {
        metadata: next.metadata().clone(),
        automatic_unlock,
        migration_state: next
            .seal(&["local", "migration-state"], &migration)
            .map_err(inventory::secret_error)?,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let staging = stage_root(root, &id)?;
    std::fs::create_dir(&staging).map_err(|e| AppError::io(&staging, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| AppError::io(&staging, e))?;
    }
    let mut manifest = Manifest {
        previous,
        previous_key: current.export_key().to_vec(),
        next: next_local.clone(),
        artifacts: Vec::new(),
        skills: None,
    };
    let capture = (|| {
        let memory = prepare_database(&conn, &current, &next)?;
        stage_database(
            root,
            &id,
            &mut manifest,
            Destination::Database,
            &memory,
            &next,
        )?;
        for path in database_backups(root)? {
            let source = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(db_error)?;
            if vault::is_prefork_relic(&source)? {
                log::warn!(
                    "跳过前代上游备份（本代无法迁移，保留原样、可能含明文）: {}",
                    path.display()
                );
                drop(source);
                continue;
            }
            let metadata = vault::stored_metadata(&source)?
                .ok_or_else(|| AppError::Config("secret.plaintext_backup".into()))?;
            if metadata.vault_id != current.metadata().vault_id
                || metadata.key_id != current.metadata().key_id
            {
                return Err(AppError::Config("secret.source_key_required".into()));
            }
            let source_vault = VaultContext::from_key(metadata, current.export_key())
                .map_err(inventory::secret_error)?;
            inventory::validate_database(&source, &source_vault)?;
            let mut memory = Connection::open_in_memory().map_err(db_error)?;
            vault::copy(&source, &mut memory)?;
            inventory::transform_database(&memory, Some(&source_vault), &next)?;
            vault::stamp(&memory, &next)?;
            stage_database(
                root,
                &id,
                &mut manifest,
                Destination::DatabaseBackup {
                    name: path
                        .file_name()
                        .and_then(|v| v.to_str())
                        .ok_or_else(invalid)?
                        .to_owned(),
                },
                &memory,
                &next,
            )?;
        }
        for plan in files::stage_owned_files_with_vault(root, &current, false)? {
            let plaintext = plan.file.decode(&current, &plan.ciphertext)?;
            let bytes = plan.file.encode(&next, &plaintext)?;
            let recovery = plan.source.starts_with(root.join("backups/vault-recovery"));
            stage_bytes(
                root,
                &id,
                &mut manifest,
                Destination::Owned {
                    relative: plan
                        .file
                        .relative_path()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    recovery,
                },
                &bytes,
            )?;
        }
        for recovery in [false, true] {
            let path = settings_destination(root, recovery);
            let staged_settings = (!recovery)
                .then_some(replacements.settings.as_ref())
                .flatten();
            if let Some(settings) = staged_settings {
                let bytes = crate::settings::encode_settings_with_vault(settings, &next)?;
                stage_bytes(
                    root,
                    &id,
                    &mut manifest,
                    Destination::Settings { recovery },
                    &bytes,
                )?;
            } else if regular_file(&path)? {
                let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
                let settings = crate::settings::decode_settings_with_vault(&bytes, &current)?;
                let bytes = crate::settings::encode_settings_with_vault(&settings, &next)?;
                stage_bytes(
                    root,
                    &id,
                    &mut manifest,
                    Destination::Settings { recovery },
                    &bytes,
                )?;
            }
        }
        if let Some(skills) = replacements.skills {
            capture_skills(root, &id, &next, &mut manifest, &skills)?;
        }
        validate_stages(root, &id, &next, &manifest)?;
        hook(Checkpoint::Staged)?;
        Ok(())
    })();
    if let Err(error) = capture {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }
    let plaintext = Zeroizing::new(serde_json::to_vec(&manifest).map_err(|_| invalid())?);
    let sealed = next
        .seal(&["local", "generation-transition", &id], &plaintext)
        .map_err(inventory::secret_error)?;
    let intent = Intent {
        version: FORMAT,
        id,
        next: next_local,
        manifest: sealed,
    };
    let bytes = serde_json::to_vec(&intent).map_err(|_| invalid())?;
    session.set_blocked(true);
    write_durable(&root.join(INTENT), &bytes)?;
    hook(Checkpoint::Intent)?;
    finish(
        root,
        &intent,
        &manifest,
        &next,
        store,
        Some(&mut conn),
        hook,
    )?;
    *current = next;
    session.set_blocked(false);
    Ok(())
}

fn stage_bytes(
    root: &Path,
    id: &str,
    manifest: &mut Manifest,
    destination: Destination,
    bytes: &[u8],
) -> Result<(), AppError> {
    let path = stage_file(root, id, manifest.artifacts.len())?;
    write_durable(&path, bytes)?;
    manifest.artifacts.push(Artifact {
        destination,
        digest: hash(bytes),
    });
    Ok(())
}
fn stage_database(
    root: &Path,
    id: &str,
    manifest: &mut Manifest,
    destination: Destination,
    memory: &Connection,
    next: &VaultContext,
) -> Result<(), AppError> {
    vault::check_identity(memory, next)?;
    inventory::validate_database(memory, next)?;
    memory
        .execute_batch("PRAGMA secure_delete=ON; VACUUM;")
        .map_err(db_error)?;
    let bytes = memory.serialize(rusqlite::MAIN_DB).map_err(db_error)?;
    stage_bytes(root, id, manifest, destination, &bytes)?;
    Ok(())
}
fn database_backups(root: &Path) -> Result<Vec<PathBuf>, AppError> {
    let directory = root.join("backups");
    if !self::directory(&directory)? {
        return Ok(Vec::new());
    }
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AppError::io(&directory, e)),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| AppError::io(&directory, e))?;
        let path = entry.path();
        if path.extension().is_some_and(|v| v == "db") {
            if !regular_file(&path)? {
                return Err(invalid());
            }
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}
fn settings_destination(root: &Path, recovery: bool) -> PathBuf {
    if recovery {
        root.join("backups/vault-recovery/settings.json")
    } else {
        crate::settings::settings_path()
    }
}
fn destination(root: &Path, target: &Destination) -> Result<Option<PathBuf>, AppError> {
    Ok(Some(match target {
        Destination::Database => root.join(crate::config::DB_FILE_NAME),
        Destination::DatabaseBackup { name } => {
            let name = safe_relative(name)?;
            if name.components().count() != 1 || name.extension().is_none_or(|v| v != "db") {
                return Err(invalid());
            }
            root.join("backups").join(name)
        }
        Destination::Owned { relative, recovery } => {
            let file = OwnedFile::registered(safe_relative(relative)?)?;
            if *recovery {
                root.join("backups/vault-recovery")
                    .join(file.relative_path())
            } else {
                root.join(file.relative_path())
            }
        }
        Destination::Settings { recovery } => settings_destination(root, *recovery),
        Destination::SkillFile { relative } => {
            safe_relative(relative)?;
            return Ok(None);
        }
    }))
}

fn database_image(bytes: &[u8], next: &VaultContext) -> Result<Connection, AppError> {
    let mut memory = Connection::open_in_memory().map_err(db_error)?;
    memory
        .deserialize_read_exact(rusqlite::MAIN_DB, bytes, bytes.len(), false)
        .map_err(db_error)?;
    let integrity: String = memory
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(db_error)?;
    if integrity != "ok" {
        return Err(invalid());
    }
    vault::preflight_connection(&memory)?;
    vault::check_identity(&memory, next)?;
    inventory::validate_database(&memory, next)?;
    Ok(memory)
}
fn read_stage(
    root: &Path,
    id: &str,
    index: usize,
    artifact: &Artifact,
) -> Result<Vec<u8>, AppError> {
    if !directory(&stage_root(root, id)?)? {
        return Err(invalid());
    }
    let path = stage_file(root, id, index)?;
    if !regular_file(&path)? {
        return Err(invalid());
    }
    let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
    if hash(&bytes) != artifact.digest {
        return Err(AppError::Config("secret.stage_tampered".into()));
    }
    Ok(bytes)
}
fn validate_stages(
    root: &Path,
    id: &str,
    next: &VaultContext,
    manifest: &Manifest,
) -> Result<(), AppError> {
    if manifest.artifacts.is_empty()
        || manifest
            .artifacts
            .iter()
            .filter(|a| matches!(a.destination, Destination::Database))
            .count()
            != 1
    {
        return Err(invalid());
    }
    let mut destinations = std::collections::HashSet::new();
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        let encoded = serde_json::to_string(&artifact.destination).map_err(|_| invalid())?;
        if !destinations.insert(encoded) {
            return Err(invalid());
        }
        validate_destination(root, &artifact.destination)?;
        let bytes = read_stage(root, id, index, artifact)?;
        match &artifact.destination {
            Destination::Database | Destination::DatabaseBackup { .. } => {
                database_image(&bytes, next)?;
            }
            Destination::Owned { relative, .. } => {
                OwnedFile::registered(relative)?.decode(next, &bytes)?;
            }
            Destination::Settings { .. } => {
                crate::settings::decode_settings_with_vault(&bytes, next)?;
            }
            Destination::SkillFile { relative } => {
                if manifest.skills.is_none() {
                    return Err(invalid());
                }
                next.open(
                    &["local", "transition-skill", id, relative],
                    std::str::from_utf8(&bytes).map_err(|_| invalid())?,
                )
                .map_err(inventory::secret_error)?;
            }
        }
    }
    if let Some(skills) = &manifest.skills {
        validate_skills_paths(root, id, skills)?;
        for directory in &skills.directories {
            safe_relative(directory)?;
        }
    }
    Ok(())
}
fn validate_authorization(
    root: &Path,
    intent: &Intent,
    manifest: &Manifest,
) -> Result<(), AppError> {
    if !same_local(&intent.next, &manifest.next) {
        return Err(invalid());
    }
    VaultContext::from_key(
        manifest.previous.metadata.clone(),
        Zeroizing::new(manifest.previous_key.clone()),
    )
    .map_err(inventory::secret_error)?;
    let committed = read_metadata(root)?;
    if !same_local(&committed, &manifest.previous) && !same_local(&committed, &manifest.next) {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    Ok(())
}

fn remove_key(store: &dyn KeyStore, vault_id: &str, key_id: &str) -> Result<(), AppError> {
    store
        .remove(vault_id, key_id)
        .map_err(|_| AppError::Config("secret.store_unavailable".into()))?;
    if store
        .load(vault_id, key_id)
        .map_err(|_| AppError::Config("secret.store_unavailable".into()))?
        .is_some()
    {
        return Err(AppError::Config("secret.key_removal_failed".into()));
    }
    Ok(())
}
fn finish(
    root: &Path,
    intent: &Intent,
    manifest: &Manifest,
    next: &VaultContext,
    store: &dyn KeyStore,
    active: Option<&mut Connection>,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    validate_authorization(root, intent, manifest)?;
    validate_stages(root, &intent.id, next, manifest)?;
    if manifest.next.automatic_unlock {
        save_verified(
            store,
            &next.metadata().vault_id,
            &next.metadata().key_id,
            &next.export_key(),
        )
        .map_err(|_| AppError::Config("secret.store_unavailable".into()))?;
    }
    let main_path = root.join(crate::config::DB_FILE_NAME);
    vault::preflight(&main_path)?;
    let mut opened;
    let connection = if let Some(active) = active {
        active
    } else {
        if !regular_file(&main_path)? {
            return Err(invalid());
        }
        opened = Connection::open(&main_path).map_err(db_error)?;
        &mut opened
    };
    let stored = vault::stored_metadata(connection)?.ok_or_else(invalid)?;
    if stored != manifest.previous.metadata && stored != manifest.next.metadata {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    if let Some(skills) = &manifest.skills {
        prepare_skills_install(root, &intent.id, next, manifest, skills)?;
    }
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        let bytes = read_stage(root, &intent.id, index, artifact)?;
        match &artifact.destination {
            Destination::Database => {
                let memory = database_image(&bytes, next)?;
                connection
                    .execute_batch("PRAGMA secure_delete=ON;")
                    .map_err(db_error)?;
                vault::copy(&memory, connection)?;
                connection
                    .execute_batch(
                        "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; VACUUM;",
                    )
                    .map_err(db_error)?;
                // VACUUM 以改名重建数据库文件，重建产物携带的是临时目录
                // 的 DACL——重收紧后再 fsync。
                crate::config::ensure_private_file(&main_path)?;
                crate::config::sync_private_file(&main_path)?;
                sync_directory(root)?;
                hook(Checkpoint::Database)?;
            }
            Destination::SkillFile { .. } => {}
            other => {
                write_durable(&destination(root, other)?.ok_or_else(invalid)?, &bytes)?;
                hook(Checkpoint::Artifact(index))?;
            }
        }
    }
    if let Some(skills) = &manifest.skills {
        commit_skills(root, &intent.id, skills)?;
    }
    write_metadata(root, &manifest.next)?;
    hook(Checkpoint::Metadata)?;
    let previous = &manifest.previous.metadata;
    let key_changed = (previous.vault_id.as_str(), previous.key_id.as_str())
        != (
            next.metadata().vault_id.as_str(),
            next.metadata().key_id.as_str(),
        );
    // The journal fixes both policies before any key-store write. Only the
    // previous automatic-unlock owner can owe revocation of an existing key.
    if manifest.previous.automatic_unlock && (key_changed || !manifest.next.automatic_unlock) {
        remove_key(store, &previous.vault_id, &previous.key_id)?;
    }
    hook(Checkpoint::Keys)?;
    remove_durable(&root.join(INTENT))?;
    // Once intent removal is durable, only obsolete staging artifacts remain.
    if let Some(skills) = &manifest.skills {
        let _ = std::fs::remove_dir_all(skills_staging(root, &intent.id, skills)?);
    }
    let _ = std::fs::remove_dir_all(stage_root(root, &intent.id)?);
    Ok(())
}

/// Called before selecting ordinary vault metadata, so only the new password is needed.
pub(crate) fn recover(
    root: &Path,
    store: &dyn KeyStore,
    password: Option<&str>,
) -> Result<(), AppError> {
    let path = root.join(INTENT);
    if !regular_file(&path)? {
        return Ok(());
    }
    if regular_file(&root.join(".vault-rewrap"))? {
        return Err(AppError::Config("secret.conflicting_transition".into()));
    }
    let metadata = std::fs::metadata(&path).map_err(|e| AppError::io(&path, e))?;
    if metadata.len() > 32 * 1024 * 1024 {
        return Err(invalid());
    }
    let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
    let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if intent.version != FORMAT {
        return Err(invalid());
    }
    stage_root(root, &intent.id)?;
    let next = if let Some(password) = password {
        VaultContext::from_password(intent.next.metadata.clone(), password)
            .map_err(inventory::secret_error)?
    } else {
        let key = store
            .load(&intent.next.metadata.vault_id, &intent.next.metadata.key_id)
            .map_err(|_| AppError::Config("secret.store_unavailable".into()))?
            .ok_or_else(|| AppError::Config("secret.locked".into()))?;
        VaultContext::from_key(intent.next.metadata.clone(), key)
            .map_err(inventory::secret_error)?
    };
    let plaintext = next
        .open(
            &["local", "generation-transition", &intent.id],
            &intent.manifest,
        )
        .map_err(inventory::secret_error)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    finish(
        root,
        &intent,
        &manifest,
        &next,
        store,
        None,
        &mut |_| Ok(()),
    )
}

fn skills_destination(root: &Path, skills: &SkillsTree) -> PathBuf {
    match skills.location {
        crate::services::skill::SkillStorageLocation::LoongPort => root.join("skills"),
        crate::services::skill::SkillStorageLocation::Unified => {
            crate::config::get_home_dir().join(".agents/skills")
        }
    }
}
fn skills_staging(root: &Path, id: &str, skills: &SkillsTree) -> Result<PathBuf, AppError> {
    stage_root(root, id)?;
    Ok(skills_destination(root, skills)
        .parent()
        .ok_or_else(invalid)?
        .join(format!(".loongport-skills-transition-{id}")))
}
fn capture_skills(
    root: &Path,
    id: &str,
    next: &VaultContext,
    manifest: &mut Manifest,
    replacement: &SkillsReplacement,
) -> Result<(), AppError> {
    let mut directories = Vec::new();
    let mut pending = vec![replacement.source.clone()];
    while let Some(directory) = pending.pop() {
        let metadata =
            std::fs::symlink_metadata(&directory).map_err(|e| AppError::io(&directory, e))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid());
        }
        for entry in std::fs::read_dir(&directory).map_err(|e| AppError::io(&directory, e))? {
            let entry = entry.map_err(|e| AppError::io(&directory, e))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&replacement.source)
                .map_err(|_| invalid())?
                .to_string_lossy()
                .replace('\\', "/");
            safe_relative(&relative)?;
            let kind = entry.file_type().map_err(|e| AppError::io(&path, e))?;
            if kind.is_dir() {
                directories.push(relative);
                pending.push(path);
            } else if kind.is_file() {
                let plaintext =
                    Zeroizing::new(std::fs::read(&path).map_err(|e| AppError::io(&path, e))?);
                let bytes = next
                    .seal(&["local", "transition-skill", id, &relative], &plaintext)
                    .map_err(inventory::secret_error)?;
                stage_bytes(
                    root,
                    id,
                    manifest,
                    Destination::SkillFile { relative },
                    bytes.as_bytes(),
                )?;
            } else {
                return Err(invalid());
            }
        }
    }
    directories.sort();
    manifest.skills = Some(SkillsTree {
        location: replacement.location,
        directories,
    });
    Ok(())
}
fn prepare_skills_install(
    root: &Path,
    id: &str,
    next: &VaultContext,
    manifest: &Manifest,
    skills: &SkillsTree,
) -> Result<(), AppError> {
    let staging = skills_staging(root, id, skills)?;
    validate_skills_paths(root, id, skills)?;
    let ready = staging.join("ready");
    if directory(&ready)? {
        std::fs::remove_dir_all(&ready).map_err(|e| AppError::io(&ready, e))?;
    }
    std::fs::create_dir_all(&ready).map_err(|e| AppError::io(&ready, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&staging, &ready] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| AppError::io(path, e))?;
        }
    }
    for directory in &skills.directories {
        let path = ready.join(safe_relative(directory)?);
        std::fs::create_dir_all(&path).map_err(|e| AppError::io(&path, e))?;
    }
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        if let Destination::SkillFile { relative } = &artifact.destination {
            let bytes = read_stage(root, id, index, artifact)?;
            let plaintext = next
                .open(
                    &["local", "transition-skill", id, relative],
                    std::str::from_utf8(&bytes).map_err(|_| invalid())?,
                )
                .map_err(inventory::secret_error)?;
            write_durable(&ready.join(safe_relative(relative)?), &plaintext)?;
        }
    }
    for directory in skills.directories.iter().rev() {
        sync_directory(&ready.join(safe_relative(directory)?))?;
    }
    sync_directory(&ready)?;
    sync_directory(&staging)
}
fn commit_skills(root: &Path, id: &str, skills: &SkillsTree) -> Result<(), AppError> {
    validate_skills_paths(root, id, skills)?;
    let destination = skills_destination(root, skills);
    let staging = skills_staging(root, id, skills)?;
    let previous = staging.join("previous");
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(invalid())
        }
        Ok(_) => {
            if directory(&previous)? {
                std::fs::remove_dir_all(&destination).map_err(|e| AppError::io(&destination, e))?;
            } else {
                std::fs::rename(&destination, &previous)
                    .map_err(|e| AppError::io(&destination, e))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppError::io(&destination, e)),
    }
    sync_directory(destination.parent().ok_or_else(invalid)?)?;
    std::fs::rename(staging.join("ready"), &destination)
        .map_err(|e| AppError::io(&destination, e))?;
    sync_directory(destination.parent().ok_or_else(invalid)?)?;
    sync_directory(&staging)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        files::{CredentialFile, OwnedFile},
        inventory,
        session::{write_durable, SecretSession},
        testing::MemoryKeyStore,
    };

    struct Fixture {
        _temporary: tempfile::TempDir,
        previous_home: Option<std::ffi::OsString>,
        root: PathBuf,
        db: Database,
        store: MemoryKeyStore,
    }
    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
            let root = temporary.path().join(crate::APP_DIR_NAME);
            let store = MemoryKeyStore::default();
            let session = SecretSession::open(&root, &store, None).unwrap();
            let conn = vault::prepare(
                &root.join(crate::config::DB_FILE_NAME),
                &session.read().unwrap(),
            )
            .unwrap();
            let db = Database::from_connection(conn, session.clone());
            db.set_setting("global_proxy_url", "checkpoint-canary")
                .unwrap();
            CredentialFile::Codex
                .write(&session, br#"{"token":"checkpoint-canary"}"#)
                .unwrap();
            session.complete_migration().unwrap();
            Self {
                _temporary: temporary,
                previous_home,
                root,
                db,
                store,
            }
        }
        fn interrupt(&self, at: Checkpoint) {
            assert!(rotate_with_hook(
                &self.db,
                &self.store,
                "next recovery password",
                false,
                &mut |point| {
                    if point == at {
                        Err(AppError::Config("injected interruption".into()))
                    } else {
                        Ok(())
                    }
                }
            )
            .is_err());
        }
        fn intent(&self) -> Intent {
            serde_json::from_slice(&std::fs::read(self.root.join(INTENT)).unwrap()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match &self.previous_home {
                Some(home) => std::env::set_var("CC_SWITCH_TEST_HOME", home),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn interruptions_recover_with_only_the_new_password() {
        for point in [
            Checkpoint::Staged,
            Checkpoint::Intent,
            Checkpoint::Database,
            Checkpoint::Artifact(1),
            Checkpoint::Metadata,
            Checkpoint::Keys,
        ] {
            let fixture = Fixture::new();
            let old = read_metadata(&fixture.root).unwrap();
            fixture.interrupt(point);
            if point == Checkpoint::Staged {
                assert!(fixture.db.secrets.read().is_ok());
                assert!(!fixture.root.join(INTENT).exists());
                assert_eq!(read_metadata(&fixture.root).unwrap().metadata, old.metadata);
                continue;
            }
            assert!(fixture.db.secrets.read().is_err());
            assert!(fixture.root.join(INTENT).exists());
            assert!(SecretSession::open_existing(
                &fixture.root,
                &fixture.store,
                Some("wrong password")
            )
            .is_err());
            let recovered = SecretSession::open_existing(
                &fixture.root,
                &fixture.store,
                Some("next recovery password"),
            )
            .unwrap();
            let next = recovered.read().unwrap();
            assert_ne!(old.metadata.key_id, next.metadata().key_id);
            let connection =
                Connection::open(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
            vault::check_identity(&connection, &next).unwrap();
            inventory::validate_database(&connection, &next).unwrap();
            assert!(CredentialFile::Codex
                .decode(
                    &next,
                    &std::fs::read(fixture.root.join("codex_oauth_auth.json")).unwrap()
                )
                .is_ok());
            assert!(!fixture.root.join(INTENT).exists());
            assert!(fixture
                .store
                .load(&old.metadata.vault_id, &old.metadata.key_id)
                .unwrap()
                .is_none());
        }
    }

    #[test]
    #[serial_test::serial]
    fn tampered_stage_and_public_intent_leave_active_files_unchanged() {
        for tamper in 0..3 {
            let fixture = Fixture::new();
            let original = std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap();
            let original_auth = std::fs::read(fixture.root.join("codex_oauth_auth.json")).unwrap();
            fixture.interrupt(Checkpoint::Intent);
            let mut intent = fixture.intent();
            if tamper == 2 {
                let next = VaultContext::from_password(
                    intent.next.metadata.clone(),
                    "next recovery password",
                )
                .unwrap();
                let plain = next
                    .open(
                        &["local", "generation-transition", &intent.id],
                        &intent.manifest,
                    )
                    .unwrap();
                let mut manifest: Manifest = serde_json::from_slice(&plain).unwrap();
                manifest.previous_key[0] ^= 1;
                intent.manifest = next
                    .seal(
                        &["local", "generation-transition", &intent.id],
                        &serde_json::to_vec(&manifest).unwrap(),
                    )
                    .unwrap();
                write_durable(
                    &fixture.root.join(INTENT),
                    &serde_json::to_vec(&intent).unwrap(),
                )
                .unwrap();
            } else if tamper == 1 {
                intent.next.automatic_unlock = !intent.next.automatic_unlock;
                write_durable(
                    &fixture.root.join(INTENT),
                    &serde_json::to_vec(&intent).unwrap(),
                )
                .unwrap();
            } else {
                write_durable(
                    &stage_file(&fixture.root, &intent.id, 0).unwrap(),
                    b"corrupted",
                )
                .unwrap();
            }
            assert!(SecretSession::open_existing(
                &fixture.root,
                &fixture.store,
                Some("next recovery password")
            )
            .is_err());
            assert_eq!(
                std::fs::read(fixture.root.join(crate::config::DB_FILE_NAME)).unwrap(),
                original
            );
            assert_eq!(
                std::fs::read(fixture.root.join("codex_oauth_auth.json")).unwrap(),
                original_auth
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn failed_key_removal_keeps_the_session_blocked_until_recovery() {
        use super::super::key_store::KeyStoreError;
        struct FailRemove<'a>(&'a MemoryKeyStore);
        impl KeyStore for FailRemove<'_> {
            fn load(
                &self,
                vault: &str,
                key: &str,
            ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
                self.0.load(vault, key)
            }
            fn save(&self, vault: &str, key: &str, bytes: &[u8]) -> Result<(), KeyStoreError> {
                self.0.save(vault, key, bytes)
            }
            fn remove(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
                Err(KeyStoreError::Unavailable)
            }
        }
        let fixture = Fixture::new();
        let old = read_metadata(&fixture.root).unwrap();
        assert!(rotate(
            &fixture.db,
            &FailRemove(&fixture.store),
            "next recovery password",
            false
        )
        .is_err());
        assert!(fixture.db.secrets.read().is_err());
        assert!(fixture
            .store
            .load(&old.metadata.vault_id, &old.metadata.key_id)
            .unwrap()
            .is_some());
        let recovered = SecretSession::open_existing(
            &fixture.root,
            &fixture.store,
            Some("next recovery password"),
        )
        .unwrap();
        assert_ne!(
            old.metadata.key_id,
            recovered.read().unwrap().metadata().key_id
        );
        assert!(fixture
            .store
            .load(&old.metadata.vault_id, &old.metadata.key_id)
            .unwrap()
            .is_none());
        assert!(!fixture.root.join(INTENT).exists());
    }

    #[test]
    #[serial_test::serial]
    fn same_key_install_keeps_automatic_unlock_and_recovers_skills_as_one_generation() {
        let fixture = Fixture::new();
        let source = fixture.root.join("incoming-skills");
        std::fs::create_dir_all(source.join("example/empty")).unwrap();
        std::fs::write(source.join("example/SKILL.md"), b"new skill canary").unwrap();
        std::fs::create_dir(fixture.root.join("skills")).unwrap();
        std::fs::write(fixture.root.join("skills/old.txt"), b"old skill").unwrap();
        let old = read_metadata(&fixture.root).unwrap();
        assert!(install_with_hook(
            &fixture.db,
            &fixture.store,
            |current| current
                .with_password("next recovery password")
                .map_err(inventory::secret_error),
            true,
            |current, _, next| {
                let mut memory = Connection::open_in_memory().unwrap();
                vault::copy(current, &mut memory)?;
                vault::stamp(&memory, next)?;
                Ok(memory)
            },
            Replacements {
                skills: Some(SkillsReplacement {
                    source,
                    location: crate::services::skill::SkillStorageLocation::LoongPort
                }),
                settings: None,
            },
            &mut |point| if point == Checkpoint::Metadata {
                Err(AppError::Config("injected interruption".into()))
            } else {
                Ok(())
            }
        )
        .is_err());
        assert_eq!(
            std::fs::read(fixture.root.join("skills/example/SKILL.md")).unwrap(),
            b"new skill canary"
        );
        let recovered = SecretSession::open_existing(&fixture.root, &fixture.store, None).unwrap();
        assert_eq!(
            recovered.read().unwrap().metadata().key_id,
            old.metadata.key_id
        );
        assert!(fixture
            .store
            .load(&old.metadata.vault_id, &old.metadata.key_id)
            .unwrap()
            .is_some());
        assert!(!fixture.root.join("skills/old.txt").exists());
        assert!(fixture.root.join("skills/example/empty").is_dir());
        assert_eq!(
            std::fs::read(fixture.root.join("skills/example/SKILL.md")).unwrap(),
            b"new skill canary"
        );
        assert!(!fixture.root.join(INTENT).exists());
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn recovery_rejects_a_symlinked_stage_directory() {
        let fixture = Fixture::new();
        fixture.interrupt(Checkpoint::Intent);
        let intent = fixture.intent();
        let staging = stage_root(&fixture.root, &intent.id).unwrap();
        let elsewhere = fixture.root.join("redirected-stage");
        std::fs::rename(&staging, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &staging).unwrap();
        assert!(SecretSession::open_existing(
            &fixture.root,
            &fixture.store,
            Some("next recovery password")
        )
        .is_err());
    }

    #[test]
    #[serial_test::serial]
    fn automatic_only_content_install_preserves_the_current_generation() {
        let fixture = Fixture::new();
        let current = fixture.db.secrets.read().unwrap().clone();
        install_generation(
            &fixture.db,
            &fixture.store,
            current.clone(),
            true,
            |conn, _, _| {
                let mut staged = Connection::open_in_memory().unwrap();
                vault::copy(conn, &mut staged)?;
                staged.execute(
                    "INSERT INTO settings(key,value) VALUES ('restored-content','kept')",
                    [],
                )?;
                Ok(staged)
            },
            None,
        )
        .unwrap();
        assert_eq!(
            fixture
                .db
                .get_setting("restored-content")
                .unwrap()
                .as_deref(),
            Some("kept")
        );
        assert_eq!(
            fixture.db.secrets.read().unwrap().metadata(),
            current.metadata()
        );
        let reopened = SecretSession::open_existing(&fixture.root, &fixture.store, None).unwrap();
        assert_eq!(reopened.read().unwrap().metadata(), current.metadata());
        assert!(install_generation(
            &fixture.db,
            &fixture.store,
            current.clone(),
            false,
            |_, _, _| unreachable!(),
            None
        )
        .is_err());
        assert!(install_generation(
            &fixture.db,
            &fixture.store,
            current.rotate_key().unwrap(),
            true,
            |_, _, _| unreachable!(),
            None
        )
        .is_err());
    }

    #[test]
    #[serial_test::serial]
    fn rotation_reencrypts_current_database_owned_files_settings_and_backups() {
        let temporary = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temporary.path());
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match &self.0 {
                    Some(home) => std::env::set_var("CC_SWITCH_TEST_HOME", home),
                    None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
                }
            }
        }
        let _home = RestoreHome(previous_home);
        let root = temporary.path().join(crate::APP_DIR_NAME);
        let store = MemoryKeyStore::default();
        let session = SecretSession::open(&root, &store, None).unwrap();
        let conn = crate::database::vault::prepare(
            &root.join(crate::config::DB_FILE_NAME),
            &session.read().unwrap(),
        )
        .unwrap();
        let db = Database::from_connection(conn, session.clone());
        db.set_setting("global_proxy_url", "rotation-canary")
            .unwrap();
        CredentialFile::Codex
            .write(&session, br#"{"token":"rotation-canary"}"#)
            .unwrap();
        OwnedFile::registered("backups/backup_20260914_010203.json")
            .unwrap()
            .write(&session, br#"{"key":"rotation-canary"}"#)
            .unwrap();
        let settings = crate::settings::encrypt_legacy_settings_with_vault(
            br#"{"webdavBackup":{"password":"rotation-canary"}}"#,
            &session.read().unwrap(),
        )
        .unwrap();
        write_durable(&crate::settings::settings_path(), &settings).unwrap();
        write_durable(
            &root.join("backups/vault-recovery/settings.json"),
            &settings,
        )
        .unwrap();
        session.complete_migration().unwrap();
        let old = super::super::VaultContext::from_key(
            session.read().unwrap().metadata().clone(),
            session.read().unwrap().export_key(),
        )
        .unwrap();
        rotate(&db, &store, "new generation protection password", false).unwrap();
        let next = session.read().unwrap();
        assert_eq!(old.metadata().vault_id, next.metadata().vault_id);
        assert_ne!(old.metadata().key_id, next.metadata().key_id);
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT value FROM settings WHERE key='global_proxy_url'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .map(|raw| inventory::open_db(
                    &next,
                    "settings",
                    "value",
                    &["global_proxy_url"],
                    &raw
                )
                .unwrap())
                .unwrap(),
            "rotation-canary"
        );
        assert!(
            super::super::VaultContext::from_key(next.metadata().clone(), old.export_key())
                .is_err()
        );
        assert!(CredentialFile::Codex
            .decode(
                &old,
                &std::fs::read(root.join("codex_oauth_auth.json")).unwrap()
            )
            .is_err());
        let plans = super::super::files::stage_owned_files_with_vault(&root, &next, false).unwrap();
        assert!(!plans.is_empty());
        for name in [
            crate::settings::settings_path(),
            root.join("backups/vault-recovery/settings.json"),
        ] {
            let bytes = std::fs::read(name).unwrap();
            crate::settings::decode_settings_with_vault(&bytes, &next).unwrap();
            assert!(crate::settings::decode_settings_with_vault(&bytes, &old).is_err());
            assert!(!String::from_utf8_lossy(&bytes).contains("rotation-canary"));
        }
        for entry in std::fs::read_dir(root.join("backups")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "db") {
                let conn = rusqlite::Connection::open_with_flags(
                    &path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                crate::database::vault::check_identity(&conn, &next).unwrap();
                inventory::validate_database(&conn, &next).unwrap();
            }
        }
        assert!(store
            .load(&old.metadata().vault_id, &old.metadata().key_id)
            .unwrap()
            .is_none());
        assert!(store
            .load(&next.metadata().vault_id, &next.metadata().key_id)
            .unwrap()
            .is_none());
        drop(next);
        assert!(SecretSession::open_existing(&root, &store, None).is_err());
        SecretSession::open_existing(&root, &store, Some("new generation protection password"))
            .unwrap();
    }
}
