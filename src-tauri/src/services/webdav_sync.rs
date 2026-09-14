//! WebDAV immutable snapshot sync protocol with DB compatibility subdirectories.
//!
//! Implements manifest-based synchronization on top of the HTTP transport
//! primitives in [`super::webdav`]. Artifact set: `db.sql` + `skills.zip`.

use chrono::Utc;
use serde_json::Value;

use crate::error::AppError;
use crate::services::webdav::{
    auth_from_credentials, build_remote_url, ensure_remote_directories, get_bytes, path_segments,
    put_bytes, put_bytes_conditional, test_connection, WebDavAuth,
};
use crate::settings::{update_webdav_sync_status, WebDavSyncSettings, WebDavSyncStatus};

pub(crate) use super::sync_protocol::run_with_sync_lock;
use super::sync_protocol::{
    apply_downloaded_snapshot, build_local_snapshot, effective_db_compat_version, localized,
    persist_sync_success_best_effort, publication_condition, publication_conflict, sha256_hex,
    snapshot_artifact_path, validate_artifact_size_limit, validate_manifest_compat,
    validate_upload_metadata, verify_artifact, verify_write_conditions, DownloadedSnapshot,
    RemoteLayout, SyncManifest, DB_COMPAT_VERSION, LEGACY_PROTOCOL_VERSION, MAX_MANIFEST_BYTES,
    MAX_SYNC_ARTIFACT_BYTES, PROTOCOL_VERSION, REMOTE_DB_SQL, REMOTE_MANIFEST, REMOTE_SKILLS_ZIP,
};

#[cfg(test)]
pub(crate) fn sync_mutex() -> &'static tokio::sync::Mutex<()> {
    super::sync_protocol::sync_mutex()
}

pub(crate) mod archive;

struct RemoteSnapshot {
    layout: RemoteLayout,
    manifest: SyncManifest,
    manifest_bytes: Vec<u8>,
    manifest_etag: Option<String>,
}
// ─── Public API ──────────────────────────────────────────────

/// Check WebDAV connectivity and ensure remote directory structure.
pub async fn check_connection(settings: &WebDavSyncSettings) -> Result<(), AppError> {
    settings.validate()?;
    let auth = auth_for(settings);
    test_connection(&settings.base_url, &auth).await?;
    let dir_segs = remote_dir_segments(settings, RemoteLayout::Current);
    ensure_remote_directories(&settings.base_url, &dir_segs, &auth).await?;
    Ok(())
}

/// Upload local snapshot (db + skills) to remote.
pub async fn upload(
    db: &crate::database::Database,
    settings: &mut WebDavSyncSettings,
) -> Result<Value, AppError> {
    settings.validate()?;
    let snapshot = build_local_snapshot(db)?;
    let manifest: SyncManifest =
        serde_json::from_slice(&snapshot.manifest_bytes).map_err(|source| AppError::Json {
            path: REMOTE_MANIFEST.into(),
            source,
        })?;

    let auth = auth_for(settings);
    let dir_segs = remote_dir_segments(settings, RemoteLayout::Current);
    ensure_remote_directories(&settings.base_url, &dir_segs, &auth).await?;

    let manifest_url = remote_file_url(settings, RemoteLayout::Current, REMOTE_MANIFEST)?;
    let remote = get_bytes(&manifest_url, &auth, MAX_MANIFEST_BYTES).await?;
    let condition = publication_condition(
        remote
            .as_ref()
            .map(|(bytes, etag)| (bytes.as_slice(), etag.as_deref())),
        settings.status.last_remote_manifest_hash.as_deref(),
    )?;
    if let Some((bytes, _)) = &remote {
        let remote_manifest: SyncManifest =
            serde_json::from_slice(bytes).map_err(|source| AppError::Json {
                path: REMOTE_MANIFEST.into(),
                source,
            })?;
        validate_manifest_compat(&remote_manifest, RemoteLayout::Current)?;
        validate_upload_metadata(
            manifest.vault_metadata()?,
            remote_manifest.vault_metadata()?,
        )?;
    }
    let db_path = snapshot_artifact_path(&manifest, REMOTE_DB_SQL)?;
    let mut snapshot_dir = dir_segs;
    snapshot_dir.extend(["snapshots".into(), manifest.snapshot_id.clone()]);
    ensure_remote_directories(&settings.base_url, &snapshot_dir, &auth).await?;
    let db_url = remote_file_url(settings, RemoteLayout::Current, &db_path)?;
    put_bytes(&db_url, &auth, snapshot.db_sql.clone(), "application/sql").await?;
    verify_write_conditions(|condition| {
        let bytes = snapshot.db_sql.clone();
        let url = &db_url;
        let auth = &auth;
        async move { put_bytes_conditional(url, auth, bytes, "application/sql", &condition).await }
    })
    .await?;
    let skills_url = remote_file_url(
        settings,
        RemoteLayout::Current,
        &snapshot_artifact_path(&manifest, REMOTE_SKILLS_ZIP)?,
    )?;
    put_bytes(&skills_url, &auth, snapshot.skills_zip, "application/zip").await?;
    let etag = put_bytes_conditional(
        &manifest_url,
        &auth,
        snapshot.manifest_bytes,
        "application/json",
        &condition,
    )
    .await?
    .ok_or_else(publication_conflict)?;

    let _persisted = persist_sync_success_best_effort(
        settings,
        snapshot.manifest_hash,
        Some(etag),
        persist_sync_success,
    );
    Ok(serde_json::json!({ "status": "uploaded" }))
}

