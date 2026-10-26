//! Same-storage DB + skills installation, owned by the original transition journal.
use super::*;
use crate::database::backup::ValidatedSyncSnapshot;
use crate::secrets::{protection, session, storage::StorageContext, VaultMetadata};
use std::collections::BTreeMap;

const CONTENT_FORMAT: u32 = 3;
const CONTENT_KIND: &str = "sameStorageContent";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContentIntent {
    version: u32,
    kind: String,
    id: String,
    authentication: VaultMetadata,
    manifest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContentManifest {
    root: PathBuf,
    skills_root: PathBuf,
    protection: Vec<u8>,
    vault: Option<Vec<u8>>,
    target_key: Option<Vec<u8>>,
    source_database: String,
    target_database: String,
    upstream: i32,
    loongport: i32,
    source_skills: SkillImage,
    target_skills: SkillImage,
    artifacts: Vec<Artifact>,
    skills: SkillsTree,
}
impl Drop for ContentManifest {
    fn drop(&mut self) {
        if let Some(key) = self.target_key.as_mut() {
            key.zeroize();
        }
    }
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillImage {
    present: bool,
    directories: Vec<String>,
    files: BTreeMap<String, String>,
}
fn skill_image(path: &Path) -> Result<SkillImage, AppError> {
    let present = directory(path)?;
    let mut files = BTreeMap::new();
    let directories = if present {
        visit_skills(path, &mut |relative, bytes| {
            if files.insert(relative, hash(&bytes)).is_some() {
                return Err(invalid());
            }
            Ok(())
        })?
    } else {
        Vec::new()
    };
    Ok(SkillImage {
        present,
        directories,
        files,
    })
}
fn validate_metadata(root: &Path, manifest: &ContentManifest) -> Result<StorageContext, AppError> {
    if root != manifest.root || skills_destination(root, &manifest.skills) != manifest.skills_root {
        return Err(invalid());
    }
    crate::config_file_io::directory_exists_no_alias(root)?;
    let (record, record_bytes) = protection::read_with_bytes(root)?.ok_or_else(invalid)?;
    let local = session::read_metadata_with_bytes(root)?;
    if record_bytes != manifest.protection
        || local.as_ref().map(|(_, bytes)| bytes) != manifest.vault.as_ref()
    {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    let context = protection::open_context(
        &record,
        local.as_ref().map(|(saved, _)| &saved.metadata),
        manifest
            .target_key
            .as_ref()
            .map(|key| Zeroizing::new(key.clone())),
        |_, _| Err(AppError::Config("secret.invalid_transition".into())),
    )?;
    protection::validate_runtime_storage(root, &context)?;
    Ok(context)
}
fn database_image(
    bytes: &[u8],
    context: &StorageContext,
    manifest: &ContentManifest,
) -> Result<Connection, AppError> {
    let mut conn = Connection::open_in_memory().map_err(db_error)?;
    conn.deserialize_read_exact(rusqlite::MAIN_DB, bytes, bytes.len(), false)
        .map_err(db_error)?;
    let integrity: String = conn
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .map_err(db_error)?;
    if integrity != "ok"
        || Database::get_user_version(&conn)? != manifest.upstream
        || crate::database::loongport_schema::read_stored_version(&conn)? != manifest.loongport
    {
        return Err(invalid());
    }
    crate::database::storage::check_identity(&conn, context)?;
    inventory::validate_storage_database(&conn, context)?;
    if Database::content_digest(&conn)? != manifest.target_database {
        return Err(invalid());
    }
    Ok(conn)
}
fn staged_database(
    root: &Path,
    intent: &ContentIntent,
    manifest: &ContentManifest,
    auth: &VaultContext,
    context: &StorageContext,
) -> Result<Connection, AppError> {
    if manifest
        .artifacts
        .iter()
        .filter(|a| matches!(a.destination, Destination::Database))
        .count()
        != 1
        || manifest.artifacts.iter().any(|a| {
            !matches!(
                a.destination,
                Destination::Database | Destination::SkillFile { .. }
            )
        })
    {
        return Err(invalid());
    }
    let mut target_files = BTreeMap::new();
    let mut database = None;
    for (index, artifact) in manifest.artifacts.iter().enumerate() {
        device::validate_digest(&artifact.digest)?;
        let bytes = read_stage(root, &intent.id, index, artifact)?;
        match &artifact.destination {
            Destination::Database => {
                let raw = auth
                    .open(
                        &["local", "transition-content-db", &intent.id],
                        std::str::from_utf8(&bytes).map_err(|_| invalid())?,
                    )
                    .map_err(inventory::secret_error)?;
                database = Some(database_image(&raw, context, manifest)?);
            }
            Destination::SkillFile { relative } => {
                safe_relative(relative)?;
                let raw = auth
                    .open(
                        &["local", "transition-skill", &intent.id, relative],
                        std::str::from_utf8(&bytes).map_err(|_| invalid())?,
                    )
                    .map_err(inventory::secret_error)?;
                if target_files.insert(relative.clone(), hash(&raw)).is_some() {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
    }
    if !manifest.target_skills.present
        || target_files != manifest.target_skills.files
        || manifest.skills.directories != manifest.target_skills.directories
    {
        return Err(invalid());
    }
    for path in &manifest.skills.directories {
        safe_relative(path)?;
    }
    database.ok_or_else(invalid)
}
fn validate_source_skills(
    root: &Path,
    intent: &ContentIntent,
    manifest: &ContentManifest,
) -> Result<(), AppError> {
    validate_skills_paths(root, &intent.id, &manifest.skills)?;
    let actual = skill_image(&skills_destination(root, &manifest.skills))?;
    let previous_path = skills_staging(root, &intent.id, &manifest.skills)?.join("previous");
    let previous = skill_image(&previous_path)?;
    if previous.present && previous != manifest.source_skills {
        return Err(invalid());
    }
    if (actual.present || previous != manifest.source_skills)
        && actual != manifest.target_skills
        && actual != manifest.source_skills
    {
        return Err(AppError::Config("secret.source_changed".into()));
    }
    Ok(())
}
fn verify_database(
    conn: &Connection,
    context: &StorageContext,
    manifest: &ContentManifest,
) -> Result<(), AppError> {
    crate::database::storage::check_identity(conn, context)?;
    inventory::validate_storage_database(conn, context)?;
    if Database::get_user_version(conn)? != manifest.upstream
        || crate::database::loongport_schema::read_stored_version(conn)? != manifest.loongport
    {
        return Err(invalid());
    }
    let digest = Database::content_digest(conn)?;
    if digest != manifest.source_database && digest != manifest.target_database {
        return Err(AppError::Config("secret.source_changed".into()));
    }
    Ok(())
}
fn finish(
    root: &Path,
    intent: &ContentIntent,
    manifest: &ContentManifest,
    auth: &VaultContext,
    active: Option<&mut Connection>,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<StorageContext, AppError> {
    let context = validate_metadata(root, manifest)?;
    let target = staged_database(root, intent, manifest, auth, &context)?;
    validate_source_skills(root, intent, manifest)?;
    let main = root.join(crate::config::DB_FILE_NAME);
    let captured = crate::database::inspection::capture(&main)?.ok_or_else(invalid)?;
    verify_database(&captured.image, &context, manifest)?;
    crate::database::inspection::verify_unchanged(&main, &captured.revision)?;
    let mut opened;
    let conn = if let Some(conn) = active {
        conn
    } else {
        opened = Connection::open_with_flags(
            &main,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(db_error)?;
        &mut opened
    };
    crate::database::inspection::verify_connection_primary_identity(
        conn,
        &captured.revision.primary_identity()?,
    )?;
    verify_database(conn, &context, manifest)?;
    prepare_skills_install(
        root,
        &intent.id,
        auth,
        &manifest.artifacts,
        &manifest.skills,
    )?;
    validate_metadata(root, manifest)?;
    validate_source_skills(root, intent, manifest)?;
    if Database::content_digest(conn)? != manifest.target_database {
        conn.execute_batch("PRAGMA secure_delete=ON;")
            .map_err(db_error)?;
        vault::copy(&target, conn)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE; VACUUM;")
            .map_err(db_error)?;
        crate::config::ensure_private_file(&main)?;
        crate::config::sync_private_file(&main)?;
        sync_directory(root)?;
    }
    hook(Checkpoint::Database)?;
    validate_source_skills(root, intent, manifest)?;
    commit_skills(root, &intent.id, &manifest.skills)?;
    // Reuses the original post-publication fault boundary; no metadata is written.
    hook(Checkpoint::Metadata)?;
    validate_metadata(root, manifest)?;
    let installed = crate::database::inspection::capture(&main)?.ok_or_else(invalid)?;
    verify_database(&installed.image, &context, manifest)?;
    if Database::content_digest(&installed.image)? != manifest.target_database
        || skill_image(&skills_destination(root, &manifest.skills))? != manifest.target_skills
    {
        return Err(invalid());
    }
    crate::database::inspection::verify_unchanged(&main, &installed.revision)?;
    remove_durable(&root.join(INTENT))?;
    let _ = std::fs::remove_dir_all(skills_staging(root, &intent.id, &manifest.skills)?);
    let _ = std::fs::remove_dir_all(stage_root(root, &intent.id)?);
    Ok(context)
}
pub(crate) fn install_sync_content(
    db: &Database,
    incoming: ValidatedSyncSnapshot,
    auth: &VaultContext,
    skills: SkillsReplacement,
) -> Result<(), AppError> {
    install_with_hook(db, incoming, auth, skills, &mut |_| Ok(()))
}
pub(super) fn install_with_hook(
    db: &Database,
    incoming: ValidatedSyncSnapshot,
    auth: &VaultContext,
    skills: SkillsReplacement,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let session = &db.secrets;
    let current = session.write()?;
    let root = session.root();
    protection::validate_runtime_storage(root, &current)?;
    if pending_record(root)?.is_some()
        || super::super::reset::pending_record(root)?.is_some()
        || super::super::bootstrap_restore::pending_record(root)?.is_some()
        || super::super::rewrap::pending_record(root)?.is_some()
    {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let device = crate::live::engine::DeviceStore::for_device();
    let roots = Roots {
        data: root,
        device: device.root(),
    };
    roots.validate()?;
    device::ensure_no_pending_mode_operation_with_storage(roots, &current)?;
    super::super::owned_file::ensure_no_pending_zcode_transaction(root)?;
    super::super::upgrade::checkpoint::ensure_no_pending_checkpoint(&device)?;
    let mut conn = db.conn.lock().map_err(|_| invalid())?;
    let memory = db.prepare_sync_join_for_receiver(incoming, &conn, &current)?;
    memory
        .execute_batch("PRAGMA secure_delete=ON; VACUUM;")
        .map_err(db_error)?;
    let (_, protection_bytes) = protection::read_with_bytes(root)?.ok_or_else(invalid)?;
    let vault_bytes = session::read_metadata_with_bytes(root)?.map(|(_, bytes)| bytes);
    let id = uuid::Uuid::new_v4().to_string();
    let staging = stage_root(root, &id)?;
    // Ordinary creation does not reuse a competing pending transaction.
    std::fs::create_dir(&staging).map_err(|e| AppError::io(&staging, e))?;
    crate::config::ensure_private_directory(&staging)?;
    let capture = (|| {
        let source_database = Database::content_digest(&conn)?;
        let target_database = Database::content_digest(&memory)?;
        let target_skills = skill_image(&skills.source)?;
        let mut artifacts = Vec::new();
        let bytes = memory.serialize(rusqlite::MAIN_DB).map_err(db_error)?;
        let sealed = auth
            .seal(&["local", "transition-content-db", &id], &bytes)
            .map_err(inventory::secret_error)?;
        stage_artifact(
            root,
            &id,
            &mut artifacts,
            Destination::Database,
            sealed.as_bytes(),
        )?;
        let skills = capture_skill_artifacts(root, &id, auth, &mut artifacts, &skills)?;
        validate_skills_paths(root, &id, &skills)?;
        let skills_root = skills_destination(root, &skills);
        let source_skills = skill_image(&skills_root)?;
        let manifest = ContentManifest {
            root: root.to_path_buf(),
            skills_root,
            protection: protection_bytes,
            vault: vault_bytes,
            target_key: match &*current {
                StorageContext::Encrypted(v) => Some(v.export_key().to_vec()),
                StorageContext::Plaintext(_) => None,
            },
            source_database,
            target_database,
            upstream: Database::get_user_version(&memory)?,
            loongport: crate::database::loongport_schema::read_stored_version(&memory)?,
            source_skills,
            target_skills,
            artifacts,
            skills,
        };
        let raw = Zeroizing::new(serde_json::to_vec(&manifest).map_err(|_| invalid())?);
        let intent = ContentIntent {
            version: CONTENT_FORMAT,
            kind: CONTENT_KIND.into(),
            id: id.clone(),
            authentication: auth.metadata().clone(),
            manifest: auth
                .seal(&["local", "content-transition", &id], &raw)
                .map_err(inventory::secret_error)?,
        };
        validate_metadata(root, &manifest)?;
        staged_database(root, &intent, &manifest, auth, &current)?;
        validate_source_skills(root, &intent, &manifest)?;
        hook(Checkpoint::Staged)?;
        if Database::content_digest(&conn)? != manifest.source_database {
            return Err(invalid());
        }
        Ok((intent, manifest))
    })();
    let (intent, manifest) = match capture {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };
    let bytes = serde_json::to_vec(&intent).map_err(|_| invalid())?;
    session.set_blocked(true);
    write_durable(&root.join(INTENT), &bytes)?;
    hook(Checkpoint::Intent)?;
    db.invalidate_pricing_observations()?;
    finish(root, &intent, &manifest, auth, Some(&mut conn), hook)?;
    session.set_blocked(false);
    Ok(())
}
/// The discriminator is checked before legacy-generation admission; unknown kinds never fall back.
pub(super) fn is_content(bytes: &[u8]) -> Result<bool, AppError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if value.get("version").and_then(|v| v.as_u64()) == Some(CONTENT_FORMAT as u64)
        || value.get("kind").is_some()
    {
        if value.get("version").and_then(|v| v.as_u64()) != Some(CONTENT_FORMAT as u64)
            || value.get("kind").and_then(|v| v.as_str()) != Some(CONTENT_KIND)
        {
            return Err(invalid());
        }
        Ok(true)
    } else {
        Ok(false)
    }
}
pub(super) fn recover(
    root: &Path,
    bytes: &[u8],
    input: Option<&str>,
) -> Result<StorageContext, AppError> {
    let intent: ContentIntent = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    if intent.version != CONTENT_FORMAT || intent.kind != CONTENT_KIND {
        return Err(invalid());
    }
    stage_root(root, &intent.id)?;
    let input = input.ok_or_else(|| AppError::Config("secret.password_required".into()))?;
    let auth = VaultContext::from_recovery_input(intent.authentication.clone(), input)
        .map_err(inventory::secret_error)?;
    let raw = auth
        .open(
            &["local", "content-transition", &intent.id],
            &intent.manifest,
        )
        .map_err(inventory::secret_error)?;
    let manifest: ContentManifest = serde_json::from_slice(&raw).map_err(|_| invalid())?;
    if pending_record(root)?.as_deref() != Some(bytes) {
        return Err(invalid());
    }
    finish(root, &intent, &manifest, &auth, None, &mut |_| Ok(()))
}
