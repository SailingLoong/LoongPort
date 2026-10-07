use super::*;
use crate::config_file_io::{ensure_private_directory, write_durable};
use crate::secrets::{owned_file::PROFILE_FILE, VaultContext};
use crate::zcode_accounts::{
    bundle::open_bundle,
    checkpoint::ProfileCatalog,
    core::{CredentialDocument, OAuthFamily},
    native::tests::{native_document, TEST_CONTEXT, TEST_SECRET},
};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const PASSWORD: &str = " \t synthetic-export-password-口令 \n";
const NOW: &str = "2026-10-07T00:00:00Z";
const RECEIPTS: &str = "zcode_bundle_exports.json";

struct Fixture {
    _root: tempfile::TempDir,
    vault_root: PathBuf,
    native_root: PathBuf,
    output: PathBuf,
    vault: VaultContext,
    native: NativeCipher,
    profiles: Vec<String>,
    revision: String,
}
impl Fixture {
    fn new() -> Self {
        let tmp = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = tempfile::tempdir_in(tmp).unwrap();
        let vault_root = root.path().join("vault");
        let native_root = root.path().join("native");
        let output = root.path().join("exports");
        for path in [&vault_root, &native_root, &output] {
            ensure_private_directory(path).unwrap();
        }
        let vault = VaultContext::generate().unwrap();
        let native = NativeCipher::new(TEST_CONTEXT, TEST_SECRET).unwrap();
        let mut catalog = ProfileCatalog::default();
        let mut profiles = Vec::new();
        for id in ["synthetic-first", "synthetic-second"] {
            let snapshot = native
                .inspect(&native_document(OAuthFamily::BigModel, id, "saved"))
                .unwrap();
            profiles.push(snapshot.identity().opaque_id());
            catalog.upsert(snapshot);
        }
        write_durable(
            &vault_root.join(PROFILE_FILE),
            catalog.seal(&vault, &native).unwrap().as_bytes(),
        )
        .unwrap();
        write_durable(
            &native_root.join("credentials.json"),
            b"synthetic native bytes stay unchanged",
        )
        .unwrap();
        let revision = VaultAccountStore::new(&vault_root, &vault)
            .unwrap()
            .catalog_status(&native)
            .unwrap()
            .revision;
        Self {
            _root: root,
            vault_root,
            native_root,
            output,
            vault,
            native,
            profiles,
            revision,
        }
    }
    fn store(&self) -> VaultAccountStore<'_> {
        VaultAccountStore::new(&self.vault_root, &self.vault).unwrap()
    }
    fn request(&self) -> ExportRequest {
        ExportRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            catalog_revision: self.revision.clone(),
            profile_ids: vec![self.profiles[1].clone()],
            destination: self.output.join("backup.zsb"),
            password: Zeroizing::new(PASSWORD.into()),
            password_confirmation: Zeroizing::new(PASSWORD.into()),
        }
    }
    fn export(&self, request: ExportRequest) -> ExportResult {
        export_bundle(&self.store(), &self.native, &self.native_root, request, NOW).unwrap()
    }
}

#[test]
fn io_export_authenticates_private_durable_backup_and_persists_queryable_receipt() {
    let f = Fixture::new();
    let profile_before = fs::read(f.vault_root.join(PROFILE_FILE)).unwrap();
    let native_before = fs::read(f.native_root.join("credentials.json")).unwrap();
    let request = f.request();
    let id = request.request_id.clone();
    let destination = request.destination.clone();
    let result = f.export(request);
    assert_eq!(result.status, ExportStatus::Saved);
    assert_eq!(result.count, Some(1));
    assert_eq!(result.destination.as_deref(), destination.to_str());
    assert_eq!(export_result(&f.store(), &id).unwrap(), result);
    let bytes = fs::read(&destination).unwrap();
    let opened = open_bundle(&bytes, PASSWORD).unwrap();
    let rows = opened.entries().unwrap();
    assert_eq!(rows.len(), 1);
    let credentials =
        CredentialDocument::parse(rows[0].as_ref().unwrap().credentials.get().as_bytes()).unwrap();
    assert_eq!(
        f.native
            .inspect(&credentials)
            .unwrap()
            .identity()
            .opaque_id(),
        f.profiles[1]
    );
    assert!(open_bundle(&bytes, PASSWORD.trim()).is_err());
    for path in [&destination, &f.vault_root.join(RECEIPTS)] {
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        let saved = fs::read(path).unwrap();
        assert!(!String::from_utf8_lossy(&saved).contains(PASSWORD));
        assert!(!String::from_utf8_lossy(&saved).contains("synthetic-second"));
    }
    assert_eq!(
        fs::read(f.vault_root.join(PROFILE_FILE)).unwrap(),
        profile_before
    );
    assert_eq!(
        fs::read(f.native_root.join("credentials.json")).unwrap(),
        native_before
    );
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 1);
    let mut replay = f.request();
    replay.request_id = id;
    replay.destination = f.output.join("must-not-replay.zsb");
    assert_eq!(f.export(replay), result);
    assert!(!f.output.join("must-not-replay.zsb").exists());
}