/// Download remote snapshot and apply to local database + skills.
pub(crate) async fn fetch_snapshot(
    settings: &WebDavSyncSettings,
) -> Result<DownloadedSnapshot, AppError> {
    settings.validate()?;
    let auth = auth_for(settings);
    let snapshot = find_remote_snapshot(settings, &auth)
        .await?
        .ok_or_else(|| {
            localized(
                "webdav.sync.remote_empty",
                "远端没有可下载的同步数据",
                "No downloadable sync data found on the remote.",
            )
        })?;

    validate_manifest_compat(&snapshot.manifest, snapshot.layout)?;

    // Download and verify artifacts
    let db_sql = download_and_verify(
        settings,
        &auth,
        snapshot.layout,
        REMOTE_DB_SQL,
        &snapshot.manifest,
    )
    .await?;
    let skills_zip = download_and_verify(
        settings,
        &auth,
        snapshot.layout,
        REMOTE_SKILLS_ZIP,
        &snapshot.manifest,
    )
    .await?;

    Ok(DownloadedSnapshot {
        manifest_hash: sha256_hex(&snapshot.manifest_bytes),
        etag: snapshot.manifest_etag,
        source_path: remote_dir_display(settings, snapshot.layout),
        layout: snapshot.layout,
        manifest: snapshot.manifest,
        db_sql,
        skills_zip,
    })
}

pub async fn download(
    db: &std::sync::Arc<crate::database::Database>,
    settings: &mut WebDavSyncSettings,
) -> Result<Value, AppError> {
    let snapshot = fetch_snapshot(settings).await?;
    let snapshot = apply_downloaded_snapshot(db.clone(), snapshot).await?;
    persist_download_success(settings, &snapshot);
    Ok(
        serde_json::json!({ "status": "downloaded", "sourceLayout": snapshot.layout.as_str(), "sourcePath": snapshot.source_path }),
    )
}

pub(crate) fn persist_download_success(
    settings: &mut WebDavSyncSettings,
    snapshot: &DownloadedSnapshot,
) {
    let _persisted = persist_sync_success_best_effort(
        settings,
        snapshot.manifest_hash.clone(),
        snapshot.etag.clone(),
        persist_sync_success,
    );
}

