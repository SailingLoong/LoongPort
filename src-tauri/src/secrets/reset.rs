//! Explicit key-loss reset. The prior tree is retained as an encrypted archive;
//! normal unlock and generation replacement never invoke this operation.
use super::{inventory, session, VaultContext, VaultMetadata};
use crate::{database::vault, error::AppError};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResetPreview {
    pub fingerprint: String,
    pub protected_values: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    id: String,
    metadata: VaultMetadata,
    body: String,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ResetPhase {
    Prepared,
    CleanupCommitted,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    phase: ResetPhase,
    fingerprint: String,
    archive_hash: String,
    database_hash: String,
    settings_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryArchive {
    id: String,
    metadata: VaultMetadata,
    content: String,
}
fn invalid() -> AppError {
    AppError::Config("secret.invalid_reset".into())
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn parent(root: &Path) -> Result<&Path, AppError> {
    root.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(invalid)
}
fn intent_path(root: &Path) -> Result<PathBuf, AppError> {
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(invalid)?;
    Ok(parent(root)?.join(format!(".{name}-vault-reset")))
}
fn locations(root: &Path, id: &str) -> Result<(PathBuf, PathBuf, PathBuf), AppError> {
    if uuid::Uuid::parse_str(id)
        .map(|u| u.to_string())
        .ok()
        .as_deref()
        != Some(id)
    {
        return Err(invalid());
    }
    let parent = parent(root)?;
    Ok((
        parent.join(format!(".vault-reset-{id}")),
        parent.join(format!(".vault-reset-previous-{id}")),
        parent.join(format!("vault-recovery-{id}.lpbackup")),
    ))
}
fn regular(path: &Path) -> Result<bool, AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(invalid()),
        Ok(m) if m.is_file() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(path, e)),
    }
}
fn directory(path: &Path) -> Result<bool, AppError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(path, e)),
    }
}
fn read_optional(path: &Path) -> Result<Vec<u8>, AppError> {
    if !regular(path)? {
        return Ok(Vec::new());
    }
    std::fs::read(path).map_err(|e| AppError::io(path, e))
}
fn sync_dir(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    std::fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| AppError::io(path, e))?;
    Ok(())
}
fn snapshot(root: &Path) -> Result<Connection, AppError> {
    let path = root.join(crate::config::DB_FILE_NAME);
    let mut memory = Connection::open_in_memory()?;
    if regular(&path)? {
        let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        vault::preflight_connection(&source)?;
        vault::copy(&source, &mut memory)?;
    } else {
        return Err(AppError::Config("secret.migration_required".into()));
    }
    Ok(memory)
}
fn is_database_file(relative: &Path) -> bool {
    let Some(name) = relative.to_str() else {
        return false;
    };
    let database = crate::config::DB_FILE_NAME;
    name == database
        || ["-wal", "-shm", "-journal"]
            .iter()
            .any(|suffix| name == format!("{database}{suffix}"))
}
fn is_diagnostic_path(relative: &Path) -> bool {
    crate::panic_hook::is_diagnostic_path(relative)
}
fn tree_hash(root: &Path) -> Result<Vec<u8>, AppError> {
    let mut digest = Sha256::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|_| invalid())?;
        if entry.path() == root {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(invalid());
        }
        let relative = entry.path().strip_prefix(root).map_err(|_| invalid())?;
        if is_database_file(relative) || is_diagnostic_path(relative) {
            continue;
        }
        let name = relative.to_string_lossy().replace('\\', "/");
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        if entry.file_type().is_dir() {
            digest.update(b"directory");
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(invalid());
        }
        let bytes = std::fs::read(entry.path()).map_err(|e| AppError::io(entry.path(), e))?;
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(digest.finalize().to_vec())
}
fn fingerprint(root: &Path, conn: &Connection, settings: &[u8]) -> Result<String, AppError> {
    let mut digest = Sha256::new();
    for bytes in [
        conn.serialize(rusqlite::MAIN_DB)?.to_vec(),
        read_optional(&root.join("vault.json"))?,
        settings.to_vec(),
        tree_hash(root)?,
    ] {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(hex::encode(digest.finalize()))
}
pub(crate) fn preview(root: &Path) -> Result<ResetPreview, AppError> {
    if regular(&intent_path(root)?)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let conn = snapshot(root)?;
    let settings = read_optional(&crate::settings::settings_path())?;
    let fingerprint = fingerprint(root, &conn, &settings)?;
    let temporary = VaultContext::generate().map_err(inventory::secret_error)?;
    let protected_values = inventory::reset_database(&conn, &temporary)?;
    Ok(ResetPreview {
        fingerprint,
        protected_values,
    })
}
fn copy_and_archive(
    root: &Path,
    stage: &Path,
    vault: &VaultContext,
    id: &str,
    settings: &[u8],
) -> Result<Vec<u8>, AppError> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().unix_permissions(0o600);
    if directory(root)? {
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = entry.map_err(|_| invalid())?;
            if entry.path() == root {
                continue;
            }
            if entry.file_type().is_symlink() {
                return Err(invalid());
            }
            if entry.file_type().is_dir() {
                continue;
            }
            if !entry.file_type().is_file() {
                return Err(invalid());
            }
            let relative = entry.path().strip_prefix(root).map_err(|_| invalid())?;
            let relative_name = relative.to_string_lossy().replace('\\', "/");
            let bytes = std::fs::read(entry.path()).map_err(|e| AppError::io(entry.path(), e))?;
            archive
                .start_file(format!("data/{relative_name}"), options)
                .map_err(|_| invalid())?;
            archive.write_all(&bytes).map_err(|_| invalid())?;
            let first = relative
                .components()
                .next()
                .ok_or_else(invalid)?
                .as_os_str()
                .to_string_lossy();
            if is_diagnostic_path(relative)
                || first == "backups"
                || first == "vault.json"
                || first.starts_with(".vault-")
                || is_database_file(relative)
                || super::files::OwnedFile::registered(relative).is_ok()
                || entry.path() == crate::settings::settings_path()
            {
                continue;
            }
            session::write_durable(&stage.join(relative), &bytes)?;
        }
    }
    archive
        .start_file("device-settings.json", options)
        .map_err(|_| invalid())?;
    archive.write_all(settings).map_err(|_| invalid())?;
    let plaintext = zeroize::Zeroizing::new(archive.finish().map_err(|_| invalid())?.into_inner());
    let content = vault
        .seal(&["local", "reset-archive", id], &plaintext)
        .map_err(inventory::secret_error)?;
    serde_json::to_vec(&RecoveryArchive {
        id: id.into(),
        metadata: vault.metadata().clone(),
        content,
    })
    .map_err(|_| invalid())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    Published,
    Archived,
    Installed,
    Settings,
    CleanupCommitted,
}
pub(crate) fn pending(root: &Path) -> Result<bool, AppError> {
    regular(&intent_path(root)?)
}
pub(crate) fn reset(root: &Path, expected: &str, password: &str) -> Result<PathBuf, AppError> {
    reset_with_hook(root, expected, password, &mut |_| Ok(()))
}
fn reset_with_hook(
    root: &Path,
    expected: &str,
    password: &str,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<PathBuf, AppError> {
    let journal = intent_path(root)?;
    if regular(&journal)? {
        return Err(AppError::Config("secret.recovery_required".into()));
    }
    let conn = snapshot(root)?;
    let settings = read_optional(&crate::settings::settings_path())?;
    let original = fingerprint(root, &conn, &settings)?;
    if original != expected {
        return Err(AppError::Config("secret.reset_source_changed".into()));
    }
    let next = VaultContext::generate()
        .and_then(|v| v.with_password(password))
        .map_err(inventory::secret_error)?;
    let id = uuid::Uuid::new_v4().to_string();
    let (stage, _, archive_path) = locations(root, &id)?;
    crate::config::ensure_private_directory(&stage)?;
    let archive = copy_and_archive(root, &stage, &next, &id, &settings)?;
    session::write_durable(&archive_path, &archive)?;
    inventory::reset_database(&conn, &next)?;
    // Only the isolated image now contains values encrypted by the new key.
    vault::stamp(&conn, &next)?;
    vault::upgrade_staging(&conn, &next)?;
    conn.execute_batch("PRAGMA secure_delete=ON; VACUUM;")?;
    inventory::validate_database(&conn, &next)?;
    let database = conn.serialize(rusqlite::MAIN_DB)?.to_vec();
    session::write_durable(&stage.join(crate::config::DB_FILE_NAME), &database)?;
    let settings = if settings.is_empty() {
        crate::settings::encode_settings_with_vault(
            &crate::settings::AppSettings::default(),
            &next,
        )?
    } else {
        crate::settings::reset_protected_settings(&settings, &next)?
    };
    session::write_durable(&stage.join(".reset-settings"), &settings)?;
    session::write_metadata(&stage, &session::completed_metadata(&next, false)?)?;
    let manifest = Manifest {
        phase: ResetPhase::Prepared,
        fingerprint: original,
        archive_hash: hash(&archive),
        database_hash: hash(&database),
        settings_hash: hash(&settings),
    };
    // Re-read the source just before publication; stale UI approval cannot reset
    // a database changed by another operation after its preview.
    let check = snapshot(root)?;
    if fingerprint(
        root,
        &check,
        &read_optional(&crate::settings::settings_path())?,
    )? != expected
    {
        return Err(AppError::Config("secret.reset_source_changed".into()));
    }
    let intent = publish_intent(root, &id, &manifest, &next)?;
    hook(Checkpoint::Published)?;
    finish(root, &intent, &manifest, &next, hook)?;
    Ok(archive_path)
}
fn publish_intent(
    root: &Path,
    id: &str,
    manifest: &Manifest,
    next: &VaultContext,
) -> Result<Intent, AppError> {
    let body = next
        .seal(
            &["local", "reset-intent", id],
            &serde_json::to_vec(manifest).map_err(|_| invalid())?,
        )
        .map_err(inventory::secret_error)?;
    let intent = Intent {
        id: id.to_owned(),
        metadata: next.metadata().clone(),
        body,
    };
    session::write_durable(
        &intent_path(root)?,
        &serde_json::to_vec(&intent).map_err(|_| invalid())?,
    )?;
    Ok(intent)
}

fn validate_replacement(
    path: &Path,
    manifest: &Manifest,
    next: &VaultContext,
) -> Result<(), AppError> {
    if !directory(path)? {
        return Err(invalid());
    }
    let bytes = read_optional(&path.join(crate::config::DB_FILE_NAME))?;
    if hash(&bytes) != manifest.database_hash {
        return Err(invalid());
    }
    let conn = Connection::open_with_flags(
        path.join(crate::config::DB_FILE_NAME),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    vault::check_identity(&conn, next)?;
    inventory::validate_database(&conn, next)?;
    if session::read_metadata(path)?.metadata != *next.metadata() {
        return Err(invalid());
    }
    if hash(&read_optional(&path.join(".reset-settings"))?) != manifest.settings_hash {
        return Err(invalid());
    }
    Ok(())
}
fn finish(
    root: &Path,
    intent: &Intent,
    manifest: &Manifest,
    next: &VaultContext,
    hook: &mut dyn FnMut(Checkpoint) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let (stage, previous, archive_path) = locations(root, &intent.id)?;
    let archive_bytes = read_optional(&archive_path)?;
    if hash(&archive_bytes) != manifest.archive_hash {
        return Err(invalid());
    }
    let archive: RecoveryArchive = serde_json::from_slice(&archive_bytes).map_err(|_| invalid())?;
    if archive.id != intent.id || archive.metadata != *next.metadata() {
        return Err(invalid());
    }
    let original_archive = next
        .open(&["local", "reset-archive", &intent.id], &archive.content)
        .map_err(inventory::secret_error)?;
    let mut zip =
        zip::ZipArchive::new(Cursor::new(original_archive.as_slice())).map_err(|_| invalid())?;
    let mut original_settings = zeroize::Zeroizing::new(Vec::new());
    zip.by_name("device-settings.json")
        .map_err(|_| invalid())?
        .read_to_end(&mut original_settings)
        .map_err(|_| invalid())?;
    if directory(&stage)? {
        if manifest.phase != ResetPhase::Prepared {
            return Err(invalid());
        }
        validate_replacement(&stage, manifest, next)?;
        if directory(root)? {
            if directory(&previous)? {
                return Err(invalid());
            }
            let source = snapshot(root)?;
            if fingerprint(
                root,
                &source,
                &read_optional(&crate::settings::settings_path())?,
            )? != manifest.fingerprint
            {
                return Err(AppError::Config("secret.reset_source_changed".into()));
            }
            std::fs::rename(root, &previous).map_err(|e| AppError::io(root, e))?;
            sync_dir(parent(root)?)?;
            hook(Checkpoint::Archived)?;
        }
        std::fs::rename(&stage, root).map_err(|e| AppError::io(root, e))?;
        sync_dir(parent(root)?)?;
        hook(Checkpoint::Installed)?;
    }
    validate_replacement(root, manifest, next)?;
    let settings = read_optional(&root.join(".reset-settings"))?;
    session::write_durable(&crate::settings::settings_path(), &settings)?;
    hook(Checkpoint::Settings)?;
    if manifest.phase == ResetPhase::Prepared {
        if !directory(&previous)? {
            return Err(invalid());
        }
        let source = snapshot(&previous)?;
        if fingerprint(&previous, &source, &original_settings)? != manifest.fingerprint {
            return Err(AppError::Config("secret.reset_source_changed".into()));
        }
        drop(source);
        // Preserve process-owned diagnostics before the irreversible commit.
        // A retry can finish these renames because diagnostics are excluded from
        // the source fingerprint and each name is moved at most once.
        let diagnostics = root.join(format!("diagnostics-before-reset-{}", intent.id));
        for name in crate::panic_hook::diagnostic_entries() {
            let source = previous.join(&name);
            match std::fs::symlink_metadata(&source) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(AppError::io(&source, error)),
                Ok(metadata) if metadata.file_type().is_symlink() => return Err(invalid()),
                Ok(_) => {}
            }
            crate::config::ensure_private_directory(&diagnostics)?;
            let destination = diagnostics.join(name);
            if std::fs::symlink_metadata(&destination).is_ok() {
                return Err(invalid());
            }
            std::fs::rename(&source, &destination).map_err(|e| AppError::io(&source, e))?;
            sync_dir(&diagnostics)?;
            sync_dir(&previous)?;
        }
        let mut committed = manifest.clone();
        committed.phase = ResetPhase::CleanupCommitted;
        publish_intent(root, &intent.id, &committed, next)?;
        hook(Checkpoint::CleanupCommitted)?;
    }
    // Only the authenticated committed phase permits removing a partial tree.
    // Its complete source, replacement and recovery archive were all verified
    // before publication, so subsequent cleanup is safely repeatable.
    if directory(&previous)? {
        std::fs::remove_dir_all(&previous).map_err(|e| AppError::io(&previous, e))?;
        sync_dir(parent(root)?)?;
    }
    let journal = intent_path(root)?;
    std::fs::remove_file(&journal).map_err(|e| AppError::io(&journal, e))?;
    sync_dir(parent(root)?)?;
    let _ = std::fs::remove_file(root.join(".reset-settings"));
    Ok(())
}
/// Resuming a reset always requires the new password; a planted reset intent
/// cannot trigger deletion during automatic startup.
pub(crate) fn recover(root: &Path, password: Option<&str>) -> Result<(), AppError> {
    let path = intent_path(root)?;
    let bytes = read_optional(&path)?;
    if bytes.is_empty() {
        return Ok(());
    }
    if bytes.len() > 128 * 1024 {
        return Err(invalid());
    }
    let password = password.ok_or_else(|| AppError::Config("secret.locked".into()))?;
    let intent: Intent = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let next = VaultContext::from_password(intent.metadata.clone(), password)
        .map_err(inventory::secret_error)?;
    let plaintext = next
        .open(&["local", "reset-intent", &intent.id], &intent.body)
        .map_err(inventory::secret_error)?;
    let manifest: Manifest = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
    finish(root, &intent, &manifest, &next, &mut |_| Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{session::SecretSession, testing::MemoryKeyStore};
    use serial_test::serial;
    struct Fixture {
        _home: tempfile::TempDir,
        old: Option<std::ffi::OsString>,
        root: PathBuf,
        original_key: VaultContext,
    }
    impl Fixture {
        fn new() -> Self {
            let home = tempfile::tempdir().unwrap();
            let old = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
            let root = crate::config::get_app_config_dir();
            let session = SecretSession::open(
                &root,
                &MemoryKeyStore::default(),
                Some("old protection password"),
            )
            .unwrap();
            let original_key = session.read().unwrap().clone();
            crate::settings::unlock_settings_for_test(session.clone()).unwrap();
            let db = crate::Database::init_with_secrets(session).unwrap();
            db.set_setting("global_proxy_url", "https://reset-canary.invalid")
                .unwrap();
            db.set_setting("theme", "dark").unwrap();
            db.set_setting("universal_providers", "{}").unwrap();
            crate::rt::block_on(db.save_live_backup("codex", "{\"auth\":\"reset-canary\"}"))
                .unwrap();
            db.save_provider(
                "codex",
                &crate::provider::Provider::with_id(
                    "retained-id".into(),
                    "Retained Name".into(),
                    serde_json::json!({"key":"reset-secret-canary"}),
                    None,
                ),
            )
            .unwrap();
            session::write_durable(&root.join("notes.txt"), b"retained plain content").unwrap();
            Self {
                _home: home,
                old,
                root,
                original_key,
            }
        }
        fn assert_reset(&self) {
            let session = SecretSession::open_existing(
                &self.root,
                &MemoryKeyStore::default(),
                Some("new protection password"),
            )
            .unwrap();
            let db = crate::Database::init_with_secrets(session).unwrap();
            assert_eq!(db.get_setting("theme").unwrap().as_deref(), Some("dark"));
            assert_eq!(db.get_setting("global_proxy_url").unwrap().as_deref(), None);
            let provider = db
                .get_provider_by_id("retained-id", "codex")
                .unwrap()
                .unwrap();
            assert!(db.get_all_universal_providers().unwrap().is_empty());
            assert!(!crate::rt::block_on(db.has_any_live_backup()).unwrap());
            assert_eq!(provider.name, "Retained Name");
            assert_eq!(provider.settings_config, serde_json::json!({}));
            assert_eq!(
                std::fs::read(self.root.join("notes.txt")).unwrap(),
                b"retained plain content"
            );
            assert_ne!(
                db.secrets.read().unwrap().metadata().vault_id,
                self.original_key.metadata().vault_id
            );
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            match &self.old {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }
    #[test]
    #[serial]
    fn reset_retains_plain_facts_and_archives_ciphertext_without_old_key() {
        let f = Fixture::new();
        let preview = preview(&f.root).unwrap();
        assert!(preview.protected_values >= 3);
        let archive_path = reset(&f.root, &preview.fingerprint, "new protection password").unwrap();
        f.assert_reset();
        let bytes = std::fs::read(archive_path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("reset-secret-canary"));
        let archive: RecoveryArchive = serde_json::from_slice(&bytes).unwrap();
        let key = VaultContext::from_password(archive.metadata, "new protection password").unwrap();
        let plain = key
            .open(&["local", "reset-archive", &archive.id], &archive.content)
            .unwrap();
        let zip = zip::ZipArchive::new(Cursor::new(plain.to_vec())).unwrap();
        assert!(zip
            .file_names()
            .any(|name| name == format!("data/{}", crate::config::DB_FILE_NAME)));
    }
    #[test]
    #[serial]
    fn changed_source_and_short_password_do_not_replace_data() {
        let f = Fixture::new();
        let before = std::fs::read(f.root.join("vault.json")).unwrap();
        let preview = preview(&f.root).unwrap();
        assert!(reset(&f.root, &preview.fingerprint, "short").is_err());
        assert!(reset(&f.root, "stale fingerprint", "new protection password").is_err());
        assert_eq!(std::fs::read(f.root.join("vault.json")).unwrap(), before);
        assert!(!pending(&f.root).unwrap());
    }
    #[test]
    #[serial]
    fn edits_after_reset_publication_are_never_discarded() {
        let f = Fixture::new();
        let preview = preview(&f.root).unwrap();
        assert!(reset_with_hook(
            &f.root,
            &preview.fingerprint,
            "new protection password",
            &mut |at| {
                if at == Checkpoint::Published {
                    std::fs::write(f.root.join("notes.txt"), b"newer external edit").unwrap();
                    Err(invalid())
                } else {
                    Ok(())
                }
            }
        )
        .is_err());
        assert!(recover(&f.root, Some("new protection password")).is_err());
        assert_eq!(
            std::fs::read(f.root.join("notes.txt")).unwrap(),
            b"newer external edit"
        );
        assert!(pending(&f.root).unwrap());
    }

    #[test]
    #[serial]
    fn startup_logging_does_not_recreate_root_during_reset_recovery() {
        let f = Fixture::new();
        session::write_durable(&f.root.join("logs/old.log"), b"old process diagnostics").unwrap();
        session::write_durable(&f.root.join("crash.log.1"), b"previous crash diagnostics").unwrap();
        let preview = preview(&f.root).unwrap();
        assert!(reset_with_hook(
            &f.root,
            &preview.fingerprint,
            "new protection password",
            &mut |at| {
                if at == Checkpoint::Archived {
                    Err(invalid())
                } else {
                    Ok(())
                }
            }
        )
        .is_err());
        // Use the logging owner's startup path, then perform the actual directory
        // creation/file append that happens before the unlock coordinator starts.
        let diagnostics = crate::panic_hook::diagnostic_root_for(&f.root).unwrap();
        let log = diagnostics
            .join(crate::panic_hook::LOG_DIRECTORY)
            .join("startup.log");
        session::write_durable(&log, b"new startup diagnostics").unwrap();
        let intent: Intent =
            serde_json::from_slice(&read_optional(&intent_path(&f.root).unwrap()).unwrap())
                .unwrap();
        recover(&f.root, Some("new protection password")).unwrap();
        assert_eq!(std::fs::read(&log).unwrap(), b"new startup diagnostics");
        let preserved = f
            .root
            .join(format!("diagnostics-before-reset-{}", intent.id));
        assert_eq!(
            std::fs::read(preserved.join("logs/old.log")).unwrap(),
            b"old process diagnostics"
        );
        assert_eq!(
            std::fs::read(preserved.join("crash.log.1")).unwrap(),
            b"previous crash diagnostics"
        );
        assert!(!pending(&f.root).unwrap());
        f.assert_reset();
    }

    #[test]
    #[serial]
    fn committed_cleanup_recovers_after_previous_tree_is_partially_deleted() {
        let f = Fixture::new();
        let preview = preview(&f.root).unwrap();
        assert!(reset_with_hook(
            &f.root,
            &preview.fingerprint,
            "new protection password",
            &mut |at| {
                if at == Checkpoint::CleanupCommitted {
                    let intent: Intent = serde_json::from_slice(
                        &read_optional(&intent_path(&f.root).unwrap()).unwrap(),
                    )
                    .unwrap();
                    let (_, previous, _) = locations(&f.root, &intent.id).unwrap();
                    // Model process death after recursive cleanup has removed its
                    // first child but before the remaining directory is removed.
                    std::fs::remove_file(previous.join("notes.txt")).unwrap();
                    Err(invalid())
                } else {
                    Ok(())
                }
            }
        )
        .is_err());
        assert!(recover(&f.root, None).is_err());
        assert!(recover(&f.root, Some("incorrect password")).is_err());
        recover(&f.root, Some("new protection password")).unwrap();
        assert!(!pending(&f.root).unwrap());
        f.assert_reset();
    }

    #[test]
    #[serial]
    fn reset_resumes_at_each_directory_commit_boundary_with_new_password() {
        for checkpoint in [
            Checkpoint::Published,
            Checkpoint::Archived,
            Checkpoint::Installed,
            Checkpoint::Settings,
        ] {
            let f = Fixture::new();
            let preview = preview(&f.root).unwrap();
            assert!(reset_with_hook(
                &f.root,
                &preview.fingerprint,
                "new protection password",
                &mut |at| if at == checkpoint {
                    Err(invalid())
                } else {
                    Ok(())
                }
            )
            .is_err());
            assert!(recover(&f.root, None).is_err());
            assert!(recover(&f.root, Some("incorrect password")).is_err());
            recover(&f.root, Some("new protection password")).unwrap();
            assert!(!pending(&f.root).unwrap());
            f.assert_reset();
        }
    }
}