#[test]
fn io_export_business_failures_are_recorded_without_destination_or_count() {
    let f = Fixture::new();
    for case in 0..6 {
        let mut request = f.request();
        let expected = match case {
            0 => {
                request.password = Zeroizing::new(" \n".into());
                ExportFailure::Password
            }
            1 => {
                request.password_confirmation = Zeroizing::new(PASSWORD.trim().into());
                ExportFailure::PasswordMismatch
            }
            2 => {
                request.profile_ids.clear();
                ExportFailure::Selection
            }
            3 => {
                request.profile_ids.push(request.profile_ids[0].clone());
                ExportFailure::Selection
            }
            4 => {
                request.catalog_revision = "stale-synthetic-revision".into();
                ExportFailure::CatalogChanged
            }
            _ => {
                request.profile_ids = vec!["f".repeat(64)];
                ExportFailure::MissingProfile
            }
        };
        let id = request.request_id.clone();
        let result = f.export(request);
        assert_eq!(result.status, ExportStatus::Failed);
        assert_eq!(result.error, Some(expected));
        assert_eq!(result.destination, None);
        assert_eq!(result.count, None);
        assert_eq!(export_result(&f.store(), &id).unwrap(), result);
        let mut replay = f.request();
        replay.request_id = id;
        assert_eq!(f.export(replay), result);
    }
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 0);
}

#[test]
fn io_export_refuses_existing_files_protected_roots_and_symlink_destinations() {
    let f = Fixture::new();
    let existing = f.output.join("existing.zsb");
    fs::write(&existing, b"keep existing ciphertext").unwrap();
    let link = f.output.join("linked.zsb");
    std::os::unix::fs::symlink(&existing, &link).unwrap();
    let directory_link = f.output.join("directory-link");
    std::os::unix::fs::symlink(&f.vault_root, &directory_link).unwrap();
    for target in [
        existing.clone(),
        link,
        f.vault_root.join("must-not-write.zsb"),
        f.native_root.join("must-not-write.zsb"),
        directory_link.join("must-not-write.zsb"),
    ] {
        let mut request = f.request();
        request.destination = target;
        let result = f.export(request);
        assert_eq!(result.status, ExportStatus::Failed);
        assert!(matches!(
            result.error,
            Some(ExportFailure::UnsafeDestination | ExportFailure::DestinationExists)
        ));
    }
    assert_eq!(fs::read(&existing).unwrap(), b"keep existing ciphertext");
    assert!(!f.vault_root.join("must-not-write.zsb").exists());
    assert!(!f.native_root.join("must-not-write.zsb").exists());
}

