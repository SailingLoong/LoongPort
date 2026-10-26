//! Same-storage DB + skills installation, owned by the original transition journal.
use super::*;
use crate::database::backup::ValidatedSyncSnapshot;
use crate::secrets::{
    protection, session,
    storage::{PlaintextIdentity, StorageContext, StorageIdentity},
    VaultMetadata,
};
use std::collections::BTreeMap;

const CONTENT_FORMAT: u32 = 3;
const STORAGE_CONTENT_FORMAT: u32 = 4;
const CONTENT_KIND: &str = "sameStorageContent";
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContentIntent {
    version: u32,
    kind: String,
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authentication: Option<VaultMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    storage: Option<ContentStorage>,
    manifest: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
enum ContentStorage {
    Current { metadata: Box<VaultMetadata> },
    Plaintext { identity: PlaintextIdentity },
}

/// The public input requirement has one native owner. No-input plaintext is a
/// consistency-only operation; it is never presented as source authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RecoveryInputKind {
    None,
    Source,
    Current,
}
impl RecoveryInputKind {
    pub(crate) fn required(self) -> bool {
        self != Self::None
    }
}

enum ContentCodec {
    Source(VaultContext),
    Current(VaultContext),
    Plaintext(PlaintextIdentity),
}
impl ContentCodec {
    fn for_install(
        root: &Path,
        source: &StorageContext,
        current: &StorageContext,
    ) -> Result<Self, AppError> {
        match source {
            StorageContext::Encrypted(vault) => Ok(Self::Source(vault.clone())),
            StorageContext::Plaintext(identity) => {
                identity.validate().map_err(inventory::secret_error)?;
                match current {
                    StorageContext::Encrypted(vault) => {
                        if !vault
                            .metadata()
                            .recovery_input_available()
                            .map_err(inventory::secret_error)?
                            && !session::read_metadata(root)?.automatic_unlock
                        {
                            return Err(AppError::Config("secret.locked".into()));
                        }
                        Ok(Self::Current(vault.clone()))
                    }
                    StorageContext::Plaintext(identity) => Ok(Self::Plaintext(identity.clone())),
                }
            }
        }
    }
    fn auth(&self) -> Option<&VaultContext> {
        match self {
            Self::Source(v) | Self::Current(v) => Some(v),
            Self::Plaintext(_) => None,
        }
    }
    fn manifest_identity<'a>(&self, id: &'a str) -> [&'a str; 3] {
        match self {
            Self::Source(_) => ["local", "content-transition", id],
            Self::Current(_) => ["local", "content-transition-current/v4", id],
            Self::Plaintext(_) => ["local", "content-transition-plaintext/v4", id],
        }
    }
    fn intent(&self, id: String, raw: &[u8]) -> Result<ContentIntent, AppError> {
        let manifest = seal_artifact_bytes(self.auth(), &self.manifest_identity(&id), raw)?;
        let (version, authentication, storage) = match self {
            Self::Source(v) => (CONTENT_FORMAT, Some(v.metadata().clone()), None),
            Self::Current(v) => (
                STORAGE_CONTENT_FORMAT,
                None,
                Some(ContentStorage::Current {
                    metadata: v.metadata().clone().into(),
                }),
            ),
            Self::Plaintext(identity) => (
                STORAGE_CONTENT_FORMAT,
                None,
                Some(ContentStorage::Plaintext {
                    identity: identity.clone(),
                }),
            ),
        };
        Ok(ContentIntent {
            version,
            kind: CONTENT_KIND.into(),
            id,
            authentication,
            storage,
            manifest: String::from_utf8(manifest).map_err(|_| invalid())?,
        })
    }
    fn manifest(&self, intent: &ContentIntent) -> Result<ContentManifest, AppError> {
        let raw = open_artifact_bytes(
            self.auth(),
            &self.manifest_identity(&intent.id),
            intent.manifest.as_bytes(),
        )?;
        serde_json::from_slice(&raw).map_err(|_| invalid())
    }
}
impl ContentIntent {
    fn checked_storage(&self) -> Result<Option<&ContentStorage>, AppError> {
        if self.kind != CONTENT_KIND {
            return Err(invalid());
        }
        match (self.version, &self.authentication, &self.storage) {
            (CONTENT_FORMAT, Some(metadata), None) => {
                metadata.validate().map_err(inventory::secret_error)?;
                Ok(None)
            }
            (STORAGE_CONTENT_FORMAT, None, Some(storage)) => {
                match storage {
                    ContentStorage::Current { metadata } => {
                        metadata.validate().map_err(inventory::secret_error)?
                    }
                    ContentStorage::Plaintext { identity } => {
                        identity.validate().map_err(inventory::secret_error)?
                    }
                }
                Ok(Some(storage))
            }
            _ => Err(invalid()),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContentManifest {
    root: PathBuf,
    skills_root: PathBuf,
    protection: Option<Vec<u8>>,
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
fn validate_metadata(
    root: &Path,
    manifest: &ContentManifest,
    codec: &ContentCodec,
) -> Result<StorageContext, AppError> {
    if root != manifest.root || skills_destination(root, &manifest.skills) != manifest.skills_root {
        return Err(invalid());
    }
    crate::config_file_io::directory_exists_no_alias(root)?;
    let record = protection::read_with_bytes(root)?;
    let local = session::read_metadata_with_bytes(root)?;
    if record.as_ref().map(|(_, bytes)| bytes) != manifest.protection.as_ref()
        || local.as_ref().map(|(_, bytes)| bytes) != manifest.vault.as_ref()
    {
        return Err(AppError::Config("secret.identity_mismatch".into()));
    }
    // Only an authenticated SOURCE envelope can carry a different target key.
    // Current-key/plaintext formats must never smuggle a new key or downgrade.
    if !matches!(codec, ContentCodec::Source(_)) && manifest.target_key.is_some() {
        return Err(invalid());
    }
    let key = match codec {
        ContentCodec::Source(_) => manifest
            .target_key
            .as_ref()
            .map(|key| Zeroizing::new(key.clone())),
        ContentCodec::Current(auth) => Some(auth.export_key()),
        ContentCodec::Plaintext(_) => None,
    };
    let context = if let Some((record, _)) = &record {
        protection::open_context(
            record,
            local.as_ref().map(|(saved, _)| &saved.metadata),
            key,
            |_, _| Err(AppError::Config("secret.invalid_transition".into())),
        )?
    } else if let ContentCodec::Current(auth) = codec {
        // No record is not a plaintext choice. This only retains a genuine,
        // authenticated legacy encrypted owner; user-text requires its record.
        if local.as_ref().map(|(saved, _)| &saved.metadata) != Some(auth.metadata())
            || auth.metadata().user_text_key.is_some()
        {
            return Err(invalid());
        }
        StorageContext::Encrypted(auth.clone())
    } else {
        return Err(invalid());
    };
    protection::validate_runtime_storage(root, &context)?;
    match codec {
        ContentCodec::Current(auth)
            if context.identity() != StorageIdentity::Encrypted(auth.metadata().clone().into()) =>
        {
            return Err(invalid())
        }
        ContentCodec::Plaintext(identity)
            if context.identity() != StorageIdentity::Plaintext(identity.clone())
                || manifest.vault.is_some() =>
        {
            return Err(invalid())
        }
        _ => {}
    }
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
    codec: &ContentCodec,
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
                let raw = open_artifact_bytes(
                    codec.auth(),
                    &["local", "transition-content-db", &intent.id],
                    &bytes,
                )?;
                database = Some(database_image(&raw, context, manifest)?);
            }
            Destination::SkillFile { relative } => {
                safe_relative(relative)?;
                let raw = open_artifact_bytes(
                    codec.auth(),
                    &["local", "transition-skill", &intent.id, relative],
                    &bytes,
                )?;
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
    codec: &ContentCodec,
    active: Option<&mut Connection>,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<StorageContext, AppError> {
    let context = validate_metadata(root, manifest, codec)?;
    let target = staged_database(root, intent, manifest, codec, &context)?;
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
    prepare_skills_install_with_codec(
        root,
        &intent.id,
        codec.auth(),
        &manifest.artifacts,
        &manifest.skills,
    )?;
    validate_metadata(root, manifest, codec)?;
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
    validate_metadata(root, manifest, codec)?;
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
    source: &StorageContext,
    skills: SkillsReplacement,
) -> Result<(), AppError> {
    install_storage_with_hook(db, incoming, source, skills, &mut |_| Ok(()))
}
#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn install_with_hook(
    db: &Database,
    incoming: ValidatedSyncSnapshot,
    auth: &VaultContext,
    skills: SkillsReplacement,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    install_storage_with_hook(
        db,
        incoming,
        &StorageContext::from(auth.clone()),
        skills,
        hook,
    )
}
pub(super) fn install_storage_with_hook(
    db: &Database,
    incoming: ValidatedSyncSnapshot,
    source: &StorageContext,
    skills: SkillsReplacement,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let session = &db.secrets;
    let current = session.write()?;
    let root = session.root();
    protection::validate_runtime_storage(root, &current)?;
    let codec = ContentCodec::for_install(root, source, &current)?;
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
    let protection_bytes = protection::read_with_bytes(root)?.map(|(_, bytes)| bytes);
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
        let sealed = seal_artifact_bytes(
            codec.auth(),
            &["local", "transition-content-db", &id],
            &bytes,
        )?;
        stage_artifact(root, &id, &mut artifacts, Destination::Database, &sealed)?;
        let skills =
            capture_skill_artifacts_with_codec(root, &id, codec.auth(), &mut artifacts, &skills)?;
        validate_skills_paths(root, &id, &skills)?;
        let skills_root = skills_destination(root, &skills);
        let source_skills = skill_image(&skills_root)?;
        let manifest = ContentManifest {
            root: root.to_path_buf(),
            skills_root,
            protection: protection_bytes,
            vault: vault_bytes,
            target_key: match (&codec, &*current) {
                (ContentCodec::Source(_), StorageContext::Encrypted(v)) => {
                    Some(v.export_key().to_vec())
                }
                _ => None,
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
        let intent = codec.intent(id.clone(), &raw)?;
        validate_metadata(root, &manifest, &codec)?;
        staged_database(root, &intent, &manifest, &codec, &current)?;
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
    finish(root, &intent, &manifest, &codec, Some(&mut conn), hook)?;
    session.set_blocked(false);
    Ok(())
}
/// The discriminator is checked before legacy-generation admission; unknown kinds never fall back.
pub(super) fn is_content(bytes: &[u8]) -> Result<bool, AppError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let version = value.get("version").and_then(|v| v.as_u64());
    if matches!(version, Some(3 | 4)) || value.get("kind").is_some() {
        let intent: ContentIntent = serde_json::from_value(value).map_err(|_| invalid())?;
        intent.checked_storage()?;
        Ok(true)
    } else {
        Ok(false)
    }
}
pub(crate) fn recovery_input_kind(
    root: &Path,
    bytes: &[u8],
) -> Result<RecoveryInputKind, AppError> {
    let intent: ContentIntent = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    stage_root(root, &intent.id)?;
    match intent.checked_storage()? {
        None => Ok(RecoveryInputKind::Source),
        Some(ContentStorage::Current { metadata }) => {
            // Preview may describe an input only for the actual current owner.
            // This is passive public-identity admission, not key authentication.
            let local = session::read_metadata(root)?;
            if &local.metadata != metadata.as_ref() {
                return Err(invalid());
            }
            match protection::read(root)? {
                Some(record) => {
                    record.validate_vault(Some(metadata.as_ref()))?;
                    if record.cleanup.is_some() {
                        return Err(invalid());
                    }
                }
                None if metadata.user_text_key.is_some() => {
                    return Err(AppError::Config("protection.record_missing".into()))
                }
                None => {}
            }
            if metadata
                .recovery_input_available()
                .map_err(inventory::secret_error)?
            {
                Ok(RecoveryInputKind::Current)
            } else {
                if !local.automatic_unlock {
                    return Err(invalid());
                }
                Ok(RecoveryInputKind::None)
            }
        }
        Some(ContentStorage::Plaintext { identity }) => {
            let codec = ContentCodec::Plaintext(identity.clone());
            let manifest = codec.manifest(&intent)?;
            validate_metadata(root, &manifest, &codec)?;
            Ok(RecoveryInputKind::None)
        }
    }
}
pub(super) fn recover(
    root: &Path,
    bytes: &[u8],
    store: &dyn KeyStore,
    input: Option<&str>,
) -> Result<StorageContext, AppError> {
    let intent: ContentIntent = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let kind = recovery_input_kind(root, bytes)?;
    if !kind.required() && input.is_some_and(|input| !input.is_empty()) {
        return Err(AppError::Config("secret.input_not_required".into()));
    }
    let supplied = || {
        input
            .filter(|input| !input.is_empty())
            .ok_or_else(|| AppError::Config("secret.password_required".into()))
    };
    let codec = match intent.checked_storage()? {
        None => ContentCodec::Source(
            VaultContext::from_recovery_input(
                intent.authentication.clone().ok_or_else(invalid)?,
                supplied()?,
            )
            .map_err(inventory::secret_error)?,
        ),
        Some(ContentStorage::Current { metadata }) => {
            let auth = if kind.required() {
                VaultContext::from_recovery_input(metadata.as_ref().clone(), supplied()?)
                    .map_err(inventory::secret_error)?
            } else {
                session::authenticate_existing(root, store, None)?
            };
            if auth.metadata() != metadata.as_ref() {
                return Err(invalid());
            }
            ContentCodec::Current(auth)
        }
        Some(ContentStorage::Plaintext { identity }) => ContentCodec::Plaintext(identity.clone()),
    };
    let manifest = codec.manifest(&intent)?;
    if pending_record(root)?.as_deref() != Some(bytes) {
        return Err(invalid());
    }
    finish(root, &intent, &manifest, &codec, None, &mut |_| Ok(()))
}
