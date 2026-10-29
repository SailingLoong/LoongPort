//! Remote recovery before ordinary application state is available.
use super::{
    inventory,
    key_store::{save_verified, KeyStore},
    session::{self, SecretSession},
    transition, VaultContext, VaultMetadata,
};
use crate::{
    database::{backup::ValidatedSyncSnapshot, vault, Database},
    error::AppError,
    services::{
        sync_protocol::{self, DownloadedSnapshot},
        webdav_sync::archive::{stage_skills_zip, PreparedSkills},
    },
    settings::{AppSettings, S3SyncSettings, WebDavSyncSettings},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

const INTENT: &str = ".vault-bootstrap";
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "transport",
    content = "settings",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum RestoreSource {
    Webdav(WebDavSyncSettings),
    S3(S3SyncSettings),
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestorePreview {
    pub snapshot_id: String,
    pub device_name: String,
    pub created_at: String,
}
pub(crate) struct PreparedRestore {
    data: ValidatedRestore,
    source: RestoreSource,
}
struct ValidatedRestore {
    snapshot: DownloadedSnapshot,
    next: VaultContext,
    incoming: ValidatedSyncSnapshot,
    skills: PreparedSkills,
}
/// Borrowed authority minted only by the authenticated original bootstrap owner.
/// This preserves its exact live intent through the shared legacy installer.
pub(crate) struct BootstrapInstallPermit {
    root: PathBuf,
    metadata: VaultMetadata,
    expected: Vec<u8>,
}
impl BootstrapInstallPermit {
    fn for_intent(
        root: &Path,
        intent: &Intent,
        current: &VaultContext,
        expected: &[u8],
    ) -> Result<Self, AppError> {
        let observed: Intent = serde_json::from_slice(expected).map_err(|_| invalid())?;
        if serde_json::to_value(&observed).map_err(|_| invalid())?
            != serde_json::to_value(intent).map_err(|_| invalid())?
            || current.metadata() != &intent.metadata
        {
            return Err(invalid());
        }
        Ok(Self {
            root: root.to_owned(),
            metadata: intent.metadata.clone(),
            expected: expected.to_vec(),
        })
    }
    pub(super) fn verify(&self, root: &Path, current: &VaultContext) -> Result<(), AppError> {
        if root != self.root
            || current.metadata() != &self.metadata
            || pending_record(root)?.as_deref() != Some(self.expected.as_slice())
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    id: String,
    metadata: VaultMetadata,
    body: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    snapshot: sync_protocol::SyncManifest,
    automatic_unlock: bool,
    original_settings_hash: String,
    settings_hash: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    Published,
    Database,
    Metadata,
    Installed,
}
fn invalid() -> AppError {
    AppError::Config("secret.invalid_bootstrap_restore".into())
}
fn regular(path: &Path) -> Result<bool, AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(v) if v.is_file() && !v.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(path, e)),
    }
}
fn directory(path: &Path) -> Result<bool, AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(v) if v.is_dir() && !v.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(path, e)),
    }
}
fn optional(path: &Path) -> Result<Vec<u8>, AppError> {
    if regular(path)? {
        std::fs::read(path).map_err(|e| AppError::io(path, e))
    } else {
        Ok(Vec::new())
    }
}
#[cfg_attr(not(unix), allow(unused_variables))]
fn sync_dir(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|v| v.sync_all())
        .map_err(|e| AppError::io(path, e))?;
    Ok(())
}
fn staging(root: &Path, id: &str) -> Result<PathBuf, AppError> {
    if uuid::Uuid::parse_str(id)
        .map(|v| v.to_string())
        .ok()
        .as_deref()
        != Some(id)
    {
        return Err(invalid());
    }
    Ok(root.join(format!(".vault-bootstrap-{id}")))
}
pub(crate) fn pending(root: &Path) -> Result<bool, AppError> {
    Ok(regular(&root.join(INTENT))? || regular(&root.join(transition::INTENT))?)
}

