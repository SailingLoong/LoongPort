//! Local-only last accepted feedback receipt and its original diagnostics ZIP.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FeedbackSafetySummary {
    pub description_included: bool,
    #[serde(default)]
    pub contact_included: bool,
    pub screenshot_count: u32,
    pub screenshot_bytes: u64,
    pub diagnostics_included: bool,
    pub diagnostics_bytes: u64,
}
/// Only a server-acknowledged identity is retained. A legacy GitHub issue is
/// explicitly labeled rather than masquerading as a new report-server ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FeedbackReceiptIdentity {
    Report { id: String },
    GithubIssue { number: u64 },
}

impl FeedbackReceiptIdentity {
    /// An internal compare-and-swap key. The report namespace excludes colons,
    /// so a legacy issue can never collide with a real report ID.
    fn cas_key(&self) -> String {
        match self {
            Self::Report { id } => id.clone(),
            Self::GithubIssue { number } => format!("github_issue:{number}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptedFeedbackReceipt {
    pub identity: FeedbackReceiptIdentity,
    pub uploaded_at: String,
    pub summary: FeedbackSafetySummary,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LocalFeedbackCopy {
    NotAttached,
    Missing { file_name: &'static str, bytes: u64 },
    Available { file_name: &'static str, bytes: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackReceipt {
    /// Derived CAS key only. Display identity to distinguish report IDs/issues.
    pub report_id: String,
    pub identity: FeedbackReceiptIdentity,
    pub uploaded_at: String,
    pub summary: FeedbackSafetySummary,
    pub local_copy: LocalFeedbackCopy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FeedbackReceiptError {
    LocalSaveFailed,
    LocalReadFailed,
    LocalDeleteFailed,
    ReceiptChanged,
    LocalCopyUnavailable,
}
/// One local envelope owns both the receipt and its original ZIP. Keeping them in
/// one atomic replacement avoids a journal or receipt/ZIP half-commit.
const RECORD_NAME: &str = "last-success.json";
const ZIP_NAME: &str = "diagnostics.zip";
const MAX_ZIP_BYTES: usize = 10 * 1024 * 1024;
const MAX_ENCODED_ZIP_BYTES: usize = MAX_ZIP_BYTES.div_ceil(3) * 4;
// 10 MiB ZIP → 13,981,016 base64 bytes plus a bounded, small receipt.
// Encoding/read validation temporarily holds a few bounded buffers (roughly
// 40–45 MiB at the limit), never screenshots or an additional attachment ZIP.
const MAX_RECORD_BYTES: u64 = 14 * 1024 * 1024;

// The GUI is single-instance. All handles, including newly constructed ones,
// serialize through this owner; a per-instance mutex would make CAS unsafe.
static OWNER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredReceipt {
    version: u8,
    receipt: AcceptedFeedbackReceipt,
    diagnostics_zip: Option<String>,
}

/// Native-only storage: the argument comes from the application's configuration
/// directory owner, never an IPC path. Only two fixed local names are appended.
pub struct FeedbackReceiptStore {
    directory: PathBuf,
}

impl FeedbackReceiptStore {
    pub fn new(app_directory: impl AsRef<Path>) -> Self {
        let app = app_directory.as_ref();
        // Resolve platform-owned parent aliases (e.g. macOS /var) once, without
        // following an alias at the application-owned directory itself.
        let app = if app.is_absolute()
            && !app
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            app.parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
                .zip(app.file_name())
                .map_or_else(|| app.to_path_buf(), |(parent, name)| parent.join(name))
        } else {
            app.to_path_buf()
        };
        Self {
            directory: app.join("feedback"),
        }
    }

    /// Passive read: absent storage is None, with no mkdir, chmod or writes.
    pub fn read_last(&self) -> Result<Option<FeedbackReceipt>, FeedbackReceiptError> {
        let _owner = OWNER
            .lock()
            .map_err(|_| FeedbackReceiptError::LocalReadFailed)?;
        Ok(self.read_stored()?.map(|stored| project(&stored)))
    }

    /// Call only after a verified server acceptance. Upload failure or an unknown
    /// server outcome must never invoke this method. A local error does not undo
    /// server acceptance and must not trigger a second upload.
    pub fn save_accepted(
        &self,
        receipt: AcceptedFeedbackReceipt,
        zip: Option<&[u8]>,
    ) -> Result<FeedbackReceipt, FeedbackReceiptError> {
        self.save_with(receipt, zip, |path, bytes| {
            // The original helper syncs the temporary file BEFORE publication.
            // commit() has no fallible post-rename work: Err preserves the old
            // destination. Do not replace this with write_durable(), which can
            // return an error from fsync AFTER replacing the old destination.
            crate::config_file_io::stage_write(path, bytes, Some(0o600), true)?.commit()
        })
    }

    fn save_with(
        &self,
        receipt: AcceptedFeedbackReceipt,
        zip: Option<&[u8]>,
        publish: impl FnOnce(&Path, &[u8]) -> Result<(), crate::error::AppError>,
    ) -> Result<FeedbackReceipt, FeedbackReceiptError> {
        use base64::Engine;
        let _owner = OWNER
            .lock()
            .map_err(|_| FeedbackReceiptError::LocalSaveFailed)?;
        let stored = (|| {
            validate_receipt(&receipt)?;
            if let Some(bytes) = zip {
                if !receipt.summary.diagnostics_included
                    || bytes.len() > MAX_ZIP_BYTES
                    || bytes.len() as u64 != receipt.summary.diagnostics_bytes
                {
                    return Err(());
                }
            }
            let stored = StoredReceipt {
                version: 1,
                receipt,
                diagnostics_zip: zip
                    .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
            };
            let bytes = serde_json::to_vec(&stored).map_err(|_| ())?;
            if bytes.len() as u64 > MAX_RECORD_BYTES {
                return Err(());
            }
            // Check before creating/tightening the sole application-owned child.
            validate_directories(&self.directory)?;
            let parent = self.directory.parent().ok_or(())?;
            if !parent.is_dir() {
                return Err(());
            }
            crate::config_file_io::ensure_private_directory(&self.directory).map_err(|_| ())?;
            validate_directories(&self.directory)?;
            let path = self.directory.join(RECORD_NAME);
            validate_leaf(&path)?;
            publish(&path, &bytes).map_err(|_| ())?;
            Ok(stored)
        })()
        .map_err(|_| FeedbackReceiptError::LocalSaveFailed)?;
        Ok(project(&stored))
    }

    /// Deletes only this one local record (which contains the local ZIP). There
    /// is no network operation, remote ID URL, or caller-supplied file path.
    pub fn delete_local(&self, expected_report_id: &str) -> Result<(), FeedbackReceiptError> {
        let _owner = OWNER
            .lock()
            .map_err(|_| FeedbackReceiptError::LocalDeleteFailed)?;
        self.read_expected(expected_report_id)?;
        std::fs::remove_file(self.directory.join(RECORD_NAME))
            .map_err(|_| FeedbackReceiptError::LocalDeleteFailed)
    }

    /// Native export gets only the original bytes; the native dialog owns the
    /// eventual destination. No absolute storage path crosses the IPC boundary.
    pub fn export_zip(&self, expected_report_id: &str) -> Result<Vec<u8>, FeedbackReceiptError> {
        let _owner = OWNER
            .lock()
            .map_err(|_| FeedbackReceiptError::LocalReadFailed)?;
        let stored = self.read_expected(expected_report_id)?;
        decode_zip(&stored)
            .map_err(|_| FeedbackReceiptError::LocalReadFailed)?
            .ok_or(FeedbackReceiptError::LocalCopyUnavailable)
    }

    fn read_expected(&self, expected: &str) -> Result<StoredReceipt, FeedbackReceiptError> {
        let stored = self
            .read_stored()?
            .ok_or(FeedbackReceiptError::ReceiptChanged)?;
        if stored.receipt.identity.cas_key() != expected {
            return Err(FeedbackReceiptError::ReceiptChanged);
        }
        Ok(stored)
    }

    fn read_stored(&self) -> Result<Option<StoredReceipt>, FeedbackReceiptError> {
        (|| {
            validate_directories(&self.directory)?;
            let path = self.directory.join(RECORD_NAME);
            validate_leaf(&path)?;
            let Some(bytes) = crate::config_file_io::read_regular_file(&path, MAX_RECORD_BYTES)
                .map_err(|_| ())?
            else {
                return Ok(None);
            };
            let stored: StoredReceipt = serde_json::from_slice(&bytes).map_err(|_| ())?;
            if stored.version != 1 {
                return Err(());
            }
            validate_receipt(&stored.receipt)?;
            // Bounded decoding also rejects malformed payloads and size lies.
            decode_zip(&stored)?;
            validate_directories(&self.directory)?;
            Ok(Some(stored))
        })()
        .map_err(|_| FeedbackReceiptError::LocalReadFailed)
    }
}

fn validate_receipt(receipt: &AcceptedFeedbackReceipt) -> Result<(), ()> {
    let summary = &receipt.summary;
    let valid_identity = match &receipt.identity {
        FeedbackReceiptIdentity::Report { id } => {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }
        // Tauri/JS must represent the accepted positive integer without loss.
        FeedbackReceiptIdentity::GithubIssue { number } => {
            (1..=9_007_199_254_740_991).contains(number)
        }
    };
    if !valid_identity
        || receipt.uploaded_at.is_empty()
        || receipt.uploaded_at.len() > 64
        || chrono::DateTime::parse_from_rfc3339(&receipt.uploaded_at).is_err()
        || summary.screenshot_count > 6
        || (summary.screenshot_count == 0 && summary.screenshot_bytes != 0)
        || (!summary.diagnostics_included && summary.diagnostics_bytes != 0)
        || summary.diagnostics_bytes > MAX_ZIP_BYTES as u64
        || summary
            .screenshot_bytes
            .checked_add(summary.diagnostics_bytes)
            .is_none_or(|total| total > MAX_ZIP_BYTES as u64)
    {
        return Err(());
    }
    Ok(())
}

fn decode_zip(stored: &StoredReceipt) -> Result<Option<Vec<u8>>, ()> {
    use base64::Engine;
    match &stored.diagnostics_zip {
        None => Ok(None),
        Some(encoded) => {
            if !stored.receipt.summary.diagnostics_included || encoded.len() > MAX_ENCODED_ZIP_BYTES
            {
                return Err(());
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| ())?;
            if bytes.len() > MAX_ZIP_BYTES
                || bytes.len() as u64 != stored.receipt.summary.diagnostics_bytes
            {
                return Err(());
            }
            Ok(Some(bytes))
        }
    }
}

/// Project an already-validated acceptance when local persistence failed. This
/// preserves the known identity/time/summary without replacing the saved card
/// or claiming a local ZIP is available. No filesystem or owner-lock access.
pub(crate) fn project_unsaved(receipt: AcceptedFeedbackReceipt) -> FeedbackReceipt {
    project(&StoredReceipt {
        version: 1,
        receipt,
        diagnostics_zip: None,
    })
}

fn project(stored: &StoredReceipt) -> FeedbackReceipt {
    let receipt = &stored.receipt;
    let local_copy = if !receipt.summary.diagnostics_included {
        LocalFeedbackCopy::NotAttached
    } else if stored.diagnostics_zip.is_some() {
        LocalFeedbackCopy::Available {
            file_name: ZIP_NAME,
            bytes: receipt.summary.diagnostics_bytes,
        }
    } else {
        LocalFeedbackCopy::Missing {
            file_name: ZIP_NAME,
            bytes: receipt.summary.diagnostics_bytes,
        }
    };
    FeedbackReceipt {
        report_id: receipt.identity.cas_key(),
        identity: receipt.identity.clone(),
        uploaded_at: receipt.uploaded_at.clone(),
        summary: receipt.summary.clone(),
        local_copy,
    }
}

fn is_alias(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

fn validate_directories(path: &Path) -> Result<(), ()> {
    if !path.is_absolute() {
        return Err(());
    }
    let mut prefix = PathBuf::new();
    for part in path.components() {
        if matches!(part, std::path::Component::ParentDir) {
            return Err(());
        }
        prefix.push(part);
        // A Windows drive/UNC prefix is not a complete directory until its
        // following root component has been appended.
        if matches!(part, std::path::Component::Prefix(_)) {
            continue;
        }
        match std::fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.is_dir() && !is_alias(&metadata) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            _ => return Err(()),
        }
    }
    Ok(())
}

fn validate_leaf(path: &Path) -> Result<(), ()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || is_alias(&metadata) {
                return Err(());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(());
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn accepted(id: &str, zip: Option<&[u8]>) -> AcceptedFeedbackReceipt {
        AcceptedFeedbackReceipt {
            identity: FeedbackReceiptIdentity::Report { id: id.to_string() },
            uploaded_at: "2026-10-18T10:00:00Z".into(),
            summary: FeedbackSafetySummary {
                description_included: true,
                contact_included: false,
                screenshot_count: 1,
                screenshot_bytes: 4,
                diagnostics_included: zip.is_some(),
                diagnostics_bytes: zip.map_or(0, |z| z.len() as u64),
            },
        }
    }
    #[test]
    fn saves_one_atomic_envelope_and_reopens_original_zip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let zip = b"PK\x03\x04synthetic-original-diagnostic";
        let saved = store
            .save_accepted(accepted("report-1", Some(zip)), Some(zip))
            .unwrap();
        assert_eq!(
            saved.local_copy,
            LocalFeedbackCopy::Available {
                file_name: "diagnostics.zip",
                bytes: zip.len() as u64
            }
        );
        let reopened = FeedbackReceiptStore::new(dir.path());
        assert_eq!(reopened.read_last().unwrap(), Some(saved));
        assert_eq!(reopened.export_zip("report-1").unwrap(), zip);
        assert_eq!(
            std::fs::read_dir(dir.path().join("feedback"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::{fs, sync::Arc};
    fn accepted(id: &str, zip: Option<&[u8]>) -> AcceptedFeedbackReceipt {
        AcceptedFeedbackReceipt {
            identity: FeedbackReceiptIdentity::Report { id: id.into() },
            uploaded_at: "2026-10-18T10:00:00.000Z".into(),
            summary: FeedbackSafetySummary {
                description_included: true,
                contact_included: false,
                screenshot_count: 0,
                screenshot_bytes: 0,
                diagnostics_included: zip.is_some(),
                diagnostics_bytes: zip.map_or(0, |z| z.len() as u64),
            },
        }
    }
    #[test]
    fn passive_empty_read_does_not_create_storage() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        assert_eq!(store.read_last().unwrap(), None);
        assert!(!dir.path().join("feedback").exists());
    }
    #[test]
    fn not_attached_has_no_empty_archive_and_removes_previous_zip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        store
            .save_accepted(accepted("old", Some(b"zip")), Some(b"zip"))
            .unwrap();
        let saved = store.save_accepted(accepted("new", None), None).unwrap();
        assert_eq!(saved.local_copy, LocalFeedbackCopy::NotAttached);
        assert_eq!(
            store.export_zip("new"),
            Err(FeedbackReceiptError::LocalCopyUnavailable)
        );
        let json = serde_json::to_value(saved).unwrap();
        assert_eq!(
            json["localCopy"],
            serde_json::json!({"status":"notAttached"})
        );
        assert!(json.get("summary").is_some());
        assert!(json.get("safetySummary").is_none());
    }
    #[test]
    fn missing_zip_has_expected_filename_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let saved = store
            .save_accepted(accepted("accepted", Some(b"zip")), None)
            .unwrap();
        assert_eq!(
            saved.local_copy,
            LocalFeedbackCopy::Missing {
                file_name: "diagnostics.zip",
                bytes: 3
            }
        );
        assert_eq!(
            store.export_zip("accepted"),
            Err(FeedbackReceiptError::LocalCopyUnavailable)
        );
    }
    #[test]
    fn stale_delete_and_export_cannot_affect_new_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        store
            .save_accepted(accepted("old", Some(b"old")), Some(b"old"))
            .unwrap();
        let new = store
            .save_accepted(accepted("new", Some(b"new")), Some(b"new"))
            .unwrap();
        assert_eq!(
            store.delete_local("old"),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(
            store.export_zip("old"),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(store.read_last().unwrap(), Some(new));
        store.delete_local("new").unwrap();
        assert_eq!(store.read_last().unwrap(), None);
        assert_eq!(
            fs::read_dir(dir.path().join("feedback")).unwrap().count(),
            0
        );
    }
    #[test]
    fn invalid_new_record_preserves_last_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let old = store
            .save_accepted(accepted("old", Some(b"old")), Some(b"old"))
            .unwrap();
        let mut invalid = accepted("new", Some(b"new"));
        invalid.summary.diagnostics_bytes += 1;
        assert_eq!(
            store.save_accepted(invalid, Some(b"new")),
            Err(FeedbackReceiptError::LocalSaveFailed)
        );
        assert_eq!(store.read_last().unwrap(), Some(old));
        assert_eq!(store.export_zip("old").unwrap(), b"old");
    }
    #[test]
    fn refuses_over_limit_raw_zip_without_replacing_old() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let old = store.save_accepted(accepted("old", None), None).unwrap();
        let too_big = vec![0; 10 * 1024 * 1024 + 1];
        assert_eq!(
            store.save_accepted(accepted("new", Some(&too_big)), Some(&too_big)),
            Err(FeedbackReceiptError::LocalSaveFailed)
        );
        assert_eq!(store.read_last().unwrap(), Some(old));
    }
    #[test]
    fn unknown_fields_in_summary_are_not_persistable() {
        let mut value = serde_json::to_value(accepted("id", None)).unwrap();
        value["summary"]["description"] = "private text".into();
        assert!(serde_json::from_value::<AcceptedFeedbackReceipt>(value).is_err());
    }
    #[test]
    fn malformed_or_oversized_local_file_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        store.save_accepted(accepted("old", None), None).unwrap();
        let file = fs::read_dir(dir.path().join("feedback"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(&file, b"{broken").unwrap();
        assert_eq!(
            store.read_last(),
            Err(FeedbackReceiptError::LocalReadFailed)
        );
        assert_eq!(
            store.delete_local("old"),
            Err(FeedbackReceiptError::LocalReadFailed)
        );
        let handle = fs::OpenOptions::new().write(true).open(&file).unwrap();
        handle.set_len(14 * 1024 * 1024 + 1).unwrap();
        assert_eq!(
            store.read_last(),
            Err(FeedbackReceiptError::LocalReadFailed)
        );
    }
    #[test]
    fn separate_store_instances_share_atomic_cas_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = Arc::new(dir.path().to_path_buf());
        FeedbackReceiptStore::new(path.as_path())
            .save_accepted(accepted("old", Some(b"old")), Some(b"old"))
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let writer_path = Arc::clone(&path);
        let writer_barrier = Arc::clone(&barrier);
        let writer = std::thread::spawn(move || {
            writer_barrier.wait();
            FeedbackReceiptStore::new(writer_path.as_path())
                .save_accepted(accepted("new", Some(b"new")), Some(b"new"))
                .unwrap();
        });
        let delete_path = Arc::clone(&path);
        let delete_barrier = Arc::clone(&barrier);
        let deleter = std::thread::spawn(move || {
            delete_barrier.wait();
            FeedbackReceiptStore::new(delete_path.as_path()).delete_local("old")
        });
        barrier.wait();
        writer.join().unwrap();
        let deleted = deleter.join().unwrap();
        assert!(deleted.is_ok() || deleted == Err(FeedbackReceiptError::ReceiptChanged));
        assert_eq!(
            FeedbackReceiptStore::new(path.as_path())
                .export_zip("new")
                .unwrap(),
            b"new"
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_directory_and_leaf_are_rejected_without_touching_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        symlink(outside.path(), dir.path().join("feedback")).unwrap();
        assert_eq!(
            store.save_accepted(accepted("id", None), None),
            Err(FeedbackReceiptError::LocalSaveFailed)
        );
        assert_eq!(
            store.read_last(),
            Err(FeedbackReceiptError::LocalReadFailed)
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        fs::remove_file(dir.path().join("feedback")).unwrap();
        store.save_accepted(accepted("id", None), None).unwrap();
        let path = fs::read_dir(dir.path().join("feedback"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let target = outside.path().join("sensitive");
        fs::write(&target, b"untouched").unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert_eq!(
            store.read_last(),
            Err(FeedbackReceiptError::LocalReadFailed)
        );
        assert_eq!(
            store.delete_local("id"),
            Err(FeedbackReceiptError::LocalReadFailed)
        );
        assert_eq!(
            store.save_accepted(accepted("new", None), None),
            Err(FeedbackReceiptError::LocalSaveFailed)
        );
        assert_eq!(fs::read(target).unwrap(), b"untouched");
    }
    #[test]
    fn failed_staged_publication_preserves_the_exact_old_envelope() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let old = store
            .save_accepted(accepted("old", Some(b"old")), Some(b"old"))
            .unwrap();
        let path = dir.path().join("feedback").join(RECORD_NAME);
        let before = fs::read(&path).unwrap();
        let result = store.save_with(
            accepted("new", Some(b"new")),
            Some(b"new"),
            |path, bytes| {
                let stage = crate::config_file_io::stage_write(path, bytes, Some(0o600), true)?;
                // Inject a genuine failure of the actual publication helper, after
                // bytes were written and fsynced, before it can replace old data.
                fs::remove_file(stage.tmp_path()).unwrap();
                stage.commit()
            },
        );
        assert_eq!(result, Err(FeedbackReceiptError::LocalSaveFailed));
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(store.read_last().unwrap(), Some(old));
        assert_eq!(
            fs::read_dir(dir.path().join("feedback")).unwrap().count(),
            1
        );
    }
    #[test]
    fn failed_write_preserves_success_and_does_not_leak_raw_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let old = store.save_accepted(accepted("old", None), None).unwrap();
        let error = store
            .save_with(accepted("new", None), None, |_, _| {
                Err(crate::error::AppError::Config(
                    "sensitive-path-and-content".into(),
                ))
            })
            .unwrap_err();
        assert_eq!(error, FeedbackReceiptError::LocalSaveFailed);
        assert_eq!(
            serde_json::to_string(&error).unwrap(),
            "\"localSaveFailed\""
        );
        assert_eq!(store.read_last().unwrap(), Some(old));
    }
    #[test]
    fn accepts_exact_ten_mib_with_bounded_encoded_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let zip = vec![7; MAX_ZIP_BYTES];
        store
            .save_accepted(accepted("max", Some(&zip)), Some(&zip))
            .unwrap();
        assert!(
            fs::metadata(dir.path().join("feedback").join(RECORD_NAME))
                .unwrap()
                .len()
                <= MAX_RECORD_BYTES
        );
        assert_eq!(store.export_zip("max").unwrap(), zip);
    }
    #[test]
    fn invalid_timestamps_and_path_like_ids_are_not_saved() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        for id in ["", "../report", "/report", "https://example.invalid/report"] {
            assert_eq!(
                store.save_accepted(accepted(id, None), None),
                Err(FeedbackReceiptError::LocalSaveFailed)
            );
        }
        for timestamp in ["2026-99-99T99:99:99Z", "----", "", "private text"] {
            let mut receipt = accepted("valid", None);
            receipt.uploaded_at = timestamp.into();
            assert_eq!(
                store.save_accepted(receipt, None),
                Err(FeedbackReceiptError::LocalSaveFailed)
            );
        }
        assert!(!dir.path().join("feedback").exists());
    }
    #[test]
    fn malformed_base64_and_declared_size_mismatch_are_rejected_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        store
            .save_accepted(accepted("id", Some(b"zip")), Some(b"zip"))
            .unwrap();
        let path = dir.path().join("feedback").join(RECORD_NAME);
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for encoded in [
            "!!!!".to_string(),
            "YQ==".to_string(),
            "A".repeat(MAX_ENCODED_ZIP_BYTES + 4),
        ] {
            let mut value = original.clone();
            value["diagnosticsZip"] = encoded.into();
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(
                store.read_last(),
                Err(FeedbackReceiptError::LocalReadFailed)
            );
        }
    }
    #[test]
    fn original_diagnostics_producer_zip_is_retained_without_repackaging() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let zip = crate::diagnostics_export::build_diagnostics_zip(
            crate::diagnostics_export::CollectedDiagnostics {
                manifest: serde_json::json!({"appVersion":"synthetic", "testOnly":true}),
                entries: vec![(
                    "logs/synthetic.log".into(),
                    b"synthetic diagnostic log".to_vec(),
                )],
            },
        )
        .unwrap();
        let mut receipt = accepted("producer", Some(&zip));
        receipt.summary.screenshot_count = 1;
        receipt.summary.screenshot_bytes = 10;
        store.save_accepted(receipt, Some(&zip)).unwrap();
        assert_eq!(store.export_zip("producer").unwrap(), zip);
        let archive =
            zip::ZipArchive::new(std::io::Cursor::new(store.export_zip("producer").unwrap()))
                .unwrap();
        let names = archive.file_names().collect::<Vec<_>>();
        assert_eq!(names, vec!["manifest.json", "logs/synthetic.log"]);
        let bytes = fs::read(dir.path().join("feedback").join(RECORD_NAME)).unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope.as_object().unwrap().len(), 3);
        assert_eq!(envelope["receipt"].as_object().unwrap().len(), 3);
        assert_eq!(envelope["receipt"]["summary"].as_object().unwrap().len(), 6);
        assert!(envelope["receipt"].get("description").is_none());
        assert!(envelope["receipt"].get("screenshots").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn envelope_and_directory_use_existing_private_file_owner_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let mode = fs::metadata(dir.path()).unwrap().permissions().mode();
        store.save_accepted(accepted("id", None), None).unwrap();
        assert_eq!(fs::metadata(dir.path()).unwrap().permissions().mode(), mode);
        assert_eq!(
            fs::metadata(dir.path().join("feedback"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(dir.path().join("feedback").join(RECORD_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    #[test]
    fn delete_waits_for_other_instance_publication_then_checks_new_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        FeedbackReceiptStore::new(&root)
            .save_accepted(accepted("old", Some(b"old")), Some(b"old"))
            .unwrap();
        let (staged, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let writer_root = root.clone();
        let writer = std::thread::spawn(move || {
            FeedbackReceiptStore::new(writer_root)
                .save_with(
                    accepted("new", Some(b"new")),
                    Some(b"new"),
                    |path, bytes| {
                        let stage =
                            crate::config_file_io::stage_write(path, bytes, Some(0o600), true)?;
                        staged.send(()).unwrap();
                        wait.recv().unwrap();
                        stage.commit()
                    },
                )
                .unwrap();
        });
        ready.recv().unwrap();
        let (started, starting) = std::sync::mpsc::channel();
        let (done, result) = std::sync::mpsc::channel();
        let delete_root = root.clone();
        let deleter = std::thread::spawn(move || {
            started.send(()).unwrap();
            done.send(FeedbackReceiptStore::new(delete_root).delete_local("old"))
                .unwrap();
        });
        starting.recv().unwrap();
        let premature = result.recv_timeout(std::time::Duration::from_millis(50));
        // Release and join before asserting, so a mutation failure cannot leave
        // a test worker blocked or poison the global owner for later tests.
        release.send(()).unwrap();
        writer.join().unwrap();
        deleter.join().unwrap();
        assert!(matches!(
            premature,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            result.recv().unwrap(),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(
            FeedbackReceiptStore::new(root).export_zip("new").unwrap(),
            b"new"
        );
    }
    #[test]
    fn legacy_issue_receipt_preserves_real_identity_and_original_zip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let input = serde_json::json!({
            "identity": {"kind":"github_issue", "number":1234},
            "uploadedAt":"2026-10-18T10:00:00Z",
            "summary": {"descriptionIncluded":true,"screenshotCount":0,"screenshotBytes":0,"diagnosticsIncluded":true,"diagnosticsBytes":3}
        });
        let receipt: AcceptedFeedbackReceipt =
            serde_json::from_value(input).expect("legacy accepted identity must be supported");
        let dto = store.save_accepted(receipt, Some(b"zip")).unwrap();
        assert_eq!(dto.report_id, "github_issue:1234");
        assert_eq!(
            serde_json::to_value(&dto).unwrap()["identity"],
            serde_json::json!({"kind":"github_issue","number":1234})
        );
        assert_eq!(store.export_zip("github_issue:1234").unwrap(), b"zip");
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(dir.path().join("feedback").join(RECORD_NAME)).unwrap(),
        )
        .unwrap();
        assert!(record["receipt"].get("reportId").is_none());
        assert_eq!(
            record["receipt"]["identity"],
            serde_json::json!({"kind":"github_issue","number":1234})
        );
        store.delete_local("github_issue:1234").unwrap();
        assert_eq!(store.read_last().unwrap(), None);
    }
    #[test]
    fn legacy_number_bounds_and_report_namespace_are_validated_without_losing_old() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let old = store
            .save_accepted(accepted("old", Some(b"old")), Some(b"old"))
            .unwrap();
        for number in [0, 9_007_199_254_740_992, u64::MAX] {
            let mut input = accepted("unused", None);
            input.identity = FeedbackReceiptIdentity::GithubIssue { number };
            assert_eq!(
                store.save_accepted(input, None),
                Err(FeedbackReceiptError::LocalSaveFailed)
            );
            assert_eq!(store.read_last().unwrap(), Some(old.clone()));
        }
        assert_eq!(
            store.save_accepted(accepted("github_issue:42", None), None),
            Err(FeedbackReceiptError::LocalSaveFailed)
        );
        assert_eq!(store.read_last().unwrap(), Some(old));
        let mut input = accepted("unused", None);
        input.identity = FeedbackReceiptIdentity::GithubIssue {
            number: 9_007_199_254_740_991,
        };
        let dto = store.save_accepted(input, None).unwrap();
        assert_eq!(dto.report_id, "github_issue:9007199254740991");
    }

    #[test]
    fn identity_wire_discriminator_and_fields_are_strict() {
        for value in [
            serde_json::json!({"kind":"github_issue","number":-1}),
            serde_json::json!({"kind":"github_issue","number":1.5}),
            serde_json::json!({"kind":"github_issue","number":1,"id":"not-a-report"}),
            serde_json::json!({"kind":"report","id":"report-1","number":1}),
            serde_json::json!({"kind":"unknown","id":"report-1"}),
            serde_json::json!({"id":"report-1"}),
            serde_json::Value::Null,
        ] {
            assert!(serde_json::from_value::<FeedbackReceiptIdentity>(value).is_err());
        }
        let mut receipt = serde_json::to_value(accepted("report-1", None)).unwrap();
        receipt.as_object_mut().unwrap().remove("identity");
        assert!(serde_json::from_value::<AcceptedFeedbackReceipt>(receipt).is_err());
    }

    #[test]
    fn issue_and_report_same_number_have_distinct_cas_keys_and_exact_ui_projection() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        store
            .save_accepted(accepted("42", Some(b"old")), Some(b"old"))
            .unwrap();
        let mut issue = accepted("unused", Some(b"new"));
        issue.identity = FeedbackReceiptIdentity::GithubIssue { number: 42 };
        let dto = store.save_accepted(issue, Some(b"new")).unwrap();
        assert_eq!(
            store.delete_local("42"),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(
            store.export_zip("42"),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(store.export_zip("github_issue:42").unwrap(), b"new");
        let wire = serde_json::to_value(dto).unwrap();
        assert_eq!(
            wire["identity"],
            serde_json::json!({"kind":"github_issue","number":42})
        );
        assert_eq!(
            wire["localCopy"],
            serde_json::json!({"status":"available","fileName":"diagnostics.zip","bytes":3})
        );
        assert_eq!(wire["reportId"], "github_issue:42");
        store
            .save_accepted(accepted("new-server-id", Some(b"latest")), Some(b"latest"))
            .unwrap();
        assert_eq!(
            store.delete_local("github_issue:42"),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(store.export_zip("new-server-id").unwrap(), b"latest");
        assert_eq!(
            serde_json::to_value(store.read_last().unwrap().unwrap()).unwrap()["identity"],
            serde_json::json!({"kind":"report","id":"new-server-id"})
        );
    }
    #[test]
    fn unsaved_report_projects_identity_time_summary_and_missing_zip_exactly() {
        let input = accepted("accepted-report", Some(b"zip"));
        let projected = project_unsaved(input);
        assert_eq!(
            serde_json::to_value(projected).unwrap(),
            serde_json::json!({
                "reportId":"accepted-report",
                "identity":{"kind":"report","id":"accepted-report"},
                "uploadedAt":"2026-10-18T10:00:00.000Z",
                "summary":{"descriptionIncluded":true,"contactIncluded":false,"screenshotCount":0,"screenshotBytes":0,"diagnosticsIncluded":true,"diagnosticsBytes":3},
                "localCopy":{"status":"missing","fileName":"diagnostics.zip","bytes":3}
            })
        );
    }

    #[test]
    fn unsaved_legacy_without_diagnostics_projects_not_attached_without_storage() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = accepted("unused", None);
        input.identity = FeedbackReceiptIdentity::GithubIssue { number: 1234 };
        let projected = project_unsaved(input);
        assert_eq!(
            serde_json::to_value(projected).unwrap(),
            serde_json::json!({
                "reportId":"github_issue:1234",
                "identity":{"kind":"github_issue","number":1234},
                "uploadedAt":"2026-10-18T10:00:00.000Z",
                "summary":{"descriptionIncluded":true,"contactIncluded":false,"screenshotCount":0,"screenshotBytes":0,"diagnosticsIncluded":false,"diagnosticsBytes":0},
                "localCopy":{"status":"notAttached"}
            })
        );
        assert_eq!(
            FeedbackReceiptStore::new(dir.path()).read_last().unwrap(),
            None
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn unsaved_new_receipt_cannot_replace_or_export_over_prior_success() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedbackReceiptStore::new(dir.path());
        let old = store
            .save_accepted(accepted("stored-old", Some(b"old")), Some(b"old"))
            .unwrap();
        let path = dir.path().join("feedback").join(RECORD_NAME);
        let bytes_before = fs::read(&path).unwrap();
        let projected = project_unsaved(accepted("accepted-new", Some(b"new")));
        assert_eq!(
            projected.local_copy,
            LocalFeedbackCopy::Missing {
                file_name: "diagnostics.zip",
                bytes: 3
            }
        );
        assert_eq!(fs::read(path).unwrap(), bytes_before);
        assert_eq!(store.read_last().unwrap(), Some(old));
        assert_eq!(
            store.export_zip("accepted-new"),
            Err(FeedbackReceiptError::ReceiptChanged)
        );
        assert_eq!(store.export_zip("stored-old").unwrap(), b"old");
        assert_eq!(
            fs::read_dir(dir.path().join("feedback")).unwrap().count(),
            1
        );
    }
    #[test]
    fn old_receipt_defaults_contact_absent_and_new_receipt_stores_only_presence() {
        let mut value = serde_json::to_value(accepted("old-contact-schema", None)).unwrap();
        value["summary"]
            .as_object_mut()
            .unwrap()
            .remove("contactIncluded");
        let mut old: AcceptedFeedbackReceipt = serde_json::from_value(value).unwrap();
        assert!(!old.summary.contact_included);
        old.summary.contact_included = true;
        let encoded = serde_json::to_value(old).unwrap();
        assert_eq!(encoded["summary"]["contactIncluded"], true);
        assert!(encoded["summary"].get("contact").is_none());
    }
}