#[test]
fn io_export_interrupted_phases_stay_unknown_and_never_replay_the_original_request() {
    for point in [
        ExportPoint::Prepared,
        ExportPoint::CiphertextWritten,
        ExportPoint::Authenticated,
    ] {
        let f = Fixture::new();
        let request = f.request();
        let id = request.request_id.clone();
        let destination = request.destination.clone();
        let result = export_with_hook(
            &f.store(),
            &f.native,
            &f.native_root,
            request,
            NOW,
            &mut |at| {
                if at == point {
                    Err(ExportFailure::ResultUnknown)
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(result.status, ExportStatus::Unknown);
        assert!(result.destination.is_none() && result.count.is_none());
        let before = fs::read(&destination).ok();
        let query = export_result(&f.store(), &id).unwrap();
        assert_eq!(query.status, ExportStatus::Unknown);
        let mut replay = f.request();
        replay.request_id = id;
        let replayed = f.export(replay);
        assert_eq!(replayed.status, ExportStatus::Unknown);
        assert_eq!(fs::read(&destination).ok(), before);
    }
}

#[test]
fn io_export_saved_receipt_survives_lost_response_and_detects_later_replacement() {
    let f = Fixture::new();
    let request = f.request();
    let id = request.request_id.clone();
    let destination = request.destination.clone();
    let result = export_with_hook(
        &f.store(),
        &f.native,
        &f.native_root,
        request,
        NOW,
        &mut |at| {
            if at == ExportPoint::ReceiptSaved {
                Err(ExportFailure::ResultUnknown)
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    assert_eq!(result.status, ExportStatus::Unknown);
    assert_eq!(
        export_result(&f.store(), &id).unwrap().status,
        ExportStatus::Saved
    );
    fs::write(&destination, b"externally replaced synthetic file").unwrap();
    let query = export_result(&f.store(), &id).unwrap();
    assert_eq!(query.status, ExportStatus::Unknown);
    assert!(query.destination.is_none() && query.count.is_none());
    let mut replay = f.request();
    replay.request_id = id;
    assert_eq!(f.export(replay).status, ExportStatus::Unknown);
    assert_eq!(
        fs::read(destination).unwrap(),
        b"externally replaced synthetic file"
    );
}

#[test]
fn io_export_racing_destination_creation_never_overwrites_the_new_file() {
    let f = Fixture::new();
    let request = f.request();
    let destination = request.destination.clone();
    let result = export_with_hook(
        &f.store(),
        &f.native,
        &f.native_root,
        request,
        NOW,
        &mut |at| {
            if at == ExportPoint::Prepared {
                fs::write(&destination, b"concurrent owner").unwrap();
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(result.status, ExportStatus::Failed);
    assert_eq!(result.error, Some(ExportFailure::DestinationExists));
    assert_eq!(fs::read(destination).unwrap(), b"concurrent owner");
}

#[test]
fn receipt_requires_durable_prepared_target_before_saved_and_keeps_unknown_distinct() {
    use super::receipt::{ExportTarget, ReceiptPhase};
    let f = Fixture::new();
    let mut ledger = ReceiptLedger::default();
    let mut record = Receipt::prepared(&uuid::Uuid::new_v4().to_string());
    ledger.put(record.clone()).unwrap();
    record.target = Some(ExportTarget {
        context_id: TEST_CONTEXT.into(),
        destination: f.output.join("saved.zsb").to_str().unwrap().into(),
        count: 1,
        ciphertext_sha256: [1; 32],
        ciphertext_bytes: 123,
    });
    record.phase = ReceiptPhase::Saved;
    assert_eq!(
        ledger.put(record.clone()),
        Err(ExportFailure::SavedDataInvalid)
    );
    record.phase = ReceiptPhase::Prepared;
    ledger.put(record.clone()).unwrap();
    record.phase = ReceiptPhase::Failed;
    record.failure = Some(ExportFailure::ResultUnknown);
    assert_eq!(ledger.put(record), Err(ExportFailure::SavedDataInvalid));
}

#[test]
fn io_export_failed_receipt_write_after_authentication_stays_unknown_and_cannot_replay() {
    let f = Fixture::new();
    let request = f.request();
    let id = request.request_id.clone();
    let destination = request.destination.clone();
    let receipt_path = f.vault_root.join(RECEIPTS);
    let mut prepared = None;
    let result = export_with_hook(
        &f.store(),
        &f.native,
        &f.native_root,
        request,
        NOW,
        &mut |at| {
            if at == ExportPoint::Authenticated {
                prepared = Some(fs::read(&receipt_path).unwrap());
                fs::remove_file(&receipt_path).unwrap();
                fs::create_dir(&receipt_path).unwrap();
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(result.status, ExportStatus::Unknown);
    let written = fs::read(&destination).unwrap();
    assert!(open_bundle(&written, PASSWORD).is_ok());
    assert_eq!(
        export_result(&f.store(), &id).unwrap().status,
        ExportStatus::Unknown
    );
    fs::remove_dir(&receipt_path).unwrap();
    write_durable(&receipt_path, &prepared.unwrap()).unwrap();
    let mut replay = f.request();
    replay.request_id = id.clone();
    replay.destination = f.output.join("never-created.zsb");
    assert_eq!(f.export(replay).status, ExportStatus::Unknown);
    assert_eq!(
        export_result(&f.store(), &id).unwrap().status,
        ExportStatus::Unknown
    );
    assert_eq!(fs::read(destination).unwrap(), written);
    assert!(!f.output.join("never-created.zsb").exists());
}

#[test]
fn io_export_tamper_or_permission_change_after_write_cannot_publish_saved() {
    for case in 0..3 {
        let f = Fixture::new();
        let request = f.request();
        let id = request.request_id.clone();
        let destination = request.destination.clone();
        let result = export_with_hook(
            &f.store(),
            &f.native,
            &f.native_root,
            request,
            NOW,
            &mut |at| {
                if at == ExportPoint::CiphertextWritten {
                    match case {
                        0 => fs::write(&destination, b"changed by another writer").unwrap(),
                        1 => fs::set_permissions(&destination, fs::Permissions::from_mode(0o644))
                            .unwrap(),
                        _ => fs::hard_link(&destination, f.output.join("second-link.zsb")).unwrap(),
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result.status, ExportStatus::Unknown);
        assert!(result.destination.is_none() && result.count.is_none());
        assert_eq!(
            export_result(&f.store(), &id).unwrap().status,
            ExportStatus::Unknown
        );
    }
}

#[test]
fn io_export_rechecks_catalog_and_parent_identity_immediately_before_create() {
    for catalog_changes in [false, true] {
        let f = Fixture::new();
        let request = f.request();
        let result = export_with_hook(
            &f.store(),
            &f.native,
            &f.native_root,
            request,
            NOW,
            &mut |at| {
                if at == ExportPoint::Prepared {
                    if catalog_changes {
                        write_durable(
                            &f.vault_root.join(PROFILE_FILE),
                            b"external catalog changed",
                        )
                        .unwrap();
                    } else {
                        fs::rename(&f.output, f.output.with_extension("old")).unwrap();
                        ensure_private_directory(&f.output).unwrap();
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result.status, ExportStatus::Failed);
        assert_eq!(
            result.error,
            Some(if catalog_changes {
                ExportFailure::CatalogChanged
            } else {
                ExportFailure::UnsafeDestination
            })
        );
        assert_eq!(fs::read_dir(&f.output).unwrap().count(), 0);
        if !catalog_changes {
            assert_eq!(
                fs::read_dir(f.output.with_extension("old"))
                    .unwrap()
                    .count(),
                0
            );
        }
    }
}

#[test]
fn receipt_authentication_bounds_identity_and_capacity_never_silently_retire_requests() {
    use crate::secrets::owned_file::{OwnedFile, BUNDLE_EXPORT_FILE, OPERATION_FILE};
    let f = Fixture::new();
    let mut ledger = ReceiptLedger::default();
    let first = uuid::Uuid::new_v4().to_string();
    ledger.put(Receipt::prepared(&first)).unwrap();
    for _ in 1..1024 {
        ledger
            .put(Receipt::prepared(&uuid::Uuid::new_v4().to_string()))
            .unwrap();
    }
    assert_eq!(
        ledger.put(Receipt::prepared(&uuid::Uuid::new_v4().to_string())),
        Err(ExportFailure::ResourceLimit)
    );
    let sealed = ledger.seal(&f.vault).unwrap();
    let reopened = ReceiptLedger::open(&sealed, &f.vault).unwrap();
    assert!(reopened.get(&first).is_some());
    assert!(ReceiptLedger::open(&sealed, &VaultContext::generate().unwrap()).is_err());
    assert!(OwnedFile::registered(OPERATION_FILE)
        .unwrap()
        .decode(&f.vault, &sealed)
        .is_err());
    assert!(!OwnedFile::registered(BUNDLE_EXPORT_FILE)
        .unwrap()
        .allows_legacy_plaintext());
    let unknown_field = r#"{"version":1,"records":[],"password":"synthetic-never-accepted"}"#;
    let invalid = OwnedFile::registered(BUNDLE_EXPORT_FILE)
        .unwrap()
        .encode(&f.vault, unknown_field.as_bytes())
        .unwrap();
    assert!(ReceiptLedger::open(&invalid, &f.vault).is_err());
    write_durable(&f.vault_root.join(RECEIPTS), b"plaintext is never upgraded").unwrap();
    let original = fs::read(f.vault_root.join(RECEIPTS)).unwrap();
    let result = f.export(f.request());
    assert_eq!(result.status, ExportStatus::Unknown);
    assert_eq!(fs::read(f.vault_root.join(RECEIPTS)).unwrap(), original);
    assert_eq!(fs::read_dir(&f.output).unwrap().count(), 0);
}

#[test]
fn io_export_response_has_static_error_codes_and_only_saved_exposes_target_metadata() {
    let f = Fixture::new();
    let mut invalid = f.request();
    invalid.password_confirmation = Zeroizing::new("synthetic-mismatch-canary".into());
    let response = serde_json::to_value(f.export(invalid)).unwrap();
    assert_eq!(response["status"], "failed");
    assert_eq!(
        response["error"]["code"],
        "zcode.account.bundle_export_password_mismatch"
    );
    assert_eq!(response["error"]["remedy"], "queryOriginal");
    assert_eq!(response["error"]["committed"], false);
    assert!(response["destination"].is_null() && response["count"].is_null());
    assert!(!response.to_string().contains("synthetic-mismatch-canary"));
    let mut invalid_id = f.request();
    invalid_id.request_id = "../synthetic-id".into();
    assert_eq!(
        export_bundle(&f.store(), &f.native, &f.native_root, invalid_id, NOW),
        Err(ExportFailure::InvalidRequest)
    );
    assert_eq!(
        export_result(&f.store(), "../synthetic-id"),
        Err(ExportFailure::InvalidRequest)
    );
}