async fn fetch(source: &RestoreSource) -> Result<DownloadedSnapshot, AppError> {
    // Startup explicitly selects direct transport before any settings vault exists.
    crate::proxy::http_client::init(None)
        .map_err(|_| AppError::Config("secret.transport_unavailable".into()))?;
    match source {
        RestoreSource::Webdav(settings) => {
            crate::services::webdav_sync::fetch_snapshot_for_startup(settings).await
        }
        RestoreSource::S3(settings) => {
            crate::services::s3_sync::fetch_snapshot_for_startup(settings).await
        }
    }
}
pub(crate) async fn preview(source: &RestoreSource) -> Result<RestorePreview, AppError> {
    let snapshot = fetch(source).await?;
    Ok(RestorePreview {
        snapshot_id: snapshot.manifest.snapshot_id,
        device_name: snapshot.manifest.device_name,
        created_at: snapshot.manifest.created_at,
    })
}
/// The startup coordinator owns the shared sync mutex across prepare and finalize.
pub(crate) async fn prepare(
    source: RestoreSource,
    password: &str,
    expected: &str,
) -> Result<PreparedRestore, AppError> {
    let snapshot = fetch(&source).await?;
    let password = Zeroizing::new(password.to_owned());
    let expected = expected.to_owned();
    tokio::task::spawn_blocking(move || prepare_snapshot(snapshot, source, &password, &expected))
        .await
        .map_err(|_| AppError::Config("secret.operation_failed".into()))?
}
fn prepare_snapshot(
    snapshot: DownloadedSnapshot,
    source: RestoreSource,
    password: &str,
    expected: &str,
) -> Result<PreparedRestore, AppError> {
    sync_protocol::validate_manifest_compat(&snapshot.manifest, snapshot.layout)?;
    if snapshot.manifest.snapshot_id != expected {
        return Err(sync_protocol::publication_conflict());
    }
    let next = VaultContext::from_password(snapshot.manifest.vault_metadata()?.clone(), password)
        .map_err(inventory::secret_error)?;
    Ok(PreparedRestore {
        data: validate(snapshot, next)?,
        source,
    })
}
fn validate(
    snapshot: DownloadedSnapshot,
    next: VaultContext,
) -> Result<ValidatedRestore, AppError> {
    sync_protocol::validate_manifest_compat(&snapshot.manifest, snapshot.layout)?;
    for (name, bytes) in [
        (sync_protocol::REMOTE_DB_SQL, &snapshot.db_sql),
        (sync_protocol::REMOTE_SKILLS_ZIP, &snapshot.skills_zip),
    ] {
        sync_protocol::snapshot_artifact_path(&snapshot.manifest, name)?;
        let meta = snapshot.manifest.artifacts.get(name).ok_or_else(invalid)?;
        sync_protocol::verify_artifact(bytes, name, meta)?;
    }
    let sql = std::str::from_utf8(&snapshot.db_sql).map_err(|_| invalid())?;
    let incoming =
        Database::validate_sync_snapshot(sql, snapshot.manifest.vault_metadata()?, &next)?;
    let skills = stage_skills_zip(&snapshot.skills_zip)?;
    Ok(ValidatedRestore {
        snapshot,
        next,
        incoming,
        skills,
    })
}
fn read_settings(context: &VaultContext) -> Result<(AppSettings, Vec<u8>), AppError> {
    let raw = optional(&crate::settings::settings_path())?;
    let settings = if raw.is_empty() {
        AppSettings::default()
    } else {
        crate::settings::decode_settings_with_vault(&raw, context)?
    };
    Ok((settings, raw))
}
fn apply_source(settings: &mut AppSettings, source: RestoreSource, snapshot: &DownloadedSnapshot) {
    match source {
        RestoreSource::Webdav(mut value) => {
            value.status.last_remote_manifest_hash = Some(snapshot.manifest_hash.clone());
            value.status.last_remote_etag = snapshot.etag.clone();
            settings.webdav_sync = Some(value)
        }
        RestoreSource::S3(mut value) => {
            value.status.last_remote_manifest_hash = Some(snapshot.manifest_hash.clone());
            value.status.last_remote_etag = snapshot.etag.clone();
            settings.s3_sync = Some(value)
        }
    }
}
fn ensure_empty(root: &Path, next: &VaultContext) -> Result<(), AppError> {
    directory(root)?;
    for name in [
        "vault.json",
        crate::config::DB_FILE_NAME,
        ".vault-rewrap",
        transition::INTENT,
    ] {
        if regular(&root.join(name))? {
            return Err(AppError::Config("secret.local_credentials_exist".into()));
        }
    }
    if !super::files::stage_owned_files_with_vault(root, next, false)?.is_empty() {
        return Err(AppError::Config("secret.local_credentials_exist".into()));
    }
    let backups = root.join("backups");
    if directory(&backups)? {
        for entry in std::fs::read_dir(&backups).map_err(|e| AppError::io(&backups, e))? {
            let path = entry.map_err(|e| AppError::io(&backups, e))?.path();
            if path.extension().is_some_and(|e| e == "db") {
                return Err(AppError::Config("secret.local_credentials_exist".into()));
            }
        }
    }
    if super::reset::pending(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    Ok(())
}
/// The caller holds the sync mutex; all network/password/SQL/ZIP validation is complete.
pub(crate) fn finalize(
    root: &Path,
    prepared: PreparedRestore,
    store: &dyn KeyStore,
    automatic_unlock: bool,
) -> Result<Arc<SecretSession>, AppError> {
    finalize_with_hook(root, prepared, store, automatic_unlock, &mut |_| Ok(()))
}
fn finalize_with_hook(
    root: &Path,
    prepared: PreparedRestore,
    store: &dyn KeyStore,
    automatic_unlock: bool,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<Arc<SecretSession>, AppError> {
    transition::ensure_legacy_generation_owner(root)?;
    transition::ensure_legacy_metadata(prepared.data.next.metadata())?;
    super::owned_file::ensure_no_pending_zcode_transaction(root)?;
    directory(root)?;
    if pending(root)? || super::reset::pending(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let _skills = crate::services::skill::skill_state_write_guard();
    let PreparedRestore { data, source } = prepared;
    let existing = if regular(&root.join("vault.json"))? {
        let saved = session::read_metadata(root)?;
        transition::ensure_legacy_metadata(&saved.metadata)?;
        Some(
            VaultContext::from_key(saved.metadata, data.next.export_key())
                .map_err(|_| AppError::Config("secret.source_key_required".into()))?,
        )
    } else {
        ensure_empty(root, &data.next)?;
        None
    };
    let (mut settings, original) = read_settings(existing.as_ref().unwrap_or(&data.next))?;
    apply_source(&mut settings, source, &data.snapshot);
    if let Some(current) = existing {
        return install(root, data, settings, current, store, automatic_unlock, None);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let stage = staging(root, &id)?;
    crate::config::ensure_private_directory(root)?;
    crate::config::ensure_private_directory(&stage)?;
    let encoded = crate::settings::encode_settings_with_vault(&settings, &data.next)?;
    for (name, bytes) in [
        ("database", data.snapshot.db_sql.as_slice()),
        ("skills", data.snapshot.skills_zip.as_slice()),
        ("settings", encoded.as_slice()),
    ] {
        let sealed = data
            .next
            .seal(&["local", "bootstrap-artifact", &id, name], bytes)
            .map_err(inventory::secret_error)?;
        session::write_durable(&stage.join(name), sealed.as_bytes())?;
    }
    let manifest = Manifest {
        snapshot: data.snapshot.manifest.clone(),
        automatic_unlock,
        original_settings_hash: sync_protocol::sha256_hex(&original),
        settings_hash: sync_protocol::sha256_hex(&encoded),
    };
    let intent = Intent {
        version: 1,
        id,
        metadata: data.next.metadata().clone(),
        body: data
            .next
            .seal(
                &["local", "bootstrap-intent"],
                &serde_json::to_vec(&manifest).map_err(|_| invalid())?,
            )
            .map_err(inventory::secret_error)?,
    };
    ensure_empty(root, &data.next)?;
    if sync_protocol::sha256_hex(&optional(&crate::settings::settings_path())?)
        != manifest.original_settings_hash
    {
        return Err(AppError::Config("secret.restore_source_changed".into()));
    }
    let expected = serde_json::to_vec(&intent).map_err(|_| invalid())?;
    session::write_durable(&root.join(INTENT), &expected)?;
    hook(Checkpoint::Published)?;
    finish_initial(
        root, intent, manifest, data, settings, store, hook, &expected,
    )
}
fn install(
    root: &Path,
    data: ValidatedRestore,
    settings: AppSettings,
    current: VaultContext,
    store: &dyn KeyStore,
    automatic_unlock: bool,
    owner: Option<&BootstrapInstallPermit>,
) -> Result<Arc<SecretSession>, AppError> {
    transition::ensure_legacy_generation_owner(root)?;
    transition::ensure_legacy_metadata(current.metadata())?;
    transition::ensure_legacy_metadata(data.next.metadata())?;
    let path = root.join(crate::config::DB_FILE_NAME);
    if !regular(&path)? {
        return Err(invalid());
    }
    vault::preflight(&path)?;
    let connection = rusqlite::Connection::open(&path)?;
    vault::check_identity(&connection, &current)?;
    inventory::validate_database(&connection, &current)?;
    let session = SecretSession::from_context(root.to_owned(), current);
    let db = Database::from_connection(connection, session.clone());
    // Recovery into an existing legacy database keeps its schema. Modern and
    // fresh bootstrap retain the original prepared image and migration path.
    let incoming = if Database::get_user_version(&*db.conn.lock()?)?
        <= crate::database::UPSTREAM4_SOURCE_SCHEMA_VERSION
    {
        let sql = std::str::from_utf8(&data.snapshot.db_sql).map_err(|_| invalid())?;
        db.validate_sync_snapshot_for_current(
            sql,
            data.snapshot.manifest.vault_metadata()?,
            &data.next,
        )?
    } else {
        data.incoming
    };
    let location = settings.skill_storage_location;
    let skills = transition::SkillsReplacement {
        source: data.skills.source.clone(),
        location,
    };
    let prepare =
        move |connection: &rusqlite::Connection, current: &VaultContext, next: &VaultContext| {
            Database::prepare_sync_join(incoming, connection, current, next)
        };
    if let Some(owner) = owner {
        transition::install_bootstrap_generation_with_settings(
            &db,
            store,
            data.next,
            automatic_unlock,
            prepare,
            Some(skills),
            settings,
            owner,
        )?;
    } else {
        transition::install_generation_with_settings(
            &db,
            store,
            data.next,
            automatic_unlock,
            prepare,
            Some(skills),
            settings,
        )?;
    }
    Ok(session)
}
#[allow(clippy::too_many_arguments)]
fn finish_initial(
    root: &Path,
    intent: Intent,
    manifest: Manifest,
    data: ValidatedRestore,
    settings: AppSettings,
    store: &dyn KeyStore,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
    expected: &[u8],
) -> Result<Arc<SecretSession>, AppError> {
    transition::ensure_legacy_generation_owner(root)?;
    transition::ensure_legacy_metadata(&intent.metadata)?;
    transition::ensure_legacy_metadata(data.next.metadata())?;
    let owner = BootstrapInstallPermit::for_intent(root, &intent, &data.next, expected)?;
    owner.verify(root, &data.next)?;
    if manifest.automatic_unlock {
        save_verified(
            store,
            &intent.metadata.vault_id,
            &intent.metadata.key_id,
            &data.next.export_key(),
        )
        .map_err(|_| AppError::Config("secret.store_unavailable".into()))?;
    }
    let path = root.join(crate::config::DB_FILE_NAME);
    if !regular(&path)? {
        if regular(&root.join("vault.json"))? {
            return Err(invalid());
        }
        let seed = rusqlite::Connection::open_in_memory()?;
        vault::upgrade_staging(&seed, &data.next)?;
        seed.execute_batch("VACUUM;")?;
        session::write_durable(&path, &seed.serialize(rusqlite::MAIN_DB)?)?;
    } else {
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        vault::preflight_connection(&connection)?;
        vault::check_identity(&connection, &data.next)?;
        inventory::validate_database(&connection, &data.next)?;
    }
    hook(Checkpoint::Database)?;
    owner.verify(root, &data.next)?;
    if !regular(&root.join("vault.json"))? {
        session::write_metadata(
            root,
            &session::completed_metadata(&data.next, manifest.automatic_unlock)?,
        )?;
    } else {
        let saved = session::read_metadata(root)?;
        if saved.metadata != *data.next.metadata() {
            return Err(invalid());
        }
        data.next
            .open(&["local", "migration-state"], &saved.migration_state)
            .map_err(inventory::secret_error)?;
    }
    hook(Checkpoint::Metadata)?;
    owner.verify(root, &data.next)?;
    let next = data.next.clone();
    let session = install(
        root,
        data,
        settings,
        next,
        store,
        manifest.automatic_unlock,
        Some(&owner),
    )?;
    hook(Checkpoint::Installed)?;
    owner.verify(root, session.read()?.encrypted()?)?;
    std::fs::remove_file(root.join(INTENT)).map_err(|e| AppError::io(root.join(INTENT), e))?;
    sync_dir(root)?;
    let _ = std::fs::remove_dir_all(staging(root, &intent.id)?);
    Ok(session)
}
fn read_artifact(
    root: &Path,
    intent: &Intent,
    next: &VaultContext,
    name: &str,
) -> Result<Zeroizing<Vec<u8>>, AppError> {
    let stage = staging(root, &intent.id)?;
    if !directory(&stage)? {
        return Err(invalid());
    }
    let path = stage.join(name);
    if !regular(&path)? {
        return Err(invalid());
    }
    let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
    next.open(
        &["local", "bootstrap-artifact", &intent.id, name],
        std::str::from_utf8(&bytes).map_err(|_| invalid())?,
    )
    .map_err(inventory::secret_error)
}
/// Runs before normal metadata/database admission, including non-GUI session opens.
pub(crate) fn recover(
    root: &Path,
    store: &dyn KeyStore,
    password: Option<&str>,
) -> Result<(), AppError> {
    recover_record(root, store, password, None)
}
pub(crate) fn recover_inspected(
    root: &Path,
    store: &dyn KeyStore,
    password: Option<&str>,
    expected: &[u8],
) -> Result<(), AppError> {
    recover_record(root, store, password, Some(expected))
}
fn recover_record(
    root: &Path,
    store: &dyn KeyStore,
    password: Option<&str>,
    expected: Option<&[u8]>,
) -> Result<(), AppError> {
    transition::ensure_legacy_generation_owner(root)?;
    let _skills = crate::services::skill::skill_state_write_guard();
    let Some(bytes) = pending_record(root)? else {
        return if expected.is_some() {
            Err(AppError::Config("upgrade.source_changed".into()))
        } else {
            Ok(())
        };
    };
    if expected.is_some_and(|expected| expected != bytes) {
        return Err(AppError::Config("upgrade.source_changed".into()));
    }
    if !directory(root)? {
        return Err(invalid());
    }
    let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if intent.version != 1 {
        return Err(invalid());
    }
    transition::ensure_legacy_metadata(&intent.metadata)?;
    staging(root, &intent.id)?;
    let next = if let Some(password) = password {
        VaultContext::from_password(intent.metadata.clone(), password)
            .map_err(inventory::secret_error)?
    } else {
        let key = store
            .load(&intent.metadata.vault_id, &intent.metadata.key_id)
            .map_err(|_| AppError::Config("secret.store_unavailable".into()))?
            .ok_or_else(|| AppError::Config("secret.locked".into()))?;
        VaultContext::from_key(intent.metadata.clone(), key).map_err(inventory::secret_error)?
    };
    let plain = next
        .open(&["local", "bootstrap-intent"], &intent.body)
        .map_err(inventory::secret_error)?;
    let manifest: Manifest = serde_json::from_slice(&plain).map_err(|_| invalid())?;
    if manifest.snapshot.vault_metadata()? != next.metadata() {
        return Err(invalid());
    }
    let sql = read_artifact(root, &intent, &next, "database")?;
    let zip = read_artifact(root, &intent, &next, "skills")?;
    let encoded = read_artifact(root, &intent, &next, "settings")?;
    if sync_protocol::sha256_hex(&encoded) != manifest.settings_hash {
        return Err(invalid());
    }
    let settings = crate::settings::decode_settings_with_vault(&encoded, &next)?;
    let snapshot = DownloadedSnapshot {
        manifest: manifest.snapshot.clone(),
        manifest_hash: String::new(),
        etag: None,
        db_sql: sql.to_vec(),
        skills_zip: zip.to_vec(),
        layout: sync_protocol::RemoteLayout::Current,
        source_path: String::new(),
    };
    let data = validate(snapshot, next)?;
    let current_bytes = optional(&crate::settings::settings_path())?;
    if sync_protocol::sha256_hex(&current_bytes) != manifest.original_settings_hash {
        // Re-sealing uses fresh nonces; compare authenticated settings values.
        let current = crate::settings::decode_settings_with_vault(&current_bytes, &data.next)?;
        if serde_json::to_value(&current).map_err(|_| invalid())?
            != serde_json::to_value(&settings).map_err(|_| invalid())?
        {
            return Err(AppError::Config("secret.restore_source_changed".into()));
        }
    }
    if !super::files::stage_owned_files_with_vault(root, &data.next, false)?.is_empty() {
        return Err(AppError::Config("secret.local_credentials_exist".into()));
    }
    transition::recover(root, store, password)?;
    finish_initial(
        root,
        intent,
        manifest,
        data,
        settings,
        store,
        &mut |_| Ok(()),
        &bytes,
    )?;
    Ok(())
}
/// Passive journal bytes; authentication and recovery remain with this owner.
pub(crate) fn pending_record(root: &Path) -> Result<Option<Vec<u8>>, AppError> {
    let path = root.join(INTENT);
    crate::config_file_io::read_regular_file(&path, 1024 * 1024).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            invalid()
        } else {
            AppError::io(&path, error)
        }
    })
}

#[cfg(any(test, feature = "test-hooks"))]
mod tests {
    use super::*;
    use crate::secrets::{testing::MemoryKeyStore, upgrade::checkpoint};
    use std::{collections::BTreeMap, io::Write};
    struct Fixture {
        home: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
            Self { home, previous }
        }
        fn root(&self) -> PathBuf {
            self.home.path().join(crate::APP_DIR_NAME)
        }
        fn snapshot(&self) -> DownloadedSnapshot {
            self.snapshot_for_version(20)
        }
        fn snapshot_for_version(&self, version: i32) -> DownloadedSnapshot {
            let context = VaultContext::generate()
                .unwrap()
                .with_password("remote recovery password")
                .unwrap();
            self.snapshot_for_context(version, context)
        }
        fn snapshot_for_context(&self, version: i32, context: VaultContext) -> DownloadedSnapshot {
            let connection = rusqlite::Connection::open_in_memory().unwrap();
            crate::database::vault::upgrade_staging_for_target(&connection, &context, version)
                .unwrap();
            let session = SecretSession::from_context(self.home.path().join("source"), context);
            let source = Database::from_connection(connection, session);
            source
                .save_provider(
                    "claude",
                    &crate::provider::Provider::with_id(
                        "remote-provider".into(),
                        "Remote provider".into(),
                        serde_json::json!({"api_key":"remote-provider-canary"}),
                        None,
                    ),
                )
                .unwrap();
            let (sql, metadata) = source.export_sync_snapshot().unwrap();
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip.start_file("example/SKILL.md", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"restored skill").unwrap();
            let db_sql = sql.into_bytes();
            let skills_zip = zip.finish().unwrap().into_inner();
            let artifacts = BTreeMap::from([
                (
                    sync_protocol::REMOTE_DB_SQL.into(),
                    sync_protocol::ArtifactMeta {
                        sha256: sync_protocol::sha256_hex(&db_sql),
                        size: db_sql.len() as u64,
                    },
                ),
                (
                    sync_protocol::REMOTE_SKILLS_ZIP.into(),
                    sync_protocol::ArtifactMeta {
                        sha256: sync_protocol::sha256_hex(&skills_zip),
                        size: skills_zip.len() as u64,
                    },
                ),
            ]);
            let manifest = sync_protocol::SyncManifest {
                format: sync_protocol::PROTOCOL_FORMAT.into(),
                version: sync_protocol::PROTOCOL_VERSION,
                db_compat_version: Some(sync_protocol::DB_COMPAT_VERSION),
                device_name: "source device".into(),
                created_at: "test".into(),
                snapshot_id: sync_protocol::compute_snapshot_id(&artifacts, &metadata),
                artifacts,
                vault: Some(metadata),
                storage: None,
            };
            DownloadedSnapshot {
                manifest_hash: sync_protocol::sha256_hex(&serde_json::to_vec(&manifest).unwrap()),
                manifest,
                etag: None,
                db_sql,
                skills_zip,
                layout: sync_protocol::RemoteLayout::Current,
                source_path: "fixture".into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match &self.previous {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }
    fn source() -> RestoreSource {
        RestoreSource::Webdav(WebDavSyncSettings {
            base_url: "https://sync.example.invalid".into(),
            username: "fixture".into(),
            password: "supplied-connection-canary".into(),
            ..Default::default()
        })
    }
    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn pending_zcode_transaction_blocks_bootstrap_restore_before_mutation() {
        let fixture = Fixture::new();
        let root = fixture.root();
        let store = MemoryKeyStore::default();
        let session = SecretSession::open(&root, &store, None).unwrap();
        let original = session
            .read()
            .unwrap()
            .encrypted()
            .unwrap()
            .metadata()
            .clone();
        let connection = vault::prepare(
            &root.join(crate::config::DB_FILE_NAME),
            session.read().unwrap().encrypted().unwrap(),
        )
        .unwrap();
        let db = Database::from_connection(connection, session.clone());
        db.set_setting("global_proxy_url", "local-restore-canary")
            .unwrap();
        session.complete_migration().unwrap();
        let pending = root.join("zcode_account_transaction.json");
        session::write_durable(&pending, b"pending transaction fixture").unwrap();
        let key = store.load(&original.vault_id, &original.key_id).unwrap();
        let metadata = std::fs::read(root.join("vault.json")).unwrap();
        let database = db
            .conn
            .lock()
            .unwrap()
            .serialize(rusqlite::MAIN_DB)
            .unwrap()
            .to_vec();
        let settings = optional(&crate::settings::settings_path()).unwrap();
        let entries = || {
            std::fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<std::collections::BTreeSet<_>>()
        };
        let names = entries();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let prepared =
            prepare_snapshot(snapshot, source(), "remote recovery password", &expected).unwrap();

        let error = finalize_with_hook(&root, prepared, &store, true, &mut |_| {
            panic!("pending transaction must block restore publication")
        })
        .err()
        .expect("pending transaction must block restore");

        assert!(
            matches!(error, AppError::Config(code) if code == "secret.zcode_recovery_required")
        );
        assert_eq!(
            session.read().unwrap().encrypted().unwrap().metadata(),
            &original
        );
        assert_eq!(
            store.load(&original.vault_id, &original.key_id).unwrap(),
            key
        );
        assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), metadata);
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .serialize(rusqlite::MAIN_DB)
                .unwrap()
                .to_vec(),
            database
        );
        assert_eq!(
            db.get_setting("global_proxy_url").unwrap().as_deref(),
            Some("local-restore-canary")
        );
        assert_eq!(
            optional(&crate::settings::settings_path()).unwrap(),
            settings
        );
        assert_eq!(
            std::fs::read(pending).unwrap(),
            b"pending transaction fixture"
        );
        assert_eq!(entries(), names);
    }

    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn password_only_first_restore_does_not_require_a_system_key_store() {
        use crate::secrets::key_store::KeyStoreError;
        struct UnavailableStore;
        impl KeyStore for UnavailableStore {
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
        let fixture = Fixture::new();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let prepared =
            prepare_snapshot(snapshot, source(), "remote recovery password", &expected).unwrap();
        finalize(&fixture.root(), prepared, &UnavailableStore, false).unwrap();
        SecretSession::open_existing(
            &fixture.root(),
            &UnavailableStore,
            Some("remote recovery password"),
        )
        .unwrap();
    }

    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn uncertain_key_store_write_keeps_a_durable_fixed_policy_intent() {
        use crate::secrets::key_store::KeyStoreError;
        struct FailReadback {
            inner: MemoryKeyStore,
            written: std::sync::atomic::AtomicBool,
        }
        impl KeyStore for FailReadback {
            fn load(
                &self,
                vault: &str,
                key: &str,
            ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
                if self.written.load(std::sync::atomic::Ordering::SeqCst) {
                    Err(KeyStoreError::Unavailable)
                } else {
                    self.inner.load(vault, key)
                }
            }
            fn save(&self, vault: &str, key: &str, value: &[u8]) -> Result<(), KeyStoreError> {
                self.inner.save(vault, key, value)?;
                self.written
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            fn remove(&self, vault: &str, key: &str) -> Result<(), KeyStoreError> {
                self.inner.remove(vault, key)
            }
        }
        let fixture = Fixture::new();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let metadata = snapshot.manifest.vault_metadata().unwrap().clone();
        let prepared =
            prepare_snapshot(snapshot, source(), "remote recovery password", &expected).unwrap();
        let store = FailReadback {
            inner: MemoryKeyStore::default(),
            written: std::sync::atomic::AtomicBool::new(false),
        };
        assert!(finalize(&fixture.root(), prepared, &store, true).is_err());
        assert!(store
            .inner
            .load(&metadata.vault_id, &metadata.key_id)
            .unwrap()
            .is_some());
        assert!(pending(&fixture.root()).unwrap());
        recover(
            &fixture.root(),
            &store.inner,
            Some("remote recovery password"),
        )
        .unwrap();
        assert!(
            session::read_metadata(&fixture.root())
                .unwrap()
                .automatic_unlock
        );
        assert!(!pending(&fixture.root()).unwrap());
    }

    struct SnapshotServer {
        origin: String,
        requests: Arc<std::sync::atomic::AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl SnapshotServer {
        async fn start(snapshot: DownloadedSnapshot) -> Self {
            let artifacts = BTreeMap::from([
                (
                    sync_protocol::REMOTE_MANIFEST,
                    serde_json::to_vec(&snapshot.manifest).unwrap(),
                ),
                (sync_protocol::REMOTE_DB_SQL, snapshot.db_sql),
                (sync_protocol::REMOTE_SKILLS_ZIP, snapshot.skills_zip),
            ]);
            let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = requests.clone();
            let app =
                axum::Router::new().fallback(axum::routing::get(move |uri: axum::http::Uri| {
                    let body = uri
                        .path()
                        .rsplit('/')
                        .next()
                        .and_then(|name| artifacts.get(name))
                        .cloned();
                    observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async move {
                        match body {
                            Some(bytes) => (axum::http::StatusCode::OK, bytes),
                            None => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
                        }
                    }
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self {
                origin,
                requests,
                task,
            }
        }

        fn sources(&self) -> [RestoreSource; 2] {
            [
                RestoreSource::Webdav(WebDavSyncSettings {
                    base_url: self.origin.clone(),
                    username: "fixture".into(),
                    password: "fixture-password".into(),
                    ..Default::default()
                }),
                RestoreSource::S3(S3SyncSettings {
                    endpoint: self.origin.clone(),
                    region: "fixture-region".into(),
                    bucket: "fixture-bucket".into(),
                    access_key_id: "fixture-access".into(),
                    secret_access_key: "fixture-secret".into(),
                    ..Default::default()
                }),
            ]
        }

        fn request_count(&self) -> usize {
            self.requests.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for SnapshotServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn assert_settings_locked() {
        assert!(matches!(
            crate::settings::get_settings_for_frontend(),
            Err(AppError::Config(code)) if code == "secret.locked"
        ));
    }

    async fn ordinary_fetch(source: &RestoreSource) -> Result<DownloadedSnapshot, AppError> {
        match source {
            RestoreSource::Webdav(settings) => {
                crate::services::webdav_sync::fetch_snapshot(settings).await
            }
            RestoreSource::S3(settings) => crate::services::s3_sync::fetch_snapshot(settings).await,
        }
    }

    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn startup_preview_and_prepare_use_supplied_mock_transport_without_app_state() {
        let fixture = Fixture::new();
        let _settings = crate::settings::TestSettingsScope::enter().unwrap();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let server = SnapshotServer::start(snapshot).await;
        let transport = server.sources()[0].clone();
        assert_settings_locked();
        assert_eq!(preview(&transport).await.unwrap().snapshot_id, expected);
        let preview_requests = server.request_count();
        assert!(preview_requests > 0);
        prepare(transport, "remote recovery password", &expected)
            .await
            .unwrap();
        assert!(server.request_count() > preview_requests);
        assert!(!fixture.root().exists());
        assert_settings_locked();
    }

    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn startup_s3_preview_and_prepare_keep_settings_locked_without_app_state() {
        let fixture = Fixture::new();
        let _settings = crate::settings::TestSettingsScope::enter().unwrap();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let server = SnapshotServer::start(snapshot).await;
        let transport = server.sources()[1].clone();
        assert_settings_locked();
        assert_eq!(preview(&transport).await.unwrap().snapshot_id, expected);
        let preview_requests = server.request_count();
        assert!(preview_requests > 0);
        prepare(transport, "remote recovery password", &expected)
            .await
            .unwrap();
        assert!(server.request_count() > preview_requests);
        assert!(!fixture.root().exists());
        assert_settings_locked();
    }

    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn normal_snapshot_fetch_requires_unlock_before_any_transport_request() {
        let fixture = Fixture::new();
        let _settings = crate::settings::TestSettingsScope::enter().unwrap();
        let server = SnapshotServer::start(fixture.snapshot()).await;
        for transport in server.sources() {
            assert!(matches!(
                ordinary_fetch(&transport).await,
                Err(AppError::Config(code)) if code == "secret.locked"
            ));
        }
        assert_eq!(server.request_count(), 0);
        assert!(!fixture.root().exists());
        assert_settings_locked();
    }

    async fn assert_checkpoint_blocks_both_entries(server: &SnapshotServer) {
        for transport in server.sources() {
            assert!(matches!(
                ordinary_fetch(&transport).await,
                Err(AppError::Config(code)) if code == "upgrade.sync_paused"
            ));
            assert!(matches!(
                preview(&transport).await,
                Err(AppError::Config(code)) if code == "upgrade.sync_paused"
            ));
            assert!(matches!(
                prepare(transport, "unused", "unused").await,
                Err(AppError::Config(code)) if code == "upgrade.sync_paused"
            ));
        }
        assert_eq!(server.request_count(), 0);
    }

    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn startup_and_normal_fetch_refuse_pending_and_corrupt_checkpoints_before_requests() {
        for corrupt in [false, true] {
            let fixture = Fixture::new();
            let _settings = crate::settings::TestSettingsScope::enter().unwrap();
            let server = SnapshotServer::start(fixture.snapshot()).await;
            let device = crate::live::engine::DeviceStore::for_device();
            if corrupt {
                crate::config_file_io::ensure_private_directory(device.root()).unwrap();
                std::fs::write(device.root().join(checkpoint::FILE), b"corrupt").unwrap();
            } else {
                let db = crate::secrets::testing::initialize_database().unwrap();
                checkpoint::create(
                    &fixture.root(),
                    &device,
                    db.secret_session().read().unwrap().encrypted().unwrap(),
                    &[],
                )
                .unwrap();
            }
            let checkpoint = device.root().join(checkpoint::FILE);
            let before = std::fs::read(&checkpoint).unwrap();
            let _locked = crate::settings::TestSettingsScope::enter().unwrap();
            assert_settings_locked();
            assert_checkpoint_blocks_both_entries(&server).await;
            assert_eq!(std::fs::read(checkpoint).unwrap(), before);
            assert_settings_locked();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn startup_and_normal_fetch_refuse_symlinked_checkpoint_before_requests() {
        let fixture = Fixture::new();
        let _settings = crate::settings::TestSettingsScope::enter().unwrap();
        let server = SnapshotServer::start(fixture.snapshot()).await;
        let device = crate::live::engine::DeviceStore::for_device();
        crate::config_file_io::ensure_private_directory(device.root()).unwrap();
        let target = fixture.home.path().join("checkpoint-target");
        std::fs::write(&target, b"unchanged checkpoint target").unwrap();
        std::os::unix::fs::symlink(&target, device.root().join(checkpoint::FILE)).unwrap();
        assert_checkpoint_blocks_both_entries(&server).await;
        assert_eq!(
            std::fs::read(target).unwrap(),
            b"unchanged checkpoint target"
        );
        assert!(!fixture.root().join("vault.json").exists());
        assert_settings_locked();
    }

    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn normal_snapshot_fetch_keeps_completed_upgrade_sync_paused_before_requests() {
        let fixture = Fixture::new();
        let _settings = crate::settings::TestSettingsScope::enter().unwrap();
        let server = SnapshotServer::start(fixture.snapshot()).await;
        let db = crate::secrets::testing::initialize_database().unwrap();
        crate::settings::set_upgrade_completion_with_storage(
            "completed-checkpoint-fixture",
            db.secret_session(),
            &db.secret_session().read().unwrap(),
        )
        .unwrap();
        assert!(!crate::live::engine::DeviceStore::for_device()
            .root()
            .join(checkpoint::FILE)
            .exists());
        for transport in server.sources() {
            assert!(matches!(
                ordinary_fetch(&transport).await,
                Err(AppError::Config(code)) if code == "upgrade.sync_paused"
            ));
        }
        assert_eq!(server.request_count(), 0);
    }

    #[tokio::test]
    #[cfg_attr(test, serial_test::serial)]
    async fn startup_transport_preserves_password_snapshot_and_hash_rejections() {
        let fixture = Fixture::new();
        let _settings = crate::settings::TestSettingsScope::enter().unwrap();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let server = SnapshotServer::start(snapshot).await;
        for transport in server.sources() {
            let before = server.request_count();
            assert!(prepare(transport.clone(), "wrong password", &expected)
                .await
                .is_err());
            assert!(server.request_count() > before);
            assert!(matches!(
                prepare(transport, "remote recovery password", "changed-snapshot").await,
                Err(AppError::Localized {
                    key: "sync.conflict",
                    ..
                })
            ));
            assert!(!fixture.root().exists());
        }
        let mut damaged = fixture.snapshot();
        damaged.db_sql[0] ^= 1;
        let corrupted = SnapshotServer::start(damaged).await;
        for transport in corrupted.sources() {
            assert!(matches!(
                preview(&transport).await,
                Err(AppError::Localized {
                    key: "sync.artifact_hash_mismatch",
                    ..
                })
            ));
        }
        assert!(corrupted.request_count() > 0);
        assert!(!fixture.root().exists());
        assert_settings_locked();
    }

    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn wrong_password_and_changed_snapshot_leave_the_root_untouched() {
        let fixture = Fixture::new();
        for wrong_password in [true, false] {
            let snapshot = fixture.snapshot();
            let expected = if wrong_password {
                snapshot.manifest.snapshot_id.clone()
            } else {
                "changed-snapshot".into()
            };
            let password = if wrong_password {
                "wrong recovery password"
            } else {
                "remote recovery password"
            };
            assert!(prepare_snapshot(snapshot, source(), password, &expected).is_err());
            assert!(!fixture.root().exists());
        }
    }
    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn every_bootstrap_checkpoint_recovers_with_only_the_remote_password() {
        for checkpoint in [
            Checkpoint::Published,
            Checkpoint::Database,
            Checkpoint::Metadata,
            Checkpoint::Installed,
        ] {
            let fixture = Fixture::new();
            let root = fixture.root();
            let snapshot = fixture.snapshot();
            let expected = snapshot.manifest.snapshot_id.clone();
            let prepared =
                prepare_snapshot(snapshot, source(), "remote recovery password", &expected)
                    .unwrap();
            let store = MemoryKeyStore::default();
            assert!(
                finalize_with_hook(&root, prepared, &store, false, &mut |point| if point
                    == checkpoint
                {
                    Err(invalid())
                } else {
                    Ok(())
                })
                .is_err()
            );
            assert!(pending(&root).unwrap());
            assert!(matches!(
                super::super::upgrade::inspect(
                    &root,
                    &crate::live::engine::DeviceStore::at(root.join("fixture-device"))
                )
                .unwrap(),
                super::super::upgrade::UpgradeInspection::RecoveryRequired(_)
            ));
            let session =
                SecretSession::open_existing(&root, &store, Some("remote recovery password"))
                    .unwrap();
            let connection =
                rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
            crate::database::vault::check_identity(
                &connection,
                session.read().unwrap().encrypted().unwrap(),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(root.join("skills/example/SKILL.md")).unwrap(),
                b"restored skill"
            );
            assert!(!pending(&root).unwrap());
        }
    }
    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn lost_system_key_can_be_recovered_only_by_a_candidate_with_the_same_key() {
        for (source_version, local_version, same_key) in [
            (17, 17, true),
            (17, 20, true),
            (20, 20, true),
            (20, 17, true),
            (17, 17, false),
            (20, 20, false),
        ] {
            let fixture = Fixture::new();
            let root = fixture.root();
            let snapshot = fixture.snapshot_for_version(source_version);
            let expected = snapshot.manifest.snapshot_id.clone();
            let candidate = VaultContext::from_password(
                snapshot.manifest.vault_metadata().unwrap().clone(),
                "remote recovery password",
            )
            .unwrap();
            let current = if same_key {
                candidate
            } else {
                VaultContext::generate().unwrap()
            };
            let connection =
                crate::database::vault::prepare(&root.join(crate::config::DB_FILE_NAME), &current)
                    .unwrap();
            session::write_metadata(&root, &session::completed_metadata(&current, true).unwrap())
                .unwrap();
            if local_version == 20 {
                Database::apply_upstream4_migrations_on_conn(&connection).unwrap();
            }
            let local = SecretSession::from_context(root.clone(), current);
            let database = Database::from_connection(connection, local.clone());
            let account_id = {
                let vault = local.read().unwrap();
                let connection = database.conn.lock().unwrap();
                crate::vendor::creds::save_account(
                    &connection,
                    &vault,
                    crate::vendor::Vendor::DeepSeek,
                    "retained-local-canary",
                    &crate::vendor::VendorAccount {
                        account_id: "local-account".into(),
                        label: "Local account".into(),
                        login_identifier: "local-login".into(),
                    },
                )
                .unwrap()
            };
            crate::secrets::files::CredentialFile::Codex
                .write(&local, br#"{"access_token":"retained-oauth-canary"}"#)
                .unwrap();
            let original = std::fs::read(root.join("vault.json")).unwrap();
            let original_database = std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap();
            let credential_path = crate::secrets::files::CredentialFile::Codex.path(&local);
            let original_credentials = std::fs::read(&credential_path).unwrap();
            let prepared =
                prepare_snapshot(snapshot, source(), "remote recovery password", &expected)
                    .unwrap();
            let store = MemoryKeyStore::default();
            let result = finalize(&root, prepared, &store, false);
            if same_key && source_version <= local_version {
                let session = result.unwrap();
                let connection =
                    rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
                assert_eq!(
                    Database::get_user_version(&connection).unwrap(),
                    local_version
                );
                let database = Database::from_connection(connection, session.clone());
                {
                    let context = session.read().unwrap();
                    let connection = database.conn.lock().unwrap();
                    assert_eq!(
                        crate::vendor::creds::get(&connection, &context, account_id)
                            .unwrap()
                            .unwrap()
                            .auth_token,
                        "retained-local-canary"
                    );
                }
                assert!(String::from_utf8_lossy(
                    &crate::secrets::files::CredentialFile::Codex
                        .read(&session)
                        .unwrap()
                        .unwrap()
                )
                .contains("retained-oauth-canary"));
            } else {
                assert!(result.is_err());
                assert_eq!(std::fs::read(root.join("vault.json")).unwrap(), original);
                assert_eq!(
                    std::fs::read(root.join(crate::config::DB_FILE_NAME)).unwrap(),
                    original_database
                );
                assert_eq!(
                    std::fs::read(&credential_path).unwrap(),
                    original_credentials
                );
                assert!(!pending(&root).unwrap());
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn replaced_bootstrap_intent_is_never_adopted_by_the_original_owner() {
        for replaced_at in [Checkpoint::Published, Checkpoint::Installed] {
            let fixture = Fixture::new();
            let root = fixture.root();
            let snapshot = fixture.snapshot();
            let expected = snapshot.manifest.snapshot_id.clone();
            let prepared =
                prepare_snapshot(snapshot, source(), "remote recovery password", &expected)
                    .unwrap();
            let result = finalize_with_hook(
                &root,
                prepared,
                &MemoryKeyStore::default(),
                false,
                &mut |point| {
                    if point == replaced_at {
                        std::fs::write(root.join(INTENT), b"foreign synthetic bootstrap intent")
                            .unwrap();
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            if replaced_at == Checkpoint::Published {
                assert!(!root.join(crate::config::DB_FILE_NAME).exists());
                assert!(!root.join("vault.json").exists());
            }
            assert_eq!(
                pending_record(&root).unwrap().unwrap(),
                b"foreign synthetic bootstrap intent"
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn existing_database_restore_checks_native_phase_with_original_context() {
        use crate::secrets::storage::StorageContext;
        use crate::workbuddy::native_owner::{inspect, testing, Disposition};
        for phase in ["pending", "corrupt", "wrong-key", "restored", "committed"] {
            let fixture = Fixture::new();
            let root = fixture.root();
            let current = VaultContext::generate()
                .unwrap()
                .with_password("remote recovery password")
                .unwrap();
            let snapshot = fixture.snapshot_for_context(17, current.clone());
            let expected = snapshot.manifest.snapshot_id.clone();
            let connection =
                vault::prepare(&root.join(crate::config::DB_FILE_NAME), &current).unwrap();
            drop(connection);
            session::write_metadata(&root, &session::completed_metadata(&current, true).unwrap())
                .unwrap();
            let client = crate::secrets::testing::tempdir().unwrap();
            let storage = StorageContext::Encrypted(if phase == "wrong-key" {
                VaultContext::generate().unwrap()
            } else {
                current
            });
            let (pending, restored, committed) =
                testing::phase_receipts(&root, &storage, client.path());
            let bytes = match phase {
                "pending" => pending,
                "corrupt" => b"synthetic corrupt native journal".to_vec(),
                "restored" => restored,
                _ => committed,
            };
            std::fs::write(
                root.join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE),
                bytes,
            )
            .unwrap();
            let before = crate::secrets::transition::fence_test_support::tree(fixture.home.path());
            let prepared =
                prepare_snapshot(snapshot, source(), "remote recovery password", &expected)
                    .unwrap();
            let result = finalize(&root, prepared, &MemoryKeyStore::default(), false);
            if matches!(phase, "restored" | "committed") {
                let restored = result.unwrap();
                assert_eq!(
                    inspect(&root, &restored.read().unwrap()).unwrap(),
                    Disposition::Clear
                );
                assert!(pending_record(&root).unwrap().is_none());
            } else {
                assert!(
                    result.is_err(),
                    "{phase} must block original bootstrap installation"
                );
                assert_eq!(
                    before,
                    crate::secrets::transition::fence_test_support::tree(fixture.home.path())
                );
            }
        }
    }

    #[test]
    #[cfg_attr(test, serial_test::serial)]
    fn first_restore_installs_remote_identity_and_encrypted_connection_settings() {
        let fixture = Fixture::new();
        let root = fixture.root();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let metadata = snapshot.manifest.vault_metadata().unwrap().clone();
        let prepared =
            prepare_snapshot(snapshot, source(), "remote recovery password", &expected).unwrap();
        let store = MemoryKeyStore::default();
        let session = finalize(&root, prepared, &store, false).unwrap();
        assert_eq!(
            session.read().unwrap().encrypted().unwrap().metadata(),
            &metadata
        );
        let settings = std::fs::read(crate::settings::settings_path()).unwrap();
        assert!(!String::from_utf8_lossy(&settings).contains("supplied-connection-canary"));
        assert_eq!(
            crate::settings::decode_settings_with_vault(
                &settings,
                session.read().unwrap().encrypted().unwrap()
            )
            .unwrap()
            .webdav_sync
            .unwrap()
            .password,
            "supplied-connection-canary"
        );
        assert_eq!(
            std::fs::read(root.join("skills/example/SKILL.md")).unwrap(),
            b"restored skill"
        );
        assert!(!root.join(".vault-bootstrap").exists());
        assert!(SecretSession::open_existing(&root, &store, None).is_err());
        SecretSession::open_existing(&root, &store, Some("remote recovery password")).unwrap();
    }
    include!("bootstrap_restore_legacy_fence_tests.rs");
}

#[cfg(feature = "test-hooks")]
pub(crate) use tests::verify_legacy_fence as verify_legacy_generation_fence_case;