/// Fetch remote manifest info without downloading artifacts.
pub async fn fetch_remote_info(settings: &WebDavSyncSettings) -> Result<Option<Value>, AppError> {
    settings.validate()?;
    let auth = auth_for(settings);
    let Some(snapshot) = find_remote_snapshot(settings, &auth).await? else {
        return Ok(None);
    };
    let compatible = validate_manifest_compat(&snapshot.manifest, snapshot.layout).is_ok();
    let db_compat_version = effective_db_compat_version(&snapshot.manifest, snapshot.layout);

    let payload = serde_json::json!({
        "deviceName": snapshot.manifest.device_name,
        "createdAt": snapshot.manifest.created_at,
        "snapshotId": snapshot.manifest.snapshot_id,
        "version": snapshot.manifest.version,
        "protocolVersion": snapshot.manifest.version,
        "dbCompatVersion": db_compat_version,
        "compatible": compatible,
        "artifacts": snapshot.manifest.artifacts.keys().collect::<Vec<_>>(),
        "layout": snapshot.layout.as_str(),
        "remotePath": remote_dir_display(settings, snapshot.layout),
    });

    Ok(Some(payload))
}

// ─── Sync status persistence ─────────────────────────────────

fn persist_sync_success(
    settings: &mut WebDavSyncSettings,
    manifest_hash: String,
    etag: Option<String>,
) -> Result<(), AppError> {
    let status = WebDavSyncStatus {
        last_sync_at: Some(Utc::now().timestamp()),
        last_error: None,
        last_error_source: None,
        last_local_manifest_hash: Some(manifest_hash.clone()),
        last_remote_manifest_hash: Some(manifest_hash),
        last_remote_etag: etag,
    };
    settings.status = status.clone();
    update_webdav_sync_status(status)
}

async fn find_remote_snapshot(
    settings: &WebDavSyncSettings,
    auth: &WebDavAuth,
) -> Result<Option<RemoteSnapshot>, AppError> {
    if let Some(snapshot) = fetch_remote_snapshot(settings, auth, RemoteLayout::Current).await? {
        return Ok(Some(snapshot));
    }
    fetch_remote_snapshot(settings, auth, RemoteLayout::Legacy).await
}

async fn fetch_remote_snapshot(
    settings: &WebDavSyncSettings,
    auth: &WebDavAuth,
    layout: RemoteLayout,
) -> Result<Option<RemoteSnapshot>, AppError> {
    let manifest_url = remote_file_url(settings, layout, REMOTE_MANIFEST)?;
    let Some((manifest_bytes, manifest_etag)) =
        get_bytes(&manifest_url, auth, MAX_MANIFEST_BYTES).await?
    else {
        return Ok(None);
    };

    let manifest: SyncManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| AppError::Json {
            path: REMOTE_MANIFEST.to_string(),
            source: e,
        })?;

    Ok(Some(RemoteSnapshot {
        layout,
        manifest,
        manifest_bytes,
        manifest_etag,
    }))
}
// ─── Download & verify ───────────────────────────────────────

async fn download_and_verify(
    settings: &WebDavSyncSettings,
    auth: &WebDavAuth,
    layout: RemoteLayout,
    artifact_name: &str,
    manifest: &SyncManifest,
) -> Result<Vec<u8>, AppError> {
    let meta = manifest.artifacts.get(artifact_name).ok_or_else(|| {
        localized(
            "webdav.sync.manifest_missing_artifact",
            format!("manifest 中缺少 artifact: {artifact_name}"),
            format!("Manifest missing artifact: {artifact_name}"),
        )
    })?;
    validate_artifact_size_limit(artifact_name, meta.size)?;

    let path = if layout == RemoteLayout::Current {
        snapshot_artifact_path(manifest, artifact_name)?
    } else {
        artifact_name.into()
    };
    let url = remote_file_url(settings, layout, &path)?;
    let (bytes, _) = get_bytes(&url, auth, MAX_SYNC_ARTIFACT_BYTES as usize)
        .await?
        .ok_or_else(|| {
            localized(
                "webdav.sync.remote_missing_artifact",
                format!("远端缺少 artifact 文件: {artifact_name}"),
                format!("Remote artifact file missing: {artifact_name}"),
            )
        })?;

    verify_artifact(&bytes, artifact_name, meta)?;
    Ok(bytes)
}

// ─── Remote path helpers ─────────────────────────────────────

