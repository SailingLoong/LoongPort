//! Transport-agnostic sync protocol layer.
//!
//! Shared by WebDAV, S3, and future transports. Artifact set: `db.sql` + `skills.zip`.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::process::Command;
use std::sync::OnceLock;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::error::AppError;
use crate::secrets::VaultMetadata;
use crate::services::skill::{skill_state_read_guard, skill_state_write_guard};

// Re-export archive functions for use by transport layers.
pub(crate) use super::webdav_sync::archive::zip_skills_ssot;

// ─── Protocol constants ──────────────────────────────────────

/// Wire-format identifier stored in remote manifests.
/// Retains historic "webdav" naming for backward compatibility with existing remotes.
pub(crate) const PROTOCOL_FORMAT: &str = "cc-switch-webdav-sync";
pub(crate) const PROTOCOL_VERSION: u32 = 3;
pub(crate) const LEGACY_PROTOCOL_VERSION: u32 = 2;
pub(crate) const DB_COMPAT_VERSION: u32 = 7;
pub(crate) const LEGACY_DB_COMPAT_VERSION: u32 = 5;
pub(crate) const REMOTE_DB_SQL: &str = "db.sql";
pub(crate) const REMOTE_SKILLS_ZIP: &str = "skills.zip";
pub(crate) const REMOTE_MANIFEST: &str = "manifest.json";
pub(crate) const MAX_DEVICE_NAME_LEN: usize = 64;
pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SYNC_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// A remote head is replaced only against the version that was observed.
#[derive(Debug, Clone)]
pub(crate) enum PutCondition {
    Absent,
    Match(String),
}

impl PutCondition {
    pub(crate) fn header(&self) -> Result<(&'static str, String), AppError> {
        match self {
            Self::Absent => Ok(("if-none-match", "*".into())),
            Self::Match(etag) => Ok(("if-match", require_strong_etag(Some(etag))?)),
        }
    }
}

pub(crate) fn require_strong_etag(etag: Option<&str>) -> Result<String, AppError> {
    let etag = etag.filter(|value| {
        value.len() >= 2
            && value.starts_with('"')
            && value.ends_with('"')
            && value.as_bytes()[1..value.len() - 1]
                .iter()
                .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
    });
    etag.map(str::to_owned).ok_or_else(|| {
        localized(
            "sync.strong_etag_required",
            "同步服务未提供可靠的版本标识",
            "The sync service does not support reliable version checks.",
        )
    })
}

pub(crate) fn publication_conflict() -> AppError {
    localized(
        "sync.conflict",
        "远端配置已变化，请先下载最新配置",
        "Remote configuration changed. Download the latest snapshot before uploading.",
    )
}

pub(crate) fn publication_condition(
    remote: Option<(&[u8], Option<&str>)>,
    expected_hash: Option<&str>,
) -> Result<PutCondition, AppError> {
    match remote {
        None if expected_hash.is_none() => Ok(PutCondition::Absent),
        Some((bytes, etag)) if expected_hash == Some(sha256_hex(bytes).as_str()) => {
            Ok(PutCondition::Match(require_strong_etag(etag)?))
        }
        _ => Err(publication_conflict()),
    }
}

/// Probe on immutable bytes before touching the mutable head. An endpoint that
/// ignores a condition can only rewrite the same artifact bytes during the probe.
pub(crate) async fn verify_write_conditions<F, Fut>(mut put: F) -> Result<(), AppError>
where
    F: FnMut(PutCondition) -> Fut,
    Fut: Future<Output = Result<Option<String>, AppError>>,
{
    for condition in [
        PutCondition::Absent,
        PutCondition::Match(format!("\"{}\"", uuid::Uuid::new_v4())),
    ] {
        if put(condition).await?.is_some() {
            return Err(localized(
                "sync.conditions_unsupported",
                "同步服务不支持安全的并发更新",
                "The sync service does not enforce conditional writes.",
            ));
        }
    }
    Ok(())
}

pub(crate) fn snapshot_artifact_path(
    manifest: &SyncManifest,
    artifact: &str,
) -> Result<String, AppError> {
    if manifest.snapshot_id != compute_snapshot_id(&manifest.artifacts, manifest.vault_metadata()?)
    {
        return Err(localized(
            "sync.snapshot_identity_invalid",
            "同步快照标识无效",
            "Invalid sync snapshot identity.",
        ));
    }
    if !matches!(artifact, REMOTE_DB_SQL | REMOTE_SKILLS_ZIP) {
        return Err(AppError::InvalidInput("Unknown sync artifact".into()));
    }
    Ok(format!("snapshots/{}/{artifact}", manifest.snapshot_id))
}

// ─── Sync operation lock ────────────────────────────────────

/// Serialize every snapshot upload/download and vault lifecycle transition.
/// Lock order is sync mutex, skill state (if needed), session, then database.
/// Session guards are only held in synchronous export/apply, never over awaits.
///
/// WebDAV and S3 used to own separate mutexes, which allowed two transports to
/// restore the database and Skills SSOT concurrently. Keep the lock in this
/// transport-agnostic layer so future transports automatically share it too.
pub(crate) fn sync_mutex() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

pub(crate) async fn run_with_sync_lock<T, Fut>(operation: Fut) -> Result<T, AppError>
where
    Fut: Future<Output = Result<T, AppError>>,
{
    let _guard = sync_mutex().lock().await;
    operation.await
}

/// Tables whose changes make the remote configuration snapshot stale.
///
/// Keep this transport-agnostic so WebDAV and S3 cannot silently drift apart.
/// `model_pricing` is intentionally excluded while its local JSON sidecar is
/// the user-owned SSOT.
pub(crate) fn should_trigger_auto_sync_for_table(table: &str) -> bool {
    let normalized = table.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "providers"
            | "provider_endpoints"
            | "mcp_servers"
            | "prompts"
            | "skills"
            | "skill_repos"
            | "profiles"
            | "settings"
            | "proxy_config"
    )
}

// ─── Error helpers ───────────────────────────────────────────

pub(crate) fn localized(
    key: &'static str,
    zh: impl Into<String>,
    en: impl Into<String>,
) -> AppError {
    AppError::localized(key, zh, en)
}

pub(crate) fn io_context_localized(
    _key: &'static str,
    zh: impl Into<String>,
    en: impl Into<String>,
    source: std::io::Error,
) -> AppError {
    let zh_msg = zh.into();
    let en_msg = en.into();
    AppError::IoContext {
        context: format!("{zh_msg} ({en_msg})"),
        source,
    }
}

// ─── Types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncManifest {
    pub format: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_compat_version: Option<u32>,
    pub device_name: String,
    pub created_at: String,
    pub artifacts: BTreeMap<String, ArtifactMeta>,
    pub snapshot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<VaultMetadata>,
}

