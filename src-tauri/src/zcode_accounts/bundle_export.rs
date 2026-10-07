//! Original-request ownership for local encrypted .zsb backups.
//! Callers hold run_owned's physical owner and the SecretSession read guard.
use super::{native::NativeCipher, transaction::VaultAccountStore};
use serde::Serialize;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[path = "bundle_export_receipt.rs"]
mod receipt;
use receipt::ReceiptPhase;
pub(crate) use receipt::{Receipt, ReceiptLedger};
#[cfg(unix)]
#[path = "bundle_export_file.rs"]
mod destination;

pub(crate) struct ExportRequest {
    pub request_id: String,
    pub catalog_revision: String,
    pub profile_ids: Vec<String>,
    pub destination: PathBuf,
    pub password: Zeroizing<String>,
    pub password_confirmation: Zeroizing<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExportFailure {
    InvalidRequest,
    Password,
    PasswordMismatch,
    Selection,
    CatalogChanged,
    MissingProfile,
    UnsafeDestination,
    DestinationExists,
    ResourceLimit,
    Storage,
    SavedDataInvalid,
    ResultUnknown,
    UnsupportedPlatform,
}
impl ExportFailure {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "zcode.account.bundle_export_invalid_request",
            Self::Password => "zcode.account.bundle_export_password",
            Self::PasswordMismatch => "zcode.account.bundle_export_password_mismatch",
            Self::Selection => "zcode.account.bundle_export_selection",
            Self::CatalogChanged => "zcode.account.catalog_changed",
            Self::MissingProfile => "zcode.account.missing_target",
            Self::UnsafeDestination => "zcode.account.unsafe_path",
            Self::DestinationExists => "zcode.account.bundle_export_destination_exists",
            Self::ResourceLimit => "zcode.account.resource_limit",
            Self::Storage => "zcode.account.storage_failed",
            Self::SavedDataInvalid => "zcode.account.saved_data_invalid",
            Self::ResultUnknown => "zcode.account.bundle_export_unknown",
            Self::UnsupportedPlatform => "zcode.account.unsupported_platform",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExportStatus {
    #[cfg(unix)]
    Saved,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportResult {
    pub request_id: String,
    pub status: ExportStatus,
    pub destination: Option<String>,
    pub count: Option<usize>,
    #[serde(serialize_with = "serialize_failure")]
    pub error: Option<ExportFailure>,
}

fn serialize_failure<S: serde::Serializer>(
    failure: &Option<ExportFailure>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Problem {
        code: &'static str,
        remedy: &'static str,
        committed: bool,
    }
    match failure {
        None => serializer.serialize_none(),
        Some(failure) => Problem {
            code: failure.code(),
            remedy: "queryOriginal",
            committed: false,
        }
        .serialize(serializer),
    }
}

fn unknown(request_id: &str, failure: ExportFailure) -> ExportResult {
    ExportResult {
        request_id: request_id.into(),
        status: ExportStatus::Unknown,
        destination: None,
        count: None,
        error: Some(failure),
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportPoint {
    Prepared,
    CiphertextWritten,
    Authenticated,
    ReceiptSaved,
}

#[cfg(unix)]
pub(crate) fn export_bundle(
    store: &VaultAccountStore<'_>,
    native: &NativeCipher,
    native_root: &Path,
    request: ExportRequest,
    exported_at: &str,
) -> Result<ExportResult, ExportFailure> {
    export_with_hook(
        store,
        native,
        native_root,
        request,
        exported_at,
        &mut |_| Ok(()),
    )
}

#[cfg(not(unix))]
pub(crate) fn export_bundle(
    _store: &VaultAccountStore<'_>,
    _native: &NativeCipher,
    _native_root: &Path,
    ExportRequest {
        request_id: _request_id,
        catalog_revision: _catalog_revision,
        profile_ids: _profile_ids,
        destination: _destination,
        password: _password,
        password_confirmation: _password_confirmation,
    }: ExportRequest,
    _exported_at: &str,
) -> Result<ExportResult, ExportFailure> {
    // Consume the owned payload without reading the vault or destination.
    // Passwords keep their normal Zeroizing drop behavior on this return.
    Err(ExportFailure::UnsupportedPlatform)
}

#[cfg(unix)]
fn export_with_hook(
    store: &VaultAccountStore<'_>,
    native: &NativeCipher,
    native_root: &Path,
    request: ExportRequest,
    exported_at: &str,
    hook: &mut dyn FnMut(ExportPoint) -> Result<(), ExportFailure>,
) -> Result<ExportResult, ExportFailure> {
    use std::io::Write;
    if !valid_request_id(&request.request_id) {
        return Err(ExportFailure::InvalidRequest);
    }
    match store.bundle_export_receipt(&request.request_id) {
        Ok(Some(existing)) => return Ok(receipt_result(&existing)),
        Ok(None) => {}
        Err(error) => return Ok(unknown(&request.request_id, error)),
    }
    // Reserve the identity durably before validating/encoding or touching the
    // destination. Repeated requests only observe this record, even after a crash.
    let mut receipt = Receipt::prepared(&request.request_id);
    if let Err(error) = store.record_bundle_export(receipt.clone()) {
        return Ok(unknown(&request.request_id, error));
    }
    let (ciphertext, target, destination) =
        match prepare(store, native, native_root, &request, exported_at) {
            Ok(prepared) => prepared,
            Err(error) => return Ok(failed(store, receipt, error)),
        };
    receipt.target = Some(target.clone());
    if let Err(error) = store.record_bundle_export(receipt.clone()) {
        return Ok(unknown(&request.request_id, error));
    }
    if let Err(error) = hook(ExportPoint::Prepared) {
        return Ok(unknown(&request.request_id, error));
    }
    // Encoding/KDF work does not grant permission to export a changed catalog.
    if let Err(error) = store.import_catalog(native, &request.catalog_revision) {
        return Ok(failed(store, receipt, transaction_failure(error)));
    }
    if let Err(error) = store.bundle_export_root() {
        return Ok(failed(store, receipt, transaction_failure(error)));
    }
    let mut file = match destination.create_new() {
        Ok(file) => file,
        Err(error) => return Ok(failed(store, receipt, error)),
    };
    // From this point onward a file may exist. Do not erase it, replay the
    // request, or turn a failed write/read-back into proof of a failed save.
    let verified = (|| {
        file.write_all(&ciphertext)
            .map_err(|_| ExportFailure::Storage)?;
        hook(ExportPoint::CiphertextWritten)?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|_| ExportFailure::Storage)?;
        destination.sync_directory()?;
        let readback = destination.verified_bytes(
            target.ciphertext_bytes,
            &target.ciphertext_sha256,
            Some(&file),
        )?;
        let opened = super::bundle::open_bundle(&readback, &request.password)
            .map_err(|_| ExportFailure::SavedDataInvalid)?;
        let entries = opened
            .entries()
            .map_err(|_| ExportFailure::SavedDataInvalid)?;
        if entries.len() != target.count || entries.iter().any(Result::is_err) {
            return Err(ExportFailure::SavedDataInvalid);
        }
        hook(ExportPoint::Authenticated)?;
        Ok(())
    })();
    if let Err(error) = verified {
        return Ok(unknown(&request.request_id, error));
    }
    receipt.phase = ReceiptPhase::Saved;
    if let Err(error) = store.record_bundle_export(receipt.clone()) {
        return Ok(unknown(&request.request_id, error));
    }
    if let Err(error) = hook(ExportPoint::ReceiptSaved) {
        return Ok(unknown(&request.request_id, error));
    }
    Ok(receipt_result(&receipt))
}

#[cfg(unix)]
fn prepare(
    store: &VaultAccountStore<'_>,
    native: &NativeCipher,
    native_root: &Path,
    request: &ExportRequest,
    exported_at: &str,
) -> Result<(Vec<u8>, receipt::ExportTarget, destination::Destination), ExportFailure> {
    use super::bundle::{encode_bundle, BundleExportAccount, BundleFailure};
    use sha2::{Digest, Sha256};
    if request.password.trim().is_empty() || request.password.len() > 4096 {
        return Err(ExportFailure::Password);
    }
    if request.password.as_bytes() != request.password_confirmation.as_bytes() {
        return Err(ExportFailure::PasswordMismatch);
    }
    if !(1..=50).contains(&request.profile_ids.len())
        || request
            .profile_ids
            .iter()
            .any(|id| id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(ExportFailure::Selection);
    }
    let ids: std::collections::BTreeSet<_> = request.profile_ids.iter().collect();
    if ids.len() != request.profile_ids.len() {
        return Err(ExportFailure::Selection);
    }
    if request.catalog_revision.is_empty() || request.catalog_revision.len() > 128 {
        return Err(ExportFailure::CatalogChanged);
    }
    if exported_at.is_empty() || exported_at.len() > 128 {
        return Err(ExportFailure::InvalidRequest);
    }
    let root = store.bundle_export_root().map_err(transaction_failure)?;
    let destination = destination::Destination::open(&request.destination, &[root, native_root])?;
    destination.require_absent()?;
    let catalog = store
        .import_catalog(native, &request.catalog_revision)
        .map_err(transaction_failure)?;
    let accounts = request
        .profile_ids
        .iter()
        .map(|id| {
            catalog
                .profiles()
                .find(|snapshot| snapshot.identity().opaque_id() == *id)
                .map(|snapshot| BundleExportAccount {
                    snapshot,
                    created_at: exported_at,
                })
                .ok_or(ExportFailure::MissingProfile)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ciphertext =
        encode_bundle(&accounts, native, &request.password, exported_at).map_err(|error| {
            match error {
                BundleFailure::Password => ExportFailure::Password,
                BundleFailure::Limits(_) => ExportFailure::ResourceLimit,
                _ => ExportFailure::SavedDataInvalid,
            }
        })?;
    let target = receipt::ExportTarget {
        context_id: native.context().into(),
        destination: request
            .destination
            .to_str()
            .ok_or(ExportFailure::UnsafeDestination)?
            .into(),
        count: accounts.len(),
        ciphertext_sha256: Sha256::digest(&ciphertext).into(),
        ciphertext_bytes: ciphertext.len(),
    };
    Ok((ciphertext, target, destination))
}

#[cfg(unix)]
fn transaction_failure(error: super::transaction::TransactionError) -> ExportFailure {
    use super::transaction::TransactionError;
    match error {
        TransactionError::CatalogChanged => ExportFailure::CatalogChanged,
        TransactionError::UnsafePath => ExportFailure::UnsafeDestination,
        TransactionError::Checkpoint(_) => ExportFailure::SavedDataInvalid,
        _ => ExportFailure::Storage,
    }
}

#[cfg(unix)]
fn failed(
    store: &VaultAccountStore<'_>,
    mut receipt: Receipt,
    error: ExportFailure,
) -> ExportResult {
    receipt.phase = ReceiptPhase::Failed;
    receipt.failure = Some(error);
    if let Err(error) = store.record_bundle_export(receipt.clone()) {
        return unknown(&receipt.request_id, error);
    }
    receipt_result(&receipt)
}

pub(crate) fn export_result(
    store: &VaultAccountStore<'_>,
    request_id: &str,
) -> Result<ExportResult, ExportFailure> {
    if !valid_request_id(request_id) {
        return Err(ExportFailure::InvalidRequest);
    }
    Ok(match store.bundle_export_receipt(request_id) {
        Ok(Some(receipt)) => receipt_result(&receipt),
        Ok(None) => unknown(request_id, ExportFailure::ResultUnknown),
        Err(error) => unknown(request_id, error),
    })
}

fn receipt_result(receipt: &Receipt) -> ExportResult {
    match receipt.phase {
        ReceiptPhase::Prepared => unknown(&receipt.request_id, ExportFailure::ResultUnknown),
        ReceiptPhase::Failed => ExportResult {
            request_id: receipt.request_id.clone(),
            status: ExportStatus::Failed,
            destination: None,
            count: None,
            error: receipt.failure,
        },
        ReceiptPhase::Saved => {
            #[cfg(unix)]
            if let Some(target) = &receipt.target {
                // The receipt certifies a prior authenticated read-back. A fresh
                // digest check proves the same ciphertext still occupies its path;
                // no package password needs to be retained for result queries.
                let checked = destination::Destination::open(Path::new(&target.destination), &[])
                    .and_then(|destination| {
                        destination.verified_bytes(
                            target.ciphertext_bytes,
                            &target.ciphertext_sha256,
                            None,
                        )
                    });
                if checked.is_ok() {
                    return ExportResult {
                        request_id: receipt.request_id.clone(),
                        status: ExportStatus::Saved,
                        destination: Some(target.destination.clone()),
                        count: Some(target.count),
                        error: None,
                    };
                }
            }
            unknown(&receipt.request_id, ExportFailure::ResultUnknown)
        }
    }
}

fn valid_request_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id)
        .map(|value| value.to_string())
        .ok()
        .as_deref()
        == Some(id)
}

fn valid_destination_shape(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|text| text.len() <= 4096 && !text.chars().any(char::is_control))
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zsb"))
        && !path.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
}

#[cfg(all(test, unix))]
#[path = "bundle_export_tests.rs"]
mod tests;
