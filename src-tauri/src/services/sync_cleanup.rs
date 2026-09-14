//! Explicit cleanup of this profile's known plaintext snapshot objects.
use super::sync_protocol::{DownloadedSnapshot, SyncManifest};
use crate::{database::Database, error::AppError};
use serde::Serialize;
use std::future::Future;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeleteResult {
    Removed,
    Missing,
    PreconditionFailed,
}

/// 已取回的远端对象：内容与其可选 ETag。
pub(crate) type FetchedRemote = (Vec<u8>, Option<String>);

pub(crate) trait LegacyRemote: Sync {
    fn scope(&self) -> String;
    fn legacy_roots(&self) -> Vec<(String, u32)>;
    fn current_root(&self) -> String;
    fn snapshot(&self) -> impl Future<Output = Result<DownloadedSnapshot, AppError>> + Send;
    fn get(
        &self,
        path: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Option<FetchedRemote>, AppError>> + Send;
    fn head(&self, path: &str) -> impl Future<Output = Result<Option<String>, AppError>> + Send;
    fn put_probe(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<String, AppError>> + Send;
    fn delete(
        &self,
        path: &str,
        etag: &str,
    ) -> impl Future<Output = Result<DeleteResult, AppError>> + Send;
}

/// Known plaintext layouts, scoped to the selected connection profile. S3 never
/// published the older flat WebDAV layout.
pub(crate) fn legacy_roots(root: &str, profile: &str, include_flat: bool) -> Vec<(String, u32)> {
    let protocol = super::sync_protocol::LEGACY_PROTOCOL_VERSION;
    let mut roots = vec![(format!("{root}/v{protocol}/db-v6/{profile}"), 6)];
    if include_flat {
        roots.push((
            format!("{root}/v{protocol}/{profile}"),
            super::sync_protocol::LEGACY_DB_COMPAT_VERSION,
        ));
    }
    roots
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyCleanupPreview {
    pub paths: Vec<String>,
    pub can_clean: bool,
    pub receipt: Option<String>,
    pub blocked_reason: Option<String>,
}

fn error(code: &str) -> AppError {
    AppError::Config(code.into())
}

#[derive(Serialize)]
struct Candidate {
    path: String,
    etag: String,
}
struct Plan {
    objects: Vec<Candidate>,
    head_hash: String,
    head_etag: String,
    receipt: String,
}

async fn legacy_objects(remote: &impl LegacyRemote) -> Result<Vec<Candidate>, AppError> {
    use super::sync_protocol::{require_strong_etag, MAX_MANIFEST_BYTES, PROTOCOL_FORMAT};
    let mut objects = Vec::new();
    for (root, db_version) in remote.legacy_roots() {
        let path = format!("{root}/manifest.json");
        let Some((bytes, etag)) = remote.get(&path, MAX_MANIFEST_BYTES).await? else {
            for name in ["db.sql", "skills.zip"] {
                if remote.head(&format!("{root}/{name}")).await?.is_some() {
                    return Err(error("sync.cleanup_unrecognized_legacy"));
                }
            }
            continue;
        };
        let manifest: SyncManifest = serde_json::from_slice(&bytes)
            .map_err(|_| error("sync.cleanup_unrecognized_legacy"))?;
        if manifest.format != PROTOCOL_FORMAT
            || manifest.version != super::sync_protocol::LEGACY_PROTOCOL_VERSION
            || manifest.db_compat_version.unwrap_or(5) != db_version
            || !manifest.artifacts.contains_key("db.sql")
            || !manifest.artifacts.contains_key("skills.zip")
        {
            return Err(error("sync.cleanup_unrecognized_legacy"));
        }
        let manifest_etag = require_strong_etag(etag.as_deref())?;
        for name in ["db.sql", "skills.zip"] {
            let path = format!("{root}/{name}");
            if let Some(etag) = remote.head(&path).await? {
                objects.push(Candidate {
                    path,
                    etag: require_strong_etag(Some(&etag))?,
                });
            }
        }
        // Keep the identifying manifest until its data objects are gone, so a
        // failed operation can re-identify and retry only the remaining files.
        objects.push(Candidate {
            path,
            etag: manifest_etag,
        });
    }
    Ok(objects)
}

async fn recoverable_head(
    db: &Database,
    remote: &impl LegacyRemote,
) -> Result<(String, String), AppError> {
    use super::sync_protocol::{
        require_strong_etag, validate_manifest_compat, verify_artifact, RemoteLayout,
        REMOTE_DB_SQL, REMOTE_SKILLS_ZIP,
    };
    let snapshot = remote.snapshot().await?;
    let hash = snapshot.manifest_hash.clone();
    let etag = require_strong_etag(snapshot.etag.as_deref())?;
    let current = db.secrets.read()?.clone();
    tokio::task::spawn_blocking(move || {
        validate_manifest_compat(&snapshot.manifest, RemoteLayout::Current)?;
        let metadata = snapshot.manifest.vault_metadata()?;
        if metadata != current.metadata() {
            return Err(error("sync.cleanup_encrypted_backup_required"));
        }
        let source = crate::secrets::VaultContext::from_key(metadata.clone(), current.export_key())
            .map_err(crate::secrets::inventory::secret_error)?;
        for (name, bytes) in [
            (REMOTE_DB_SQL, &snapshot.db_sql),
            (REMOTE_SKILLS_ZIP, &snapshot.skills_zip),
        ] {
            let artifact = snapshot
                .manifest
                .artifacts
                .get(name)
                .ok_or_else(|| error("sync.cleanup_encrypted_backup_required"))?;
            verify_artifact(bytes, name, artifact)?;
        }
        let sql = std::str::from_utf8(&snapshot.db_sql)
            .map_err(|_| error("sync.cleanup_encrypted_backup_required"))?;
        Database::validate_sync_snapshot(sql, metadata, &source)?;
        let _skills = super::webdav_sync::archive::stage_skills_zip(&snapshot.skills_zip)?;
        Ok(())
    })
    .await
    .map_err(|_| error("sync.cleanup_encrypted_backup_required"))??;
    Ok((hash, etag))
}

fn plan(
    remote: &impl LegacyRemote,
    objects: Vec<Candidate>,
    head_hash: String,
    head_etag: String,
) -> Result<Plan, AppError> {
    let receipt = super::sync_protocol::sha256_hex(
        &serde_json::to_vec(&(remote.scope(), &objects, &head_hash, &head_etag))
            .map_err(|_| error("sync.cleanup_invalid_receipt"))?,
    );
    Ok(Plan {
        objects,
        head_hash,
        head_etag,
        receipt,
    })
}

/// Read-only detection never publishes probes or changes local/remote settings.
pub(crate) async fn preview(
    db: &Database,
    remote: &impl LegacyRemote,
) -> Result<LegacyCleanupPreview, AppError> {
    let objects = legacy_objects(remote).await?;
    let paths = objects
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    if objects.is_empty() {
        return Ok(LegacyCleanupPreview {
            paths,
            can_clean: false,
            receipt: None,
            blocked_reason: None,
        });
    }
    match recoverable_head(db, remote).await {
        Ok((hash, etag)) => {
            let plan = plan(remote, objects, hash, etag)?;
            Ok(LegacyCleanupPreview {
                paths,
                can_clean: true,
                receipt: Some(plan.receipt),
                blocked_reason: None,
            })
        }
        Err(_) => Ok(LegacyCleanupPreview {
            paths,
            can_clean: false,
            receipt: None,
            blocked_reason: Some("sync.cleanup_encrypted_backup_required".into()),
        }),
    }
}

async fn verify_delete_conditions(remote: &impl LegacyRemote) -> Result<(), AppError> {
    let path = format!(
        "{}/.cleanup-probe-{}",
        remote.current_root(),
        uuid::Uuid::new_v4()
    );
    let etag = remote
        .put_probe(&path, uuid::Uuid::new_v4().as_bytes().to_vec())
        .await?;
    let wrong = format!("\"{}\"", uuid::Uuid::new_v4());
    let rejected = remote.delete(&path, &wrong).await? == DeleteResult::PreconditionFailed;
    let retained = remote.head(&path).await?.as_deref() == Some(etag.as_str());
    // Only the random, non-secret probe may be touched before capability proof.
    let removed = if retained {
        remote.delete(&path, &etag).await? == DeleteResult::Removed
    } else {
        false
    };
    if !rejected || !retained || !removed || remote.head(&path).await?.is_some() {
        return Err(error("sync.cleanup_conditions_unsupported"));
    }
    Ok(())
}

/// Caller holds the common sync mutex. Client input is an opaque receipt, never
/// paths; each deletion stays bound to this profile and its observed strong ETag.
pub(crate) async fn cleanup(
    db: &Database,
    remote: &impl LegacyRemote,
    expected_receipt: &str,
) -> Result<usize, AppError> {
    let objects = legacy_objects(remote).await?;
    let (hash, etag) = recoverable_head(db, remote)
        .await
        .map_err(|_| error("sync.cleanup_encrypted_backup_required"))?;
    let plan = plan(remote, objects, hash, etag)?;
    if plan.objects.is_empty() || plan.receipt != expected_receipt {
        return Err(error("sync.cleanup_changed"));
    }
    verify_delete_conditions(remote).await?;
    let Some((bytes, etag)) = remote
        .get(
            &format!("{}/manifest.json", remote.current_root()),
            super::sync_protocol::MAX_MANIFEST_BYTES,
        )
        .await?
    else {
        return Err(error("sync.cleanup_changed"));
    };
    if super::sync_protocol::sha256_hex(&bytes) != plan.head_hash
        || etag.as_deref() != Some(plan.head_etag.as_str())
    {
        return Err(error("sync.cleanup_changed"));
    }
    let mut deleted = 0;
    for object in plan.objects {
        match remote.delete(&object.path, &object.etag).await? {
            DeleteResult::PreconditionFailed => return Err(error("sync.cleanup_changed")),
            DeleteResult::Removed | DeleteResult::Missing => {}
        }
        if remote.head(&object.path).await?.is_some() {
            return Err(error("sync.cleanup_incomplete"));
        }
        deleted += 1;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::super::sync_protocol::{
        compute_snapshot_id, sha256_hex, ArtifactMeta, RemoteLayout, PROTOCOL_FORMAT,
        REMOTE_DB_SQL, REMOTE_SKILLS_ZIP,
    };
    use super::*;
    use crate::secrets::{session::SecretSession, VaultContext};
    use std::collections::BTreeMap;
    use std::{io::Write, sync::Mutex};
    struct Remote {
        snapshot: DownloadedSnapshot,
        state: Mutex<State>,
    }
    #[derive(Default)]
    struct State {
        objects: BTreeMap<String, (Vec<u8>, String)>,
        writes: Vec<String>,
        deletes: Vec<String>,
        ignore_delete_conditions: bool,
        fail_skills_once: bool,
        change_head_on_probe: bool,
    }
    impl LegacyRemote for Remote {
        fn scope(&self) -> String {
            "test-remote/profile".into()
        }
        fn legacy_roots(&self) -> Vec<(String, u32)> {
            vec![
                ("root/v2/db-v6/profile".into(), 6),
                ("root/v2/profile".into(), 5),
            ]
        }
        fn current_root(&self) -> String {
            "root/v3/db-v7/profile".into()
        }
        async fn snapshot(&self) -> Result<DownloadedSnapshot, AppError> {
            Ok(self.snapshot.clone())
        }
        async fn get(
            &self,
            path: &str,
            _limit: usize,
        ) -> Result<Option<(Vec<u8>, Option<String>)>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .objects
                .get(path)
                .map(|(bytes, etag)| (bytes.clone(), Some(etag.clone()))))
        }
        async fn head(&self, path: &str) -> Result<Option<String>, AppError> {
            Ok(self
                .state
                .lock()
                .unwrap()
                .objects
                .get(path)
                .map(|(_, etag)| etag.clone()))
        }
        async fn put_probe(&self, path: &str, bytes: Vec<u8>) -> Result<String, AppError> {
            let mut state = self.state.lock().unwrap();
            state.writes.push(path.into());
            if state.change_head_on_probe {
                state.objects.insert(
                    "root/v3/db-v7/profile/manifest.json".into(),
                    (b"changed head".to_vec(), "\"changed\"".into()),
                );
            }
            state
                .objects
                .insert(path.into(), (bytes, "\"probe\"".into()));
            Ok("\"probe\"".into())
        }
        async fn delete(&self, path: &str, etag: &str) -> Result<DeleteResult, AppError> {
            let mut state = self.state.lock().unwrap();
            let Some((_, current)) = state.objects.get(path) else {
                return Ok(DeleteResult::Missing);
            };
            if !state.ignore_delete_conditions && current != etag {
                return Ok(DeleteResult::PreconditionFailed);
            }
            if state.fail_skills_once && path.ends_with("skills.zip") {
                state.fail_skills_once = false;
                return Err(AppError::Config("test.delete_failed".into()));
            }
            state.deletes.push(path.into());
            state.objects.remove(path);
            Ok(DeleteResult::Removed)
        }
    }
    fn fixture() -> (tempfile::TempDir, Database, Remote) {
        let temporary = tempfile::tempdir().unwrap();
        let vault = VaultContext::generate()
            .unwrap()
            .with_password("cleanup recovery password")
            .unwrap();
        let conn =
            crate::database::vault::prepare(&temporary.path().join("source.db"), &vault).unwrap();
        let db = Database::from_connection(
            conn,
            SecretSession::from_context(temporary.path().into(), vault),
        );
        db.set_setting("common_config_claude", "cleanup-canary")
            .unwrap();
        let (sql, metadata) = db.export_sync_snapshot().unwrap();
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("example/SKILL.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"example").unwrap();
        let skills = zip.finish().unwrap().into_inner();
        let artifacts = BTreeMap::from([
            (
                REMOTE_DB_SQL.into(),
                ArtifactMeta {
                    sha256: sha256_hex(sql.as_bytes()),
                    size: sql.len() as u64,
                },
            ),
            (
                REMOTE_SKILLS_ZIP.into(),
                ArtifactMeta {
                    sha256: sha256_hex(&skills),
                    size: skills.len() as u64,
                },
            ),
        ]);
        let manifest = SyncManifest {
            format: PROTOCOL_FORMAT.into(),
            version: 3,
            db_compat_version: Some(7),
            snapshot_id: compute_snapshot_id(&artifacts, &metadata),
            device_name: "test".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            artifacts,
            vault: Some(metadata),
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let snapshot = DownloadedSnapshot {
            manifest: manifest.clone(),
            manifest_hash: sha256_hex(&bytes),
            etag: Some("\"head\"".into()),
            db_sql: sql.into_bytes(),
            skills_zip: skills,
            layout: RemoteLayout::Current,
            source_path: "root/v3/db-v7/profile".into(),
        };
        let mut state = State::default();
        state.objects.insert(
            "root/v3/db-v7/profile/manifest.json".into(),
            (bytes, "\"head\"".into()),
        );
        for (root, version) in [("root/v2/db-v6/profile", 6), ("root/v2/profile", 5)] {
            let mut legacy = manifest.clone();
            legacy.version = 2;
            legacy.db_compat_version = Some(version);
            legacy.vault = None;
            state.objects.insert(
                format!("{root}/manifest.json"),
                (serde_json::to_vec(&legacy).unwrap(), "\"legacy\"".into()),
            );
            state.objects.insert(
                format!("{root}/db.sql"),
                (b"old plaintext".to_vec(), "\"db\"".into()),
            );
            state.objects.insert(
                format!("{root}/skills.zip"),
                (b"old skills".to_vec(), "\"skills\"".into()),
            );
        }
        state.objects.insert(
            "root/v2/db-v6/other/db.sql".into(),
            (b"untouched".to_vec(), "\"other\"".into()),
        );
        (
            temporary,
            db,
            Remote {
                snapshot,
                state: Mutex::new(state),
            },
        )
    }
    #[tokio::test]
    async fn preview_is_read_only_and_cleanup_only_removes_confirmed_legacy_objects() {
        let (_home, db, remote) = fixture();
        let plan = preview(&db, &remote).await.unwrap();
        assert_eq!(plan.paths.len(), 6);
        assert!(plan.can_clean);
        assert!(remote.state.lock().unwrap().writes.is_empty());
        assert_eq!(
            cleanup(&db, &remote, plan.receipt.as_deref().unwrap())
                .await
                .unwrap(),
            6
        );
        let state = remote.state.lock().unwrap();
        assert!(state.objects.contains_key("root/v2/db-v6/other/db.sql"));
        assert!(state
            .objects
            .contains_key("root/v3/db-v7/profile/manifest.json"));
        for root in ["root/v2/db-v6/profile", "root/v2/profile"] {
            let manifest = state
                .deletes
                .iter()
                .position(|path| path == &format!("{root}/manifest.json"))
                .unwrap();
            assert!(
                state
                    .deletes
                    .iter()
                    .position(|path| path == &format!("{root}/db.sql"))
                    .unwrap()
                    < manifest
            );
        }
    }
    #[tokio::test]
    async fn ignored_delete_conditions_only_remove_the_probe() {
        let (_home, db, remote) = fixture();
        let plan = preview(&db, &remote).await.unwrap();
        remote.state.lock().unwrap().ignore_delete_conditions = true;
        assert!(cleanup(&db, &remote, plan.receipt.as_deref().unwrap())
            .await
            .is_err());
        let state = remote.state.lock().unwrap();
        assert!(state
            .deletes
            .iter()
            .all(|path| path.contains(".cleanup-probe-")));
        assert_eq!(
            state
                .objects
                .keys()
                .filter(|path| path.starts_with("root/v2/"))
                .count(),
            7
        );
    }
    #[tokio::test]
    async fn changed_legacy_object_or_head_rejects_before_any_probe_or_delete() {
        let (_home, db, mut remote) = fixture();
        let plan = preview(&db, &remote).await.unwrap();
        remote
            .state
            .lock()
            .unwrap()
            .objects
            .get_mut("root/v2/db-v6/profile/db.sql")
            .unwrap()
            .1 = "\"changed\"".into();
        assert!(cleanup(&db, &remote, plan.receipt.as_deref().unwrap())
            .await
            .is_err());
        assert!(remote.state.lock().unwrap().writes.is_empty());
        let plan = preview(&db, &remote).await.unwrap();
        remote.snapshot.etag = Some("\"new-head\"".into());
        assert!(cleanup(&db, &remote, plan.receipt.as_deref().unwrap())
            .await
            .is_err());
        assert!(remote.state.lock().unwrap().writes.is_empty());
    }
    #[tokio::test]
    async fn partial_failure_retains_the_manifest_and_retry_targets_only_remaining_objects() {
        let (_home, db, remote) = fixture();
        let plan = preview(&db, &remote).await.unwrap();
        remote.state.lock().unwrap().fail_skills_once = true;
        assert!(cleanup(&db, &remote, plan.receipt.as_deref().unwrap())
            .await
            .is_err());
        assert!(remote
            .state
            .lock()
            .unwrap()
            .objects
            .contains_key("root/v2/db-v6/profile/manifest.json"));
        let retry = preview(&db, &remote).await.unwrap();
        assert_eq!(retry.paths.len(), 5);
        assert_eq!(
            cleanup(&db, &remote, retry.receipt.as_deref().unwrap())
                .await
                .unwrap(),
            5
        );
    }
    #[tokio::test]
    async fn changing_the_encrypted_head_during_the_probe_keeps_legacy_objects() {
        let (_home, db, remote) = fixture();
        let plan = preview(&db, &remote).await.unwrap();
        remote.state.lock().unwrap().change_head_on_probe = true;
        assert!(cleanup(&db, &remote, plan.receipt.as_deref().unwrap())
            .await
            .is_err());
        let state = remote.state.lock().unwrap();
        assert!(state
            .deletes
            .iter()
            .all(|path| path.contains(".cleanup-probe-")));
        assert_eq!(
            state
                .objects
                .keys()
                .filter(|path| path.starts_with("root/v2/"))
                .count(),
            7
        );
    }

    #[tokio::test]
    async fn unrecognized_manifests_and_orphan_objects_are_not_cleanup_targets() {
        for orphan in [false, true] {
            let (_home, db, remote) = fixture();
            {
                let mut state = remote.state.lock().unwrap();
                let path = "root/v2/db-v6/profile/manifest.json";
                if orphan {
                    state.objects.remove(path);
                } else {
                    state.objects.get_mut(path).unwrap().0 = b"unrecognized file".to_vec();
                }
            }
            assert!(preview(&db, &remote).await.is_err());
            assert!(cleanup(&db, &remote, "invalid").await.is_err());
            assert!(remote.state.lock().unwrap().writes.is_empty());
            assert!(remote.state.lock().unwrap().deletes.is_empty());
        }
    }

    #[tokio::test]
    async fn a_valid_head_with_a_different_password_wrapper_cannot_authorize_cleanup() {
        let (_home, db, mut remote) = fixture();
        let alternate = db
            .secrets
            .read()
            .unwrap()
            .with_password("different cleanup password")
            .unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(std::str::from_utf8(&remote.snapshot.db_sql).unwrap())
            .unwrap();
        crate::database::vault::stamp(&conn, &alternate).unwrap();
        let sql = Database::dump_sql(&conn, &[]).unwrap();
        Database::validate_sync_snapshot(&sql, alternate.metadata(), &alternate).unwrap();
        remote.snapshot.db_sql = sql.into_bytes();
        remote.snapshot.manifest.artifacts.insert(
            REMOTE_DB_SQL.into(),
            ArtifactMeta {
                sha256: sha256_hex(&remote.snapshot.db_sql),
                size: remote.snapshot.db_sql.len() as u64,
            },
        );
        remote.snapshot.manifest.vault = Some(alternate.metadata().clone());
        remote.snapshot.manifest.snapshot_id =
            compute_snapshot_id(&remote.snapshot.manifest.artifacts, alternate.metadata());
        let bytes = serde_json::to_vec(&remote.snapshot.manifest).unwrap();
        remote.snapshot.manifest_hash = sha256_hex(&bytes);
        remote.state.lock().unwrap().objects.insert(
            "root/v3/db-v7/profile/manifest.json".into(),
            (bytes, "\"head\"".into()),
        );
        let plan = preview(&db, &remote).await.unwrap();
        assert!(!plan.can_clean);
        assert!(plan.receipt.is_none());
        assert!(remote.state.lock().unwrap().writes.is_empty());
    }

    #[tokio::test]
    async fn an_invalid_encrypted_head_never_authorizes_legacy_cleanup() {
        let (_home, db, mut remote) = fixture();
        remote.snapshot.db_sql = b"invalid".to_vec();
        let plan = preview(&db, &remote).await.unwrap();
        assert_eq!(plan.paths.len(), 6);
        assert!(!plan.can_clean);
        assert!(plan.receipt.is_none());
        assert!(cleanup(&db, &remote, "invalid").await.is_err());
        assert!(remote.state.lock().unwrap().writes.is_empty());
    }
}