impl SyncManifest {
    pub(crate) fn vault_metadata(&self) -> Result<&VaultMetadata, AppError> {
        self.vault
            .as_ref()
            .ok_or_else(|| AppError::Config("sync.vault_metadata_required".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArtifactMeta {
    pub sha256: String,
    pub size: u64,
}

pub(crate) struct LocalSnapshot {
    pub db_sql: Vec<u8>,
    pub skills_zip: Vec<u8>,
    pub manifest_bytes: Vec<u8>,
    pub manifest_hash: String,
}

/// Fully fetched immutable artifacts; fetching never changes local state.
#[derive(Clone)]
pub(crate) struct DownloadedSnapshot {
    pub manifest: SyncManifest,
    pub manifest_hash: String,
    pub etag: Option<String>,
    pub db_sql: Vec<u8>,
    pub skills_zip: Vec<u8>,
    pub layout: RemoteLayout,
    pub source_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteLayout {
    Current,
    Legacy,
}

impl RemoteLayout {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Legacy => "legacy",
        }
    }
}

// ─── Snapshot building ───────────────────────────────────────

pub(crate) fn build_local_snapshot(
    db: &crate::database::Database,
) -> Result<LocalSnapshot, AppError> {
    // Keep the DB's skill rows and the filesystem SSOT at one logical point in
    // time. Skill writers take the matching write guard around both mutations.
    let _skill_state_guard = skill_state_read_guard();

    // Export database to SQL string
    let (sql_string, vault) = db.export_sync_snapshot()?;
    let db_sql = sql_string.into_bytes();

    // Pack skills into deterministic ZIP
    let tmp = tempdir().map_err(|e| {
        io_context_localized(
            "sync.snapshot_tmpdir_failed",
            "创建快照临时目录失败",
            "Failed to create temporary directory for snapshot",
            e,
        )
    })?;
    let skills_zip_path = tmp.path().join(REMOTE_SKILLS_ZIP);
    zip_skills_ssot(&skills_zip_path)?;
    let skills_zip = fs::read(&skills_zip_path).map_err(|e| AppError::io(&skills_zip_path, e))?;

    // Build artifact map and compute hashes
    let mut artifacts = BTreeMap::new();
    artifacts.insert(
        REMOTE_DB_SQL.to_string(),
        ArtifactMeta {
            sha256: sha256_hex(&db_sql),
            size: db_sql.len() as u64,
        },
    );
    artifacts.insert(
        REMOTE_SKILLS_ZIP.to_string(),
        ArtifactMeta {
            sha256: sha256_hex(&skills_zip),
            size: skills_zip.len() as u64,
        },
    );

    let snapshot_id = compute_snapshot_id(&artifacts, &vault);
    let manifest = SyncManifest {
        format: PROTOCOL_FORMAT.to_string(),
        version: PROTOCOL_VERSION,
        db_compat_version: Some(DB_COMPAT_VERSION),
        device_name: detect_system_device_name().unwrap_or_else(|| "Unknown Device".to_string()),
        created_at: Utc::now().to_rfc3339(),
        artifacts,
        snapshot_id,
        vault: Some(vault),
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::JsonSerialize { source: e })?;
    let manifest_hash = sha256_hex(&manifest_bytes);

    Ok(LocalSnapshot {
        db_sql,
        skills_zip,
        manifest_bytes,
        manifest_hash,
    })
}

// ─── Manifest handling ───────────────────────────────────────

/// Bind both artifact digests/sizes and the authenticated public vault metadata.
/// BTreeMap and struct serialization provide a deterministic field order.
pub(crate) fn compute_snapshot_id(
    artifacts: &BTreeMap<String, ArtifactMeta>,
    vault: &VaultMetadata,
) -> String {
    let identity = serde_json::to_vec(&(artifacts, vault))
        .expect("snapshot identity contains only serializable values");
    sha256_hex(&identity)
}

/// The transport separately requires the saved remote receipt and a strong ETag.
/// An older client must never publish over newer key or password metadata.
pub(crate) fn validate_upload_metadata(
    local: &VaultMetadata,
    remote: &VaultMetadata,
) -> Result<(), AppError> {
    remote
        .validate()
        .map_err(crate::secrets::inventory::secret_error)?;
    if remote.vault_id != local.vault_id {
        return Err(AppError::Config("sync.vault_adoption_required".into()));
    }
    if local.revision < remote.revision {
        return Err(AppError::Config("sync.vault_revision_newer".into()));
    }
    if local.revision == remote.revision && local != remote {
        return Err(AppError::Config("sync.vault_metadata_conflict".into()));
    }
    Ok(())
}

pub(crate) fn effective_db_compat_version(
    manifest: &SyncManifest,
    layout: RemoteLayout,
) -> Option<u32> {
    manifest
        .db_compat_version
        .or_else(|| (layout == RemoteLayout::Legacy).then_some(LEGACY_DB_COMPAT_VERSION))
}

pub(crate) fn validate_manifest_compat(
    manifest: &SyncManifest,
    layout: RemoteLayout,
) -> Result<(), AppError> {
    if manifest.format != PROTOCOL_FORMAT {
        return Err(localized(
            "sync.manifest_format_incompatible",
            format!("远端 manifest 格式不兼容: {}", manifest.format),
            format!(
                "Remote manifest format is incompatible: {}",
                manifest.format
            ),
        ));
    }
    if layout == RemoteLayout::Legacy || manifest.version == LEGACY_PROTOCOL_VERSION {
        return Err(AppError::Config("sync.legacy_import_required".into()));
    }
    if manifest.version != PROTOCOL_VERSION {
        return Err(AppError::Config(
            "sync.manifest_version_incompatible".into(),
        ));
    }
    let Some(db_compat_version) = effective_db_compat_version(manifest, layout) else {
        return Err(localized(
            "sync.manifest_db_version_missing",
            "远端 manifest 缺少数据库兼容版本",
            "Remote manifest is missing the database compatibility version.",
        ));
    };
    if db_compat_version != DB_COMPAT_VERSION {
        return Err(AppError::Config(
            "sync.manifest_db_version_incompatible".into(),
        ));
    }
    let vault = manifest.vault_metadata()?;
    vault
        .validate()
        .map_err(crate::secrets::inventory::secret_error)?;
    if vault.wrapped_key.is_none() {
        return Err(AppError::Config("sync.recovery_password_required".into()));
    }
    snapshot_artifact_path(manifest, REMOTE_DB_SQL)?;
    Ok(())
}

// ─── Artifact verification ───────────────────────────────────

pub(crate) fn validate_artifact_size_limit(artifact_name: &str, size: u64) -> Result<(), AppError> {
    if size > MAX_SYNC_ARTIFACT_BYTES {
        let max_mb = MAX_SYNC_ARTIFACT_BYTES / 1024 / 1024;
        return Err(localized(
            "sync.artifact_too_large",
            format!("artifact {artifact_name} 超过下载上限（{} MB）", max_mb),
            format!(
                "Artifact {artifact_name} exceeds download limit ({} MB)",
                max_mb
            ),
        ));
    }
    Ok(())
}

/// Verify that downloaded artifact bytes match the expected size and SHA-256 hash.
pub(crate) fn verify_artifact(
    bytes: &[u8],
    artifact_name: &str,
    meta: &ArtifactMeta,
) -> Result<(), AppError> {
    // Quick size check before expensive hash
    if bytes.len() as u64 != meta.size {
        return Err(localized(
            "sync.artifact_size_mismatch",
            format!(
                "artifact {artifact_name} 大小不匹配 (expected: {}, got: {})",
                meta.size,
                bytes.len(),
            ),
            format!(
                "Artifact {artifact_name} size mismatch (expected: {}, got: {})",
                meta.size,
                bytes.len(),
            ),
        ));
    }

    let actual_hash = sha256_hex(bytes);
    if actual_hash != meta.sha256 {
        return Err(localized(
            "sync.artifact_hash_mismatch",
            format!(
                "artifact {artifact_name} SHA256 校验失败 (expected: {}..., got: {}...)",
                meta.sha256.get(..8).unwrap_or(&meta.sha256),
                actual_hash.get(..8).unwrap_or(&actual_hash),
            ),
            format!(
                "Artifact {artifact_name} SHA256 verification failed (expected: {}..., got: {}...)",
                meta.sha256.get(..8).unwrap_or(&meta.sha256),
                actual_hash.get(..8).unwrap_or(&actual_hash),
            ),
        ));
    }
    Ok(())
}

// ─── Snapshot application ────────────────────────────────────

/// Keep filesystem and SQLite transition work off the async network executor.
pub(crate) async fn apply_downloaded_snapshot(
    db: std::sync::Arc<crate::database::Database>,
    snapshot: DownloadedSnapshot,
) -> Result<DownloadedSnapshot, AppError> {
    tokio::task::spawn_blocking(move || {
        apply_snapshot(
            &db,
            &snapshot.manifest,
            &snapshot.db_sql,
            &snapshot.skills_zip,
        )?;
        Ok(snapshot)
    })
    .await
    .map_err(|_| AppError::Config("secret.operation_failed".into()))?
}

pub(crate) fn apply_snapshot(
    db: &crate::database::Database,
    manifest: &SyncManifest,
    db_sql: &[u8],
    skills_zip: &[u8],
) -> Result<(), AppError> {
    apply_snapshot_with_store(
        db,
        manifest,
        db_sql,
        skills_zip,
        &crate::secrets::key_store::SystemKeyStore,
    )
}

fn apply_snapshot_with_store(
    db: &crate::database::Database,
    manifest: &SyncManifest,
    db_sql: &[u8],
    skills_zip: &[u8],
    store: &dyn crate::secrets::key_store::KeyStore,
) -> Result<(), AppError> {
    validate_manifest_compat(manifest, RemoteLayout::Current)?;
    let next = db.secrets.read()?.clone();
    let sql =
        std::str::from_utf8(db_sql).map_err(|_| AppError::Config("sync.sql_not_utf8".into()))?;
    let staged =
        crate::database::Database::validate_sync_snapshot(sql, manifest.vault_metadata()?, &next)?;
    install_sync_snapshot(
        db,
        next,
        skills_zip,
        store,
        move |connection, current, next| {
            crate::database::Database::prepare_sync_join(staged, connection, current, next)
        },
    )
}

/// Explicit user-authorized replacement. The caller owns the shared sync mutex.
/// Authentication and artifact admission finish before the engine starts its journal.
pub(crate) fn restore_from_sync(
    db: &crate::database::Database,
    snapshot: &DownloadedSnapshot,
    expected_snapshot_id: &str,
    password: &str,
    store: &dyn crate::secrets::key_store::KeyStore,
) -> Result<(), AppError> {
    validate_manifest_compat(&snapshot.manifest, snapshot.layout)?;
    if snapshot.manifest.snapshot_id != expected_snapshot_id {
        return Err(publication_conflict());
    }
    for (name, bytes) in [
        (REMOTE_DB_SQL, &snapshot.db_sql),
        (REMOTE_SKILLS_ZIP, &snapshot.skills_zip),
    ] {
        let meta = snapshot
            .manifest
            .artifacts
            .get(name)
            .ok_or_else(|| AppError::Config("sync.artifact_missing".into()))?;
        verify_artifact(bytes, name, meta)?;
    }
    let next = crate::secrets::VaultContext::from_password(
        snapshot.manifest.vault_metadata()?.clone(),
        password,
    )
    .map_err(crate::secrets::inventory::secret_error)?;
    let sql = std::str::from_utf8(&snapshot.db_sql)
        .map_err(|_| AppError::Config("sync.sql_not_utf8".into()))?;
    let staged = crate::database::Database::validate_sync_snapshot(
        sql,
        snapshot.manifest.vault_metadata()?,
        &next,
    )?;
    install_sync_snapshot(
        db,
        next,
        &snapshot.skills_zip,
        store,
        move |connection, current, next| {
            crate::database::Database::prepare_sync_join(staged, connection, current, next)
        },
    )
}

fn install_sync_snapshot<F>(
    db: &crate::database::Database,
    next: crate::secrets::VaultContext,
    skills_zip: &[u8],
    store: &dyn crate::secrets::key_store::KeyStore,
    prepare_database: F,
) -> Result<(), AppError>
where
    F: FnOnce(
        &rusqlite::Connection,
        &crate::secrets::VaultContext,
        &crate::secrets::VaultContext,
    ) -> Result<rusqlite::Connection, AppError>,
{
    let prepared_skills = super::webdav_sync::archive::stage_skills_zip(skills_zip)?;
    let automatic_unlock = {
        let _current = db.secrets.read()?;
        crate::secrets::session::read_metadata(db.secrets.root())?.automatic_unlock
    };
    let location = crate::settings::get_skill_storage_location();
    let _skill_state = skill_state_write_guard();
    crate::secrets::transition::install_generation(
        db,
        store,
        next,
        automatic_unlock,
        prepare_database,
        Some(crate::secrets::transition::SkillsReplacement {
            source: prepared_skills.source.clone(),
            location,
        }),
    )
}

// ─── Utilities ───────────────────────────────────────────────

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

pub(crate) fn detect_system_device_name() -> Option<String> {
    let env_name = ["CC_SWITCH_DEVICE_NAME", "COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find_map(|value| normalize_device_name(&value));

    if env_name.is_some() {
        return env_name;
    }

    let output = Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let hostname = String::from_utf8(output.stdout).ok()?;
    normalize_device_name(&hostname)
}

pub(crate) fn normalize_device_name(raw: &str) -> Option<String> {
    let compact = raw
        .chars()
        .fold(String::with_capacity(raw.len()), |mut acc, ch| {
            if ch.is_whitespace() {
                acc.push(' ');
            } else if !ch.is_control() {
                acc.push(ch);
            }
            acc
        });
    let normalized = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }

    let limited = trimmed
        .chars()
        .take(MAX_DEVICE_NAME_LEN)
        .collect::<String>();
    if limited.is_empty() {
        None
    } else {
        Some(limited)
    }
}

// ─── Sync status persistence ─────────────────────────────────

pub(crate) fn persist_sync_success_best_effort<S, F>(
    settings: &mut S,
    manifest_hash: String,
    etag: Option<String>,
    persist_fn: F,
) -> bool
where
    F: FnOnce(&mut S, String, Option<String>) -> Result<(), AppError>,
{
    match persist_fn(settings, manifest_hash, etag) {
        Ok(()) => true,
        Err(err) => {
            log::warn!("[Sync] Persist sync status failed, keep operation success: {err}");
            false
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────

#[cfg(test)]
fn test_vault_metadata() -> VaultMetadata {
    static METADATA: OnceLock<VaultMetadata> = OnceLock::new();
    METADATA
        .get_or_init(|| {
            crate::secrets::VaultContext::generate()
                .unwrap()
                .with_password("sync-test-password")
                .unwrap()
                .metadata()
                .clone()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn webdav_and_s3_operations_share_one_sync_mutex() {
        let webdav_lock = crate::services::webdav_sync::sync_mutex();
        let s3_lock = crate::services::s3_sync::sync_mutex();
        assert!(
            std::ptr::eq(webdav_lock, s3_lock),
            "every transport must expose the same global sync lock"
        );

        let guard = webdav_lock.lock().await;
        assert!(s3_lock.try_lock().is_err());
        drop(guard);
        assert!(s3_lock.try_lock().is_ok());
    }

    fn artifact(sha256: &str, size: u64) -> ArtifactMeta {
        ArtifactMeta {
            sha256: sha256.to_string(),
            size,
        }
    }

    #[test]
    fn auto_sync_table_filter_covers_shared_configuration() {
        for table in [
            "providers",
            "provider_endpoints",
            "mcp_servers",
            "prompts",
            "skills",
            "skill_repos",
            "profiles",
            "settings",
            "proxy_config",
        ] {
            assert!(
                should_trigger_auto_sync_for_table(table),
                "{table} should trigger an automatic snapshot upload"
            );
        }

        assert!(should_trigger_auto_sync_for_table("  PROFILES  "));
        for table in [
            "proxy_request_logs",
            "provider_health",
            "session_log_sync",
            "model_pricing",
        ] {
            assert!(
                !should_trigger_auto_sync_for_table(table),
                "{table} should not trigger automatic snapshot upload"
            );
        }
    }

    #[test]
    fn snapshot_id_is_stable() {
        let mut artifacts = BTreeMap::new();
        artifacts.insert("db.sql".to_string(), artifact("abc123", 100));
        artifacts.insert("skills.zip".to_string(), artifact("def456", 200));

        let id1 = compute_snapshot_id(&artifacts, &test_vault_metadata());
        let id2 = compute_snapshot_id(&artifacts, &test_vault_metadata());
        assert_eq!(id1, id2);
    }

    #[test]
    fn snapshot_id_changes_with_artifacts() {
        let mut a1 = BTreeMap::new();
        a1.insert("db.sql".to_string(), artifact("hash-a", 1));

        let mut a2 = BTreeMap::new();
        a2.insert("db.sql".to_string(), artifact("hash-b", 1));

        assert_ne!(
            compute_snapshot_id(&a1, &test_vault_metadata()),
            compute_snapshot_id(&a2, &test_vault_metadata())
        );
    }

    #[test]
    fn encrypted_sync_identity_binds_sizes_and_vault_metadata() {
        let mut artifacts = BTreeMap::from([(REMOTE_DB_SQL.into(), artifact("digest", 1))]);
        let original = test_vault_metadata();
        let baseline = compute_snapshot_id(&artifacts, &original);
        artifacts.get_mut(REMOTE_DB_SQL).unwrap().size += 1;
        assert_ne!(baseline, compute_snapshot_id(&artifacts, &original));
        artifacts.get_mut(REMOTE_DB_SQL).unwrap().size -= 1;
        let mut changed = original.clone();
        changed.revision += 1;
        assert_ne!(baseline, compute_snapshot_id(&artifacts, &changed));
    }

    #[test]
    fn encrypted_sync_upload_blocks_old_clients_and_foreign_or_conflicting_heads() {
        let old = crate::secrets::VaultContext::generate()
            .unwrap()
            .with_password("old-sync-password")
            .unwrap();
        let new = old.with_password("new-sync-password").unwrap();
        assert!(validate_upload_metadata(old.metadata(), new.metadata()).is_err());
        assert!(validate_upload_metadata(new.metadata(), old.metadata()).is_ok());
        assert!(validate_upload_metadata(new.metadata(), new.metadata()).is_ok());
        let conflict = old.with_password("another-sync-password").unwrap();
        assert!(validate_upload_metadata(new.metadata(), conflict.metadata()).is_err());
        let foreign = crate::secrets::VaultContext::generate().unwrap();
        assert!(validate_upload_metadata(new.metadata(), foreign.metadata()).is_err());
        let head = serde_json::to_vec(old.metadata()).unwrap();
        assert!(publication_condition(Some((&head, Some("\"etag\""))), None).is_err());
        let receipt = sha256_hex(&head);
        assert!(publication_condition(Some((&head, Some("\"etag\""))), Some(&receipt)).is_ok());
    }

    #[test]
    fn encrypted_sync_manifest_requires_metadata_and_portable_wrapper() {
        let mut manifest =
            manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        manifest.vault = None;
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
        manifest.vault = Some(
            crate::secrets::VaultContext::generate()
                .unwrap()
                .metadata()
                .clone(),
        );
        manifest.snapshot_id =
            compute_snapshot_id(&manifest.artifacts, manifest.vault_metadata().unwrap());
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn encrypted_sync_foreign_source_is_rejected_before_skill_archive_work() {
        let db = crate::database::Database::memory().unwrap();
        db.set_setting("local-sentinel", "unchanged").unwrap();
        let sql = db.export_sql_string().unwrap();
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        let error = apply_snapshot(&db, &manifest, sql.as_bytes(), b"invalid archive").unwrap_err();
        assert!(error.to_string().contains("sync.vault_adoption_required"));
        assert_eq!(
            db.get_setting("local-sentinel").unwrap().as_deref(),
            Some("unchanged")
        );
    }

    #[test]
    fn sha256_hex_is_correct() {
        let hash = sha256_hex(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn persist_best_effort_returns_true_on_success() {
        let mut dummy = ();
        let ok = persist_sync_success_best_effort(
            &mut dummy,
            "hash".to_string(),
            Some("etag".to_string()),
            |_settings, _hash, _etag| Ok(()),
        );
        assert!(ok);
    }

    #[test]
    fn persist_best_effort_returns_false_on_error() {
        let mut dummy = ();
        let ok = persist_sync_success_best_effort(
            &mut dummy,
            "hash".to_string(),
            None,
            |_settings, _hash, _etag| Err(AppError::Config("boom".to_string())),
        );
        assert!(!ok);
    }

    fn manifest_with(format: &str, version: u32, db_compat_version: Option<u32>) -> SyncManifest {
        let mut artifacts = BTreeMap::new();
        artifacts.insert("db.sql".to_string(), artifact("abc", 1));
        artifacts.insert("skills.zip".to_string(), artifact("def", 2));
        SyncManifest {
            format: format.to_string(),
            version,
            db_compat_version,
            device_name: "My MacBook".to_string(),
            created_at: "2026-02-12T00:00:00Z".to_string(),
            snapshot_id: compute_snapshot_id(&artifacts, &test_vault_metadata()),
            vault: Some(test_vault_metadata()),
            artifacts,
        }
    }

    #[test]
    fn validate_manifest_compat_accepts_supported_manifest() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_ok());
    }

    #[test]
    fn validate_manifest_compat_rejects_wrong_format() {
        let manifest = manifest_with("other-format", PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn validate_manifest_compat_rejects_wrong_version() {
        let manifest = manifest_with(
            PROTOCOL_FORMAT,
            PROTOCOL_VERSION + 1,
            Some(DB_COMPAT_VERSION),
        );
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn validate_manifest_compat_rejects_legacy_manifest_without_db_compat() {
        let manifest = manifest_with(PROTOCOL_FORMAT, 2, None);
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Legacy).is_err());
    }

    #[test]
    fn validate_manifest_compat_rejects_current_manifest_with_wrong_db_compat() {
        let manifest = manifest_with(
            PROTOCOL_FORMAT,
            PROTOCOL_VERSION,
            Some(LEGACY_DB_COMPAT_VERSION),
        );
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Current).is_err());
    }

    #[test]
    fn validate_manifest_compat_rejects_legacy_manifest_from_newer_db_generation() {
        let manifest = manifest_with(
            PROTOCOL_FORMAT,
            PROTOCOL_VERSION,
            Some(DB_COMPAT_VERSION + 1),
        );
        assert!(validate_manifest_compat(&manifest, RemoteLayout::Legacy).is_err());
    }

    #[test]
    fn effective_db_compat_version_defaults_legacy_layout_to_v5() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, None);
        assert_eq!(
            effective_db_compat_version(&manifest, RemoteLayout::Legacy),
            Some(LEGACY_DB_COMPAT_VERSION)
        );
        assert_eq!(
            effective_db_compat_version(&manifest, RemoteLayout::Current),
            None
        );
    }

    #[test]
    fn normalize_device_name_returns_none_for_blank_input() {
        assert_eq!(normalize_device_name("   \n\t  "), None);
    }

    #[test]
    fn normalize_device_name_collapses_whitespace_and_drops_control_chars() {
        assert_eq!(
            normalize_device_name("  Mac\tBook \n Pro\u{0007} "),
            Some("Mac Book Pro".to_string())
        );
    }

    #[test]
    fn normalize_device_name_truncates_to_max_len() {
        let long = "a".repeat(80);
        assert_eq!(normalize_device_name(&long).map(|s| s.len()), Some(64));
    }

    #[test]
    fn manifest_serialization_uses_device_name_only() {
        let manifest = manifest_with(PROTOCOL_FORMAT, PROTOCOL_VERSION, Some(DB_COMPAT_VERSION));
        let value = serde_json::to_value(&manifest).expect("serialize manifest");
        assert!(
            value.get("deviceName").is_some(),
            "manifest should contain deviceName"
        );
        assert_eq!(
            value.get("dbCompatVersion").and_then(|v| v.as_u64()),
            Some(DB_COMPAT_VERSION as u64)
        );
        assert!(
            value.get("deviceId").is_none(),
            "manifest should not contain deviceId"
        );
    }

    #[test]
    fn validate_artifact_size_limit_rejects_oversized_artifacts() {
        let err = validate_artifact_size_limit("skills.zip", MAX_SYNC_ARTIFACT_BYTES + 1)
            .expect_err("artifact larger than limit should be rejected");
        assert!(
            err.to_string().contains("too large") || err.to_string().contains("超过"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_artifact_size_limit_accepts_limit_boundary() {
        assert!(validate_artifact_size_limit("skills.zip", MAX_SYNC_ARTIFACT_BYTES).is_ok());
    }

    #[test]
    fn verify_artifact_rejects_size_mismatch() {
        let meta = artifact("abc123", 100);
        let bytes = vec![0u8; 50];
        let err = verify_artifact(&bytes, "test.bin", &meta)
            .expect_err("size mismatch should be rejected");
        assert!(
            err.to_string().contains("mismatch") || err.to_string().contains("不匹配"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn verify_artifact_rejects_hash_mismatch() {
        let meta = ArtifactMeta {
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            size: 5,
        };
        let bytes = b"hello";
        let err = verify_artifact(bytes, "test.bin", &meta)
            .expect_err("hash mismatch should be rejected");
        assert!(
            err.to_string().contains("verification failed") || err.to_string().contains("校验失败"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn verify_artifact_accepts_matching_data() {
        let data = b"hello";
        let meta = ArtifactMeta {
            sha256: sha256_hex(data),
            size: data.len() as u64,
        };
        assert!(verify_artifact(data, "test.bin", &meta).is_ok());
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::any,
        Router,
    };
    use std::{
        future::IntoFuture,
        sync::{Arc, Mutex},
    };

    #[derive(Default)]
    struct StoredObject {
        body: Vec<u8>,
        revision: u64,
    }
    #[derive(Default)]
    struct Store {
        objects: BTreeMap<String, StoredObject>,
        ignore_conditions: bool,
        race_on_manifest_publish: bool,
    }

    async fn object(
        State(state): State<Arc<Mutex<Store>>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let mut store = state.lock().unwrap();
        if store.race_on_manifest_publish
            && method == Method::PUT
            && uri.path().ends_with("/manifest.json")
            && headers.contains_key("if-match")
        {
            store.objects.get_mut(uri.path()).unwrap().revision += 1;
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
        let current = store.objects.get(uri.path());
        if method == Method::GET || method == Method::HEAD {
            return match current {
                Some(object) => (
                    StatusCode::OK,
                    [("etag", format!("\"{}\"", object.revision))],
                    object.body.clone(),
                )
                    .into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            };
        }
        let etag = current.map(|o| format!("\"{}\"", o.revision));
        if !store.ignore_conditions
            && (headers.get("if-none-match").is_some_and(|v| v == "*") && current.is_some()
                || headers
                    .get("if-match")
                    .is_some_and(|v| Some(v.as_bytes()) != etag.as_ref().map(|e| e.as_bytes())))
        {
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
        if method == Method::DELETE {
            store.objects.remove(uri.path());
            return StatusCode::NO_CONTENT.into_response();
        }
        let revision = current.map_or(1, |o| o.revision + 1);
        store.objects.insert(
            uri.path().into(),
            StoredObject {
                body: body.to_vec(),
                revision,
            },
        );
        (StatusCode::CREATED, [("etag", format!("\"{revision}\""))]).into_response()
    }

    struct TestRemote {
        use_s3: bool,
        endpoint: String,
        state: Arc<Mutex<Store>>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for TestRemote {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    impl TestRemote {
        async fn new(use_s3: bool) -> Self {
            let state = Arc::new(Mutex::new(Store::default()));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let app = Router::new()
                .fallback(any(object))
                .with_state(state.clone());
            let server = tokio::spawn(async move {
                axum::serve(listener, app).into_future().await.unwrap();
            });
            Self {
                use_s3,
                endpoint,
                state,
                server,
            }
        }
        fn creds(&self) -> super::super::s3::S3Credentials {
            super::super::s3::S3Credentials {
                endpoint: self.endpoint.clone(),
                bucket: "bucket".into(),
                region: "test".into(),
                access_key_id: "test".into(),
                secret_access_key: "test".into(),
            }
        }
        async fn artifact(&self, path: &str, bytes: &[u8]) -> Result<(), AppError> {
            if self.use_s3 {
                super::super::s3::put_object(
                    &self.creds(),
                    path,
                    bytes.to_vec(),
                    "application/octet-stream",
                )
                .await
            } else {
                super::super::webdav::put_bytes(
                    &format!("{}/{path}", self.endpoint),
                    &super::super::webdav::auth_from_credentials("test", "test"),
                    bytes.to_vec(),
                    "application/octet-stream",
                )
                .await
            }
        }
        async fn publish(
            &self,
            path: &str,
            bytes: &[u8],
            condition: &PutCondition,
        ) -> Result<Option<String>, AppError> {
            if self.use_s3 {
                super::super::s3::put_object_conditional(
                    &self.creds(),
                    path,
                    bytes.to_vec(),
                    "application/json",
                    condition,
                )
                .await
            } else {
                super::super::webdav::put_bytes_conditional(
                    &format!("{}/{path}", self.endpoint),
                    &super::super::webdav::auth_from_credentials("test", "test"),
                    bytes.to_vec(),
                    "application/json",
                    condition,
                )
                .await
            }
        }
        async fn read(&self, path: &str) -> Vec<u8> {
            if self.use_s3 {
                super::super::s3::get_object(&self.creds(), path, 4096)
                    .await
                    .unwrap()
                    .unwrap()
                    .0
            } else {
                super::super::webdav::get_bytes(
                    &format!("{}/{path}", self.endpoint),
                    &super::super::webdav::auth_from_credentials("test", "test"),
                    4096,
                )
                .await
                .unwrap()
                .unwrap()
                .0
            }
        }
    }

    #[tokio::test]
    async fn conditional_delete_and_head_work_for_both_transports() {
        use super::super::sync_cleanup::DeleteResult;
        for use_s3 in [false, true] {
            let remote = TestRemote::new(use_s3).await;
            remote
                .artifact("cleanup-target", b"old artifact")
                .await
                .unwrap();
            let auth = super::super::webdav::auth_from_credentials("test", "test");
            let url = format!("{}/cleanup-target", remote.endpoint);
            let head = async || {
                if use_s3 {
                    super::super::s3::head_object(&remote.creds(), "cleanup-target").await
                } else {
                    super::super::webdav::head_etag(&url, &auth).await
                }
            };
            let delete = async |etag: &str| {
                if use_s3 {
                    super::super::s3::delete_object_conditional(
                        &remote.creds(),
                        "cleanup-target",
                        etag,
                    )
                    .await
                } else {
                    super::super::webdav::delete_conditional(&url, &auth, etag).await
                }
            };
            let etag = head().await.unwrap().unwrap();
            assert_eq!(
                delete("\"wrong\"").await.unwrap(),
                DeleteResult::PreconditionFailed
            );
            assert_eq!(head().await.unwrap().as_deref(), Some(etag.as_str()));
            assert_eq!(delete(&etag).await.unwrap(), DeleteResult::Removed);
            assert!(head().await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn publication_artifact_cannot_overwrite_an_existing_snapshot() {
        for use_s3 in [false, true] {
            let remote = TestRemote::new(use_s3).await;
            remote.artifact("artifact", b"winner").await.unwrap();
            remote.artifact("artifact", b"winner").await.unwrap(); // retry is idempotent
            assert!(remote.artifact("artifact", b"loser").await.is_err());
            assert_eq!(remote.read("artifact").await, b"winner");
        }
    }

    #[tokio::test]
    async fn publication_losing_writer_preserves_winners_downloadable_snapshot() {
        for use_s3 in [false, true] {
            let remote = TestRemote::new(use_s3).await;
            let initial = remote
                .publish("manifest.json", b"old", &PutCondition::Absent)
                .await
                .unwrap()
                .unwrap();
            assert!(remote
                .publish("manifest.json", b"unexpected", &PutCondition::Absent)
                .await
                .unwrap()
                .is_none());
            remote
                .artifact("snapshots/winner/db.sql", b"winner data")
                .await
                .unwrap();
            remote
                .artifact("snapshots/loser/db.sql", b"loser data")
                .await
                .unwrap();
            let condition = PutCondition::Match(initial);
            assert!(remote
                .publish("manifest.json", b"snapshots/winner/db.sql", &condition)
                .await
                .unwrap()
                .is_some());
            assert!(remote
                .publish("manifest.json", b"snapshots/loser/db.sql", &condition)
                .await
                .unwrap()
                .is_none());
            let head = String::from_utf8(remote.read("manifest.json").await).unwrap();
            assert_eq!(remote.read(&head).await, b"winner data");
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn encrypted_sync_losing_cas_keeps_local_database_skills_and_receipt() {
        struct RestoreHome(Option<std::ffi::OsString>);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                    None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
                }
            }
        }
        let home = tempfile::tempdir().unwrap();
        let _restore = RestoreHome(std::env::var_os("CC_SWITCH_TEST_HOME"));
        std::env::set_var("CC_SWITCH_TEST_HOME", home.path());
        std::fs::create_dir_all(home.path().join(".cc-switch")).unwrap();
        let skills = crate::services::skill::SkillService::get_ssot_dir().unwrap();
        assert!(skills.starts_with(home.path()));
        std::fs::create_dir_all(&skills).unwrap();
        let skill = skills.join("local-skill.txt");
        std::fs::write(&skill, b"local skill").unwrap();
        let context = crate::secrets::VaultContext::generate()
            .unwrap()
            .with_password("cas-snapshot-password")
            .unwrap();
        let mut db = crate::database::Database::memory().unwrap();
        db.secrets = crate::secrets::session::SecretSession::from_context(
            home.path().join("vault"),
            context.clone(),
        );
        crate::database::vault::stamp(&db.conn.lock().unwrap(), &context).unwrap();
        db.set_setting("common_config_claude", "local-credential-canary")
            .unwrap();
        let (before, _) = db.export_sync_snapshot().unwrap();
        for use_s3 in [false, true] {
            let remote = TestRemote::new(use_s3).await;
            let seed = build_local_snapshot(&db).unwrap();
            let path =
                format!("sync/v{PROTOCOL_VERSION}/db-v{DB_COMPAT_VERSION}/test/{REMOTE_MANIFEST}");
            let etag = remote
                .publish(&path, &seed.manifest_bytes, &PutCondition::Absent)
                .await
                .unwrap()
                .unwrap();
            remote.state.lock().unwrap().race_on_manifest_publish = true;
            let status = crate::settings::WebDavSyncStatus {
                last_remote_manifest_hash: Some(seed.manifest_hash.clone()),
                last_remote_etag: Some(etag),
                ..Default::default()
            };
            let error = if use_s3 {
                let mut settings = crate::settings::S3SyncSettings {
                    endpoint: remote.endpoint.clone(),
                    bucket: "bucket".into(),
                    region: "test".into(),
                    access_key_id: "test".into(),
                    secret_access_key: "test".into(),
                    remote_root: "sync".into(),
                    profile: "test".into(),
                    status: status.clone(),
                    ..Default::default()
                };
                let result =
                    run_with_sync_lock(super::super::s3_sync::upload(&db, &mut settings)).await;
                assert_eq!(
                    serde_json::to_value(&settings.status).unwrap(),
                    serde_json::to_value(&status).unwrap()
                );
                result.unwrap_err()
            } else {
                let mut settings = crate::settings::WebDavSyncSettings {
                    base_url: remote.endpoint.clone(),
                    username: "test".into(),
                    password: "test".into(),
                    remote_root: "sync".into(),
                    profile: "test".into(),
                    status: status.clone(),
                    ..Default::default()
                };
                let result =
                    run_with_sync_lock(super::super::webdav_sync::upload(&db, &mut settings)).await;
                assert_eq!(
                    serde_json::to_value(&settings.status).unwrap(),
                    serde_json::to_value(&status).unwrap()
                );
                result.unwrap_err()
            };
            assert!(
                matches!(
                    error,
                    AppError::Localized {
                        key: "sync.conflict",
                        ..
                    }
                ),
                "{error}"
            );
            assert_eq!(std::fs::read(&skill).unwrap(), b"local skill");
            let (after, metadata) = db.export_sync_snapshot().unwrap();
            assert_eq!(metadata, *context.metadata());
            assert!(before.lines().skip(2).eq(after.lines().skip(2)));
        }
    }

    #[tokio::test]
    async fn publication_probes_reject_backends_that_ignore_conditions() {
        for use_s3 in [false, true] {
            let remote = TestRemote::new(use_s3).await;
            remote.artifact("probe", b"immutable").await.unwrap();
            verify_write_conditions(|condition| {
                let remote = &remote;
                async move { remote.publish("probe", b"immutable", &condition).await }
            })
            .await
            .unwrap();
            remote.state.lock().unwrap().ignore_conditions = true;
            assert!(verify_write_conditions(|condition| {
                let remote = &remote;
                async move { remote.publish("probe", b"immutable", &condition).await }
            })
            .await
            .is_err());
            assert_eq!(remote.read("probe").await, b"immutable");
        }
    }

    #[test]
    fn publication_admission_rejects_stale_or_unversioned_heads() {
        let hash = sha256_hex(b"current");
        assert!(publication_condition(Some((b"newer", Some("\"2\""))), Some(&hash)).is_err());
        assert!(publication_condition(Some((b"current", None)), Some(&hash)).is_err());
        assert!(publication_condition(Some((b"current", Some("W/\"2\""))), Some(&hash)).is_err());
        assert!(publication_condition(None, Some(&hash)).is_err());
        assert!(publication_condition(Some((b"current", Some("\"2\""))), None).is_err());
        assert!(matches!(
            publication_condition(None, None).unwrap(),
            PutCondition::Absent
        ));
        assert!(matches!(
            publication_condition(Some((b"current", Some("\"2\""))), Some(&hash)).unwrap(),
            PutCondition::Match(_)
        ));
    }

    #[test]
    fn publication_artifact_paths_bind_downloads_to_manifest_content() {
        let artifacts = BTreeMap::from([(
            REMOTE_DB_SQL.into(),
            ArtifactMeta {
                sha256: sha256_hex(b"a"),
                size: 1,
            },
        )]);
        let mut manifest = SyncManifest {
            format: PROTOCOL_FORMAT.into(),
            version: PROTOCOL_VERSION,
            db_compat_version: Some(DB_COMPAT_VERSION),
            device_name: "test".into(),
            created_at: "test".into(),
            snapshot_id: compute_snapshot_id(&artifacts, &test_vault_metadata()),
            vault: Some(test_vault_metadata()),
            artifacts,
        };
        let first = snapshot_artifact_path(&manifest, REMOTE_DB_SQL).unwrap();
        manifest.artifacts.get_mut(REMOTE_DB_SQL).unwrap().sha256 = sha256_hex(b"b");
        assert!(snapshot_artifact_path(&manifest, REMOTE_DB_SQL).is_err());
        manifest.snapshot_id = compute_snapshot_id(&manifest.artifacts, &test_vault_metadata());
        assert_ne!(
            snapshot_artifact_path(&manifest, REMOTE_DB_SQL).unwrap(),
            first
        );
        manifest.snapshot_id = "../../manifest.json".into();
        assert!(snapshot_artifact_path(&manifest, REMOTE_DB_SQL).is_err());
    }
}

#[cfg(test)]
mod adoption_tests {
    use super::*;
    use crate::{
        database::{vault, Database},
        secrets::{files::CredentialFile, session::SecretSession, testing::MemoryKeyStore},
    };
    use std::{io::Write, path::Path};

    struct TestHome {
        previous: Option<std::ffi::OsString>,
        directory: tempfile::TempDir,
    }
    impl TestHome {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", directory.path());
            Self {
                previous,
                directory,
            }
        }
        fn root(&self) -> std::path::PathBuf {
            self.directory.path().join(crate::config::APP_DIR_NAME)
        }
    }
    impl Drop for TestHome {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn database(root: &Path, store: &MemoryKeyStore, password: Option<&str>) -> Database {
        let secrets = SecretSession::open(root, store, password).unwrap();
        let conn = vault::prepare(
            &root.join(crate::config::DB_FILE_NAME),
            &secrets.read().unwrap(),
        )
        .unwrap();
        secrets.complete_migration().unwrap();
        Database::from_connection(conn, secrets)
    }

    fn snapshot(db: &Database) -> DownloadedSnapshot {
        let (sql, metadata) = db.export_sync_snapshot().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "cloud-skill/SKILL.md",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"cloud skill content").unwrap();
        let skills_zip = zip.finish().unwrap().into_inner();
        let db_sql = sql.into_bytes();
        let artifacts = BTreeMap::from([
            (
                REMOTE_DB_SQL.into(),
                ArtifactMeta {
                    sha256: sha256_hex(&db_sql),
                    size: db_sql.len() as u64,
                },
            ),
            (
                REMOTE_SKILLS_ZIP.into(),
                ArtifactMeta {
                    sha256: sha256_hex(&skills_zip),
                    size: skills_zip.len() as u64,
                },
            ),
        ]);
        let manifest = SyncManifest {
            format: PROTOCOL_FORMAT.into(),
            version: PROTOCOL_VERSION,
            db_compat_version: Some(DB_COMPAT_VERSION),
            device_name: "source device".into(),
            created_at: "test".into(),
            snapshot_id: compute_snapshot_id(&artifacts, &metadata),
            artifacts,
            vault: Some(metadata),
        };
        DownloadedSnapshot {
            manifest_hash: sha256_hex(&serde_json::to_vec(&manifest).unwrap()),
            manifest,
            etag: Some("\"1\"".into()),
            db_sql,
            skills_zip,
            layout: RemoteLayout::Current,
            source_path: "test".into(),
        }
    }

    #[test]
    #[serial_test::serial]
    fn explicit_restore_adopts_source_and_retains_local_credentials_and_automatic_unlock() {
        let home = TestHome::new();
        let store = MemoryKeyStore::default();
        let source = database(
            &home.directory.path().join("source"),
            &store,
            Some("source recovery password"),
        );
        let provider = crate::provider::Provider::with_id(
            "cloud-provider".into(),
            "Cloud provider".into(),
            serde_json::json!({"api_key":"cloud-provider-canary"}),
            None,
        );
        source.save_provider("claude", &provider).unwrap();
        let incoming = snapshot(&source);
        let local = database(&home.root(), &store, None);
        CredentialFile::Codex
            .write(&local.secrets, br#"{"access_token":"local-oauth-canary"}"#)
            .unwrap();
        {
            let context = local.secrets.read().unwrap();
            let conn = local.conn.lock().unwrap();
            crate::vendor::creds::save_account(
                &conn,
                &context,
                crate::vendor::Vendor::DeepSeek,
                "local-vendor-canary",
                &crate::vendor::VendorAccount {
                    account_id: "local-account".into(),
                    label: "Local account".into(),
                    login_identifier: "local-login".into(),
                },
            )
            .unwrap();
            let mut settings = crate::settings::AppSettings::default();
            settings.webdav_sync = Some(crate::settings::WebDavSyncSettings {
                base_url: "https://sync.example.invalid".into(),
                username: "local-user".into(),
                password: "local-sync-canary".into(),
                ..Default::default()
            });
            let bytes = crate::settings::encode_settings_with_vault(&settings, &context).unwrap();
            crate::secrets::session::write_durable(&crate::settings::settings_path(), &bytes)
                .unwrap();
        }
        crate::settings::unlock_settings_for_test(local.secrets.clone()).unwrap();
        let skills = crate::services::skill::SkillService::resolve_ssot_dir();
        assert!(skills.starts_with(home.directory.path()));
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("previous.txt"), b"previous skill").unwrap();
        restore_from_sync(
            &local,
            &incoming,
            &incoming.manifest.snapshot_id,
            "source recovery password",
            &store,
        )
        .unwrap();
        assert_eq!(
            local
                .get_provider_by_id("cloud-provider", "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            provider.settings_config
        );
        assert_eq!(
            &**CredentialFile::Codex.read(&local.secrets).unwrap().unwrap(),
            br#"{"access_token":"local-oauth-canary"}"#
        );
        let current = local.secrets.read().unwrap();
        let settings = crate::settings::decode_settings_with_vault(
            &std::fs::read(crate::settings::settings_path()).unwrap(),
            &current,
        )
        .unwrap();
        assert_eq!(settings.webdav_sync.unwrap().password, "local-sync-canary");
        assert_eq!(
            crate::vendor::creds::list(&local.conn.lock().unwrap(), &current).unwrap()[0]
                .auth_token,
            "local-vendor-canary"
        );
        Database::validate_sync_source(&current, incoming.manifest.vault_metadata().unwrap())
            .unwrap();
        assert_eq!(
            current.metadata(),
            incoming.manifest.vault_metadata().unwrap()
        );
        drop(current);
        assert!(
            crate::secrets::session::read_metadata(local.secrets.root())
                .unwrap()
                .automatic_unlock
        );
        assert!(SecretSession::open_existing(local.secrets.root(), &store, None).is_ok());
        assert_eq!(
            std::fs::read(skills.join("cloud-skill/SKILL.md")).unwrap(),
            b"cloud skill content"
        );
        assert!(!skills.join("previous.txt").exists());
        apply_snapshot_with_store(
            &local,
            &incoming.manifest,
            &incoming.db_sql,
            &incoming.skills_zip,
            &store,
        )
        .unwrap();
        assert_eq!(
            local
                .get_provider_by_id("cloud-provider", "claude")
                .unwrap()
                .unwrap()
                .settings_config,
            provider.settings_config
        );
    }

    #[test]
    #[serial_test::serial]
    fn explicit_restore_accepts_a_new_key_for_the_same_vault() {
        let home = TestHome::new();
        let store = MemoryKeyStore::default();
        let local = database(&home.root(), &store, None);
        let next = local
            .secrets
            .read()
            .unwrap()
            .rotate_key()
            .unwrap()
            .with_password("rotated recovery password")
            .unwrap();
        let source_root = home.directory.path().join("source");
        let source = Database::from_connection(
            vault::prepare(&source_root.join(crate::config::DB_FILE_NAME), &next).unwrap(),
            SecretSession::from_context(source_root, next.clone()),
        );
        source
            .set_setting("common_config_claude", "rotated-source-canary")
            .unwrap();
        let incoming = snapshot(&source);
        assert!(Database::validate_sync_source(
            &local.secrets.read().unwrap(),
            incoming.manifest.vault_metadata().unwrap()
        )
        .is_err());
        restore_from_sync(
            &local,
            &incoming,
            &incoming.manifest.snapshot_id,
            "rotated recovery password",
            &store,
        )
        .unwrap();
        assert_eq!(
            local
                .get_setting("common_config_claude")
                .unwrap()
                .as_deref(),
            Some("rotated-source-canary")
        );
        assert_eq!(local.secrets.read().unwrap().metadata(), next.metadata());
        assert!(SecretSession::open_existing(local.secrets.root(), &store, None).is_ok());
    }

    #[test]
    #[serial_test::serial]
    fn explicit_restore_rejects_password_conflict_and_forgery_before_mutation() {
        let home = TestHome::new();
        let store = MemoryKeyStore::default();
        let source = database(
            &home.directory.path().join("source"),
            &store,
            Some("source recovery password"),
        );
        let mut incoming = snapshot(&source);
        let local = database(&home.root(), &store, None);
        local.set_setting("local-sentinel", "unchanged").unwrap();
        let metadata_before = std::fs::read(local.secrets.root().join("vault.json")).unwrap();
        let raw_before =
            std::fs::read(local.secrets.root().join(crate::config::DB_FILE_NAME)).unwrap();
        assert!(restore_from_sync(
            &local,
            &incoming,
            &incoming.manifest.snapshot_id,
            "incorrect password",
            &store
        )
        .is_err());
        assert!(restore_from_sync(
            &local,
            &incoming,
            "different-snapshot",
            "source recovery password",
            &store
        )
        .is_err());
        incoming.manifest.vault.as_mut().unwrap().revision += 1;
        incoming.manifest.snapshot_id = compute_snapshot_id(
            &incoming.manifest.artifacts,
            incoming.manifest.vault_metadata().unwrap(),
        );
        assert!(restore_from_sync(
            &local,
            &incoming,
            &incoming.manifest.snapshot_id,
            "source recovery password",
            &store
        )
        .is_err());
        assert_eq!(
            std::fs::read(local.secrets.root().join("vault.json")).unwrap(),
            metadata_before
        );
        assert_eq!(
            std::fs::read(local.secrets.root().join(crate::config::DB_FILE_NAME)).unwrap(),
            raw_before
        );
        assert_eq!(
            local.get_setting("local-sentinel").unwrap().as_deref(),
            Some("unchanged")
        );
        assert!(!local.secrets.root().join("skills").exists());
        assert!(!local.secrets.root().join(".vault-transition").exists());
    }
}