fn remote_dir_segments(settings: &WebDavSyncSettings, layout: RemoteLayout) -> Vec<String> {
    let mut segs = Vec::new();
    segs.extend(path_segments(&settings.remote_root).map(str::to_string));
    segs.push(format!(
        "v{}",
        if layout == RemoteLayout::Legacy {
            LEGACY_PROTOCOL_VERSION
        } else {
            PROTOCOL_VERSION
        }
    ));
    if layout == RemoteLayout::Current {
        segs.push(format!("db-v{DB_COMPAT_VERSION}"));
    }
    segs.extend(path_segments(&settings.profile).map(str::to_string));
    segs
}

fn remote_file_url(
    settings: &WebDavSyncSettings,
    layout: RemoteLayout,
    file_name: &str,
) -> Result<String, AppError> {
    let mut segs = remote_dir_segments(settings, layout);
    segs.extend(path_segments(file_name).map(str::to_string));
    build_remote_url(&settings.base_url, &segs)
}

fn remote_dir_display(settings: &WebDavSyncSettings, layout: RemoteLayout) -> String {
    let segs = remote_dir_segments(settings, layout);
    format!("/{}", segs.join("/"))
}

fn auth_for(settings: &WebDavSyncSettings) -> WebDavAuth {
    auth_from_credentials(&settings.username, &settings.password)
}

// ─── Tests ───────────────────────────────────────────────────

pub(crate) struct LegacyCleanupRemote<'a>(pub &'a WebDavSyncSettings);
impl LegacyCleanupRemote<'_> {
    fn url(&self, path: &str) -> Result<String, AppError> {
        build_remote_url(
            &self.0.base_url,
            &path_segments(path).map(str::to_string).collect::<Vec<_>>(),
        )
    }
}
impl super::sync_cleanup::LegacyRemote for LegacyCleanupRemote<'_> {
    fn scope(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.0.base_url, self.0.username, self.0.remote_root, self.0.profile
        )
    }
    fn legacy_roots(&self) -> Vec<(String, u32)> {
        super::sync_cleanup::legacy_roots(&self.0.remote_root, &self.0.profile, true)
    }
    fn current_root(&self) -> String {
        remote_dir_segments(self.0, RemoteLayout::Current).join("/")
    }
    async fn snapshot(&self) -> Result<DownloadedSnapshot, AppError> {
        fetch_snapshot(self.0).await
    }
    async fn get(
        &self,
        path: &str,
        limit: usize,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, AppError> {
        get_bytes(&self.url(path)?, &auth_for(self.0), limit).await
    }
    async fn head(&self, path: &str) -> Result<Option<String>, AppError> {
        super::webdav::head_etag(&self.url(path)?, &auth_for(self.0)).await
    }
    async fn put_probe(&self, path: &str, bytes: Vec<u8>) -> Result<String, AppError> {
        put_bytes_conditional(
            &self.url(path)?,
            &auth_for(self.0),
            bytes,
            "application/octet-stream",
            &super::sync_protocol::PutCondition::Absent,
        )
        .await?
        .ok_or_else(|| AppError::Config("sync.cleanup_changed".into()))
    }
    async fn delete(
        &self,
        path: &str,
        etag: &str,
    ) -> Result<super::sync_cleanup::DeleteResult, AppError> {
        super::webdav::delete_conditional(&self.url(path)?, &auth_for(self.0), etag).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_dir_segments_uses_current_layout() {
        let settings = WebDavSyncSettings {
            remote_root: "cc-switch-sync".to_string(),
            profile: "default".to_string(),
            ..WebDavSyncSettings::default()
        };
        let segs = remote_dir_segments(&settings, RemoteLayout::Current);
        assert_eq!(segs, vec!["cc-switch-sync", "v3", "db-v7", "default"]);
    }

    #[test]
    fn remote_dir_segments_uses_legacy_layout() {
        let settings = WebDavSyncSettings {
            remote_root: "cc-switch-sync".to_string(),
            profile: "default".to_string(),
            ..WebDavSyncSettings::default()
        };
        let segs = remote_dir_segments(&settings, RemoteLayout::Legacy);
        assert_eq!(segs, vec!["cc-switch-sync", "v2", "default"]);
    }
}
