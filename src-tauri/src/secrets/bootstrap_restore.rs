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
            crate::services::webdav_sync::fetch_snapshot(settings).await
        }
        RestoreSource::S3(settings) => crate::services::s3_sync::fetch_snapshot(settings).await,
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
    directory(root)?;
    if pending(root)? || super::reset::pending(root)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let _skills = crate::services::skill::skill_state_write_guard();
    let PreparedRestore { data, source } = prepared;
    let existing = if regular(&root.join("vault.json"))? {
        let saved = session::read_metadata(root)?;
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
        return install(root, data, settings, current, store, automatic_unlock);
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
    session::write_durable(
        &root.join(INTENT),
        &serde_json::to_vec(&intent).map_err(|_| invalid())?,
    )?;
    hook(Checkpoint::Published)?;
    finish_initial(root, intent, manifest, data, settings, store, hook)
}
fn install(
    root: &Path,
    data: ValidatedRestore,
    settings: AppSettings,
    current: VaultContext,
    store: &dyn KeyStore,
    automatic_unlock: bool,
) -> Result<Arc<SecretSession>, AppError> {
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
    let location = settings.skill_storage_location;
    let skills = transition::SkillsReplacement {
        source: data.skills.source.clone(),
        location,
    };
    transition::install_generation_with_settings(
        &db,
        store,
        data.next,
        automatic_unlock,
        move |connection, current, next| {
            Database::prepare_sync_join(data.incoming, connection, current, next)
        },
        Some(skills),
        settings,
    )?;
    Ok(session)
}
fn finish_initial(
    root: &Path,
    intent: Intent,
    manifest: Manifest,
    data: ValidatedRestore,
    settings: AppSettings,
    store: &dyn KeyStore,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<Arc<SecretSession>, AppError> {
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
    let next = data.next.clone();
    let session = install(root, data, settings, next, store, manifest.automatic_unlock)?;
    hook(Checkpoint::Installed)?;
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
    let path = root.join(INTENT);
    if !regular(&path)? {
        return Ok(());
    }
    if !directory(root)? {
        return Err(invalid());
    }
    let _skills = crate::services::skill::skill_state_write_guard();
    let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, e))?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid());
    }
    let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if intent.version != 1 {
        return Err(invalid());
    }
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
    finish_initial(root, intent, manifest, data, settings, store, &mut |_| {
        Ok(())
    })?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::testing::MemoryKeyStore;
    use std::{collections::BTreeMap, io::Write};
    struct Fixture {
        home: tempfile::TempDir,
        previous: Option<std::ffi::OsString>,
    }
    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
            Self { home, previous }
        }
        fn root(&self) -> PathBuf {
            self.home.path().join(crate::APP_DIR_NAME)
        }
        fn snapshot(&self) -> DownloadedSnapshot {
            let context = VaultContext::generate()
                .unwrap()
                .with_password("remote recovery password")
                .unwrap();
            let connection = rusqlite::Connection::open_in_memory().unwrap();
            crate::database::vault::upgrade_staging(&connection, &context).unwrap();
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
    #[serial_test::serial]
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
    #[serial_test::serial]
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

    #[tokio::test]
    #[serial_test::serial]
    async fn startup_preview_and_prepare_use_supplied_mock_transport_without_app_state() {
        let fixture = Fixture::new();
        let snapshot = fixture.snapshot();
        let expected = snapshot.manifest.snapshot_id.clone();
        let manifest = serde_json::to_vec(&snapshot.manifest).unwrap();
        let sql = snapshot.db_sql;
        let zip = snapshot.skills_zip;
        let app = axum::Router::new().fallback(axum::routing::get(move |uri: axum::http::Uri| {
            let manifest = manifest.clone();
            let sql = sql.clone();
            let zip = zip.clone();
            async move {
                let bytes = if uri.path().ends_with(sync_protocol::REMOTE_MANIFEST) {
                    manifest
                } else if uri.path().ends_with(sync_protocol::REMOTE_DB_SQL) {
                    sql
                } else if uri.path().ends_with(sync_protocol::REMOTE_SKILLS_ZIP) {
                    zip
                } else {
                    return (axum::http::StatusCode::NOT_FOUND, Vec::new());
                };
                (axum::http::StatusCode::OK, bytes)
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let transport = RestoreSource::Webdav(WebDavSyncSettings {
            base_url: format!("http://{address}"),
            username: "fixture".into(),
            password: "fixture-password".into(),
            ..Default::default()
        });
        assert_eq!(preview(&transport).await.unwrap().snapshot_id, expected);
        prepare(transport, "remote recovery password", &expected)
            .await
            .unwrap();
        assert!(!fixture.root().exists());
        server.abort();
    }

    #[test]
    #[serial_test::serial]
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
    #[serial_test::serial]
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
            let session =
                SecretSession::open_existing(&root, &store, Some("remote recovery password"))
                    .unwrap();
            let connection =
                rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
            crate::database::vault::check_identity(&connection, &session.read().unwrap()).unwrap();
            assert_eq!(
                std::fs::read(root.join("skills/example/SKILL.md")).unwrap(),
                b"restored skill"
            );
            assert!(!pending(&root).unwrap());
        }
    }
    #[test]
    #[serial_test::serial]
    fn lost_system_key_can_be_recovered_only_by_a_candidate_with_the_same_key() {
        for same_key in [true, false] {
            let fixture = Fixture::new();
            let root = fixture.root();
            let snapshot = fixture.snapshot();
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
            let prepared =
                prepare_snapshot(snapshot, source(), "remote recovery password", &expected)
                    .unwrap();
            let store = MemoryKeyStore::default();
            let result = finalize(&root, prepared, &store, false);
            if same_key {
                let session = result.unwrap();
                let connection =
                    rusqlite::Connection::open(root.join(crate::config::DB_FILE_NAME)).unwrap();
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
                assert!(!pending(&root).unwrap());
            }
        }
    }

    #[test]
    #[serial_test::serial]
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
        assert_eq!(session.read().unwrap().metadata(), &metadata);
        let settings = std::fs::read(crate::settings::settings_path()).unwrap();
        assert!(!String::from_utf8_lossy(&settings).contains("supplied-connection-canary"));
        assert_eq!(
            crate::settings::decode_settings_with_vault(&settings, &session.read().unwrap())
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
}
