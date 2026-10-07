//! Thin command-facing account API. Only explicit actions enter native credential IO.
#[cfg(test)]
use super::admission::VerifiedContext;
use super::{
    admission::{BlockedReason, ContextObservation, ContextProbe, ContractEntry, Remedy},
    checkpoint::CheckpointError,
    core::{CoreError, OAuthFamily},
    native_context::{
        discover_metadata, supported_contracts, ContextSelection, MetadataDiscovery,
        NativeContextProbe,
    },
    recovery::RecoveryError,
    runtime::{self, CapturePreview, RuntimeError},
    transaction::{
        ArchiveOutcome, CaptureCommitOutcome, CaptureOutcome, CatalogStatus, RecoveryStatus,
        SwitchOutcome, TransactionError,
    },
};
use crate::database::Database;
use std::sync::{Arc, OnceLock};
use zeroize::Zeroize;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceContext {
    context_id: String,
    context_revision: String,
    data_root: String,
    family: &'static str,
    version: String,
    build: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct PublicError {
    code: &'static str,
    remedy: &'static str,
    committed: bool,
}
impl PublicError {
    pub(super) fn new(code: &'static str, remedy: &'static str) -> Self {
        Self {
            code,
            remedy,
            committed: false,
        }
    }
}
impl From<BlockedReason> for PublicError {
    fn from(reason: BlockedReason) -> Self {
        let remedy = match reason.remedy() {
            Remedy::ChooseContext => "chooseContext",
            Remedy::UseVerifiedBuild => "useVerifiedBuild",
            Remedy::FinishPlatformCheck => "finishPlatformCheck",
            Remedy::ReviewDataLocation => "reviewDataLocation",
            Remedy::UseStandardContext => "useStandardContext",
            Remedy::OpenNativeSettings => "openNativeSettings",
            Remedy::QuitNativeWriters => "quitNativeWriters",
            Remedy::VerifyWriterState => "verifyWriterState",
            Remedy::RefreshContext => "refreshContext",
            #[cfg(test)]
            Remedy::ChooseSavedAccount => "chooseSavedAccount",
        };
        Self::new(reason.code(), remedy)
    }
}
impl From<CheckpointError> for PublicError {
    fn from(error: CheckpointError) -> Self {
        match error {
            CheckpointError::WrongContext => {
                Self::new("zcode.account.context_changed", "refreshContext")
            }
            CheckpointError::ResourceLimit => {
                Self::new("zcode.account.resource_limit", "reviewRecovery")
            }
            CheckpointError::Native(_) => {
                Self::new("zcode.account.native_session_invalid", "openNativeSettings")
            }
            CheckpointError::Core(CoreError::DifferentFamily) => {
                Self::new("zcode.account.unsupported_scope", "openNativeSettings")
            }
            CheckpointError::Core(CoreError::DifferentContext | CoreError::SourceChanged) => {
                Self::new("zcode.account.source_changed", "refreshContext")
            }
            _ => Self::new("zcode.account.saved_data_invalid", "reviewSavedData"),
        }
    }
}
impl From<RecoveryError> for PublicError {
    fn from(error: RecoveryError) -> Self {
        match error {
            RecoveryError::Checkpoint(error) => error.into(),
            RecoveryError::ArchiveFull => {
                Self::new("zcode.account.recovery_full", "reviewRecovery")
            }
            RecoveryError::Unconfirmed => {
                Self::new("zcode.account.recovery_unconfirmed", "confirmOrRecapture")
            }
            RecoveryError::ConfirmationMismatch => {
                Self::new("zcode.account.confirmation_mismatch", "confirmOrRecapture")
            }
            RecoveryError::NotFound | RecoveryError::DuplicateRecord => {
                Self::new("zcode.account.recovery_changed", "refreshContext")
            }
        }
    }
}
impl From<TransactionError> for PublicError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::Admission(reason) => reason.into(),
            TransactionError::Checkpoint(error) => error.into(),
            TransactionError::Recovery(error) => error.into(),
            TransactionError::CommittedNeedsCleanup => Self {
                code: "zcode.account.committed_recovery_required",
                remedy: "reviewRecovery",
                committed: true,
            },
            TransactionError::NotAdmitted => {
                if cfg!(unix) {
                    Self::new("zcode.account.not_admitted", "refreshContext")
                } else {
                    // Account storage has no non-Unix adapter. Inspecting the
                    // native source cannot make local recovery available there.
                    BlockedReason::UnsupportedPlatform.into()
                }
            }
            TransactionError::UnsupportedScope => {
                Self::new("zcode.account.unsupported_scope", "openNativeSettings")
            }
            TransactionError::UnverifiedSource => {
                Self::new("zcode.account.source_unverified", "captureCurrent")
            }
            TransactionError::UnsafePath => {
                Self::new("zcode.account.unsafe_path", "reviewDataLocation")
            }
            TransactionError::Storage => {
                Self::new("zcode.account.storage_failed", "checkLocalStorage")
            }
            TransactionError::SourceChanged => {
                Self::new("zcode.account.source_changed", "refreshContext")
            }
            TransactionError::RecoveryRequired => Self::new(
                "zcode.account.pending_or_locked",
                "quitWritersAndReviewRecovery",
            ),
            TransactionError::Imported => Self::new(
                "zcode.account.recovery_context_mismatch",
                "reviewDataLocation",
            ),
            TransactionError::MissingSavedSource => {
                Self::new("zcode.account.missing_saved_source", "captureCurrent")
            }
            TransactionError::MissingTarget => {
                Self::new("zcode.account.missing_target", "refreshContext")
            }
            TransactionError::CatalogChanged => {
                Self::new("zcode.account.catalog_changed", "refreshContext")
            }
            TransactionError::RecoveryChanged => {
                Self::new("zcode.account.recovery_changed", "refreshContext")
            }
            TransactionError::NativeUnconfirmed => {
                Self::new("zcode.account.native_unconfirmed", "confirmOrRecapture")
            }
            TransactionError::OperationAlreadyKnown => {
                Self::new("zcode.account.operation_already_known", "queryOriginal")
            }
            TransactionError::ArchiveNeedsCleanup => {
                Self::new("zcode.account.recovery_cleanup_required", "reviewRecovery")
            }
        }
    }
}
impl From<RuntimeError> for PublicError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::Blocked(reason) => reason.into(),
            RuntimeError::VaultUnavailable => {
                Self::new("zcode.account.vault_unavailable", "unlockVault")
            }
            RuntimeError::Transaction(error) => error.into(),
            RuntimeError::TaskFailed => {
                Self::new("zcode.account.operation_failed", "refreshContext")
            }
            RuntimeError::BundleAuthentication => {
                Self::new("zcode.account.bundle_authentication", "reviewSavedData")
            }
            RuntimeError::BundleInvalid => {
                Self::new("zcode.account.bundle_invalid", "reviewSavedData")
            }
            RuntimeError::CommittedRestartFailed => Self {
                code: "zcode.account.committed_restart_failed",
                remedy: "openNativeSettings",
                committed: true,
            },
            RuntimeError::CommittedResultUnknown => Self {
                code: "zcode.account.committed_result_unknown",
                remedy: "queryOriginal",
                committed: true,
            },
        }
    }
}

/// The review-page revision is a concurrency check, never an authorization token.
/// The real probe supplies every fact again on every runtime admission callback.
// Construction is deliberately cheap: artifact hashing and OS observations must
// begin inside runtime's physical worker, never on the async IPC executor.
struct DeferredNativeProbe {
    source: ContextSelection,
    opened: OnceLock<Result<NativeContextProbe, BlockedReason>>,
}
impl DeferredNativeProbe {
    fn probe(&self) -> Result<&NativeContextProbe, BlockedReason> {
        self.opened
            .get_or_init(|| NativeContextProbe::new(self.source.clone()))
            .as_ref()
            .map_err(|error| *error)
    }
}
impl ContextProbe for DeferredNativeProbe {
    fn open_for_login(&self) -> Result<(), BlockedReason> {
        self.probe()?.open_for_login()
    }
    fn prepare_switch(&self) -> Result<bool, BlockedReason> {
        self.probe()?.prepare_switch()
    }
    fn restart_after_switch(&self) -> Result<(), BlockedReason> {
        self.probe()?.restart_after_switch()
    }
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        match self
            .opened
            .get_or_init(|| NativeContextProbe::new(self.source.clone()))
        {
            Ok(probe) => probe.observe(),
            Err(error) => Err(*error),
        }
    }
}
struct ExpectedContextProbe {
    inner: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    expected: String,
}
impl ContextProbe for ExpectedContextProbe {
    fn prepare_switch(&self) -> Result<bool, BlockedReason> {
        self.observe()?;
        self.inner.prepare_switch()
    }
    fn restart_after_switch(&self) -> Result<(), BlockedReason> {
        self.observe()?;
        self.inner.restart_after_switch()
    }
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        let mut observed = self.inner.observe()?;
        let checked = super::admission::ReadOnlyContext::assess(observed.clone(), &self.contracts)
            .and_then(|context| {
                if context.context_revision() == self.expected {
                    Ok(())
                } else {
                    Err(BlockedReason::ContextChanged)
                }
            });
        if let Err(error) = checked {
            observed.settings.zeroize();
            return Err(error);
        }
        Ok(observed)
    }
}
struct NativeInputs {
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
}
fn require_selection(source: &ContextSelection) -> Result<(), BlockedReason> {
    if source.install_path.as_os_str().is_empty() || source.data_root.as_os_str().is_empty() {
        Err(BlockedReason::SelectContext)
    } else {
        Ok(())
    }
}
fn inputs(source: ContextSelection, revision: String) -> Result<NativeInputs, PublicError> {
    require_selection(&source)?;
    if revision.is_empty() || revision.len() > 128 {
        return Err(BlockedReason::ContextChanged.into());
    }
    let contracts = supported_contracts();
    let probe = Arc::new(ExpectedContextProbe {
        inner: Arc::new(DeferredNativeProbe {
            source,
            opened: OnceLock::new(),
        }),
        contracts: contracts.clone(),
        expected: revision,
    });
    Ok(NativeInputs { probe, contracts })
}
fn summary(
    observed: ContextObservation,
    contracts: &[ContractEntry],
) -> Result<SourceContext, BlockedReason> {
    let version = observed.install.version.clone();
    let build = observed.install.build.clone();
    let context = super::admission::ReadOnlyContext::assess(observed, contracts)?;
    Ok(SourceContext {
        context_id: context.context_id().to_owned(),
        context_revision: context.context_revision(),
        data_root: context
            .native_root()
            .to_str()
            .ok_or(BlockedReason::RootUnverified)?
            .to_owned(),
        family: match context.family() {
            OAuthFamily::Zai => "zai",
            OAuthFamily::BigModel => "bigmodel",
        },
        version,
        build,
    })
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LatestVersion {
    version: Option<String>,
    checked_at: u64,
    error: Option<&'static str>,
}
pub(crate) async fn latest_version() -> LatestVersion {
    let checked_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    #[cfg(not(target_os = "macos"))]
    {
        LatestVersion {
            version: None,
            checked_at,
            error: Some("unsupportedPlatform"),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let result = async {
            let url = super::latest_version::manifest_url(std::env::consts::ARCH)
                .ok_or("unsupportedPlatform")?;
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .map_err(|_| "network")?;
            let platform = if std::env::consts::ARCH == "aarch64" {
                "darwin-aarch64"
            } else {
                "darwin-x86_64"
            };
            let mut response = client
                .get(url)
                .header("Accept", "application/x-yaml,text/yaml,text/plain")
                .header("X-Platform", platform)
                .header("X-Release-Channel", "1")
                .send()
                .await
                .map_err(|_| "network")?
                .error_for_status()
                .map_err(|_| "network")?;
            let limit = super::latest_version::MAX_MANIFEST_BYTES;
            if response
                .content_length()
                .is_some_and(|size| size > limit as u64)
            {
                return Err("invalidManifest");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| "network")? {
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err("invalidManifest");
                }
                bytes.extend_from_slice(&chunk);
            }
            super::latest_version::version(&bytes).ok_or("invalidManifest")
        }
        .await;
        match result {
            Ok(version) => LatestVersion {
                version: Some(version),
                checked_at,
                error: None,
            },
            Err(error) => LatestVersion {
                version: None,
                checked_at,
                error: Some(error),
            },
        }
    }
}
pub(crate) async fn discover(
    db: Arc<Database>,
    selected: Option<std::path::PathBuf>,
) -> Result<MetadataDiscovery, PublicError> {
    runtime::run_owned(db, move |_| {
        discover_metadata(selected.as_deref()).map_err(Into::into)
    })
    .await
    .map_err(Into::into)
}
pub(crate) async fn inspect(
    db: Arc<Database>,
    source: ContextSelection,
) -> Result<SourceContext, PublicError> {
    runtime::run_owned(db, move |_| {
        require_selection(&source)?;
        let probe = NativeContextProbe::new(source)?;
        summary(probe.observe()?, &supported_contracts()).map_err(Into::into)
    })
    .await
    .map_err(Into::into)
}
pub(crate) async fn open_for_login(
    db: Arc<Database>,
    source: ContextSelection,
) -> Result<(), PublicError> {
    runtime::run_owned(db, move |_| {
        require_selection(&source)?;
        NativeContextProbe::new(source)?
            .open_for_login()
            .map_err(Into::into)
    })
    .await
    .map_err(Into::into)
}
pub(crate) async fn status(
    db: Arc<Database>,
    source: ContextSelection,
    revision: String,
) -> Result<CatalogStatus, PublicError> {
    let input = inputs(source, revision)?;
    runtime::account_status(db, input.probe, input.contracts)
        .await
        .map_err(Into::into)
}
pub(crate) async fn preview_capture(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
) -> Result<CapturePreview, PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::preview_capture_account(db, input.probe, input.contracts, catalog_revision)
        .await
        .map_err(Into::into)
}
pub(crate) async fn preview_bundle(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    input: runtime::BundlePreviewInput,
) -> Result<runtime::BundlePreview, PublicError> {
    let input_source = inputs(source, context_revision)?;
    runtime::preview_bundle(db, input_source.probe, input_source.contracts, input)
        .await
        .map_err(Into::into)
}
pub(crate) async fn commit_bundle(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    input: runtime::BundleCommitInput,
) -> Result<Vec<CaptureCommitOutcome>, PublicError> {
    let input_source = match inputs(source, context_revision) {
        Ok(value) => value,
        Err(error) => {
            runtime::cancel_bundle_preview(db, input.preview_id)
                .await
                .map_err(PublicError::from)?;
            return Err(error);
        }
    };
    runtime::commit_bundle(db, input_source.probe, input_source.contracts, input)
        .await
        .map_err(Into::into)
}
pub(crate) async fn cancel_bundle_preview(
    db: Arc<Database>,
    preview_id: String,
) -> Result<(), PublicError> {
    runtime::cancel_bundle_preview(db, preview_id)
        .await
        .map_err(Into::into)
}
pub(crate) async fn capture_reviewed(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
    preview_id: String,
    update_duplicate: bool,
) -> Result<CaptureCommitOutcome, PublicError> {
    let input = match inputs(source, context_revision) {
        Ok(input) => input,
        Err(error) => {
            runtime::cancel_capture_preview(db, preview_id)
                .await
                .map_err(PublicError::from)?;
            return Err(error);
        }
    };
    runtime::capture_reviewed_account(
        db,
        input.probe,
        input.contracts,
        catalog_revision,
        preview_id,
        update_duplicate,
    )
    .await
    .map_err(Into::into)
}
pub(crate) async fn cancel_capture_preview(
    db: Arc<Database>,
    preview_id: String,
) -> Result<(), PublicError> {
    runtime::cancel_capture_preview(db, preview_id)
        .await
        .map_err(Into::into)
}
pub(crate) async fn switch_saved(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    catalog_revision: String,
    request_id: String,
) -> Result<&'static str, PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::switch_account_request(
        db,
        input.probe,
        input.contracts,
        id,
        catalog_revision,
        request_id,
    )
    .await
    .map(|result| match result {
        SwitchOutcome::Switched => "switched",
        SwitchOutcome::Refreshed => "refreshed",
    })
    .map_err(Into::into)
}
pub(crate) async fn operation_status(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    request_id: String,
) -> Result<Option<runtime::OperationStatus>, PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::operation_status(db, input.probe, input.contracts, request_id)
        .await
        .map_err(Into::into)
}
pub(crate) async fn recovery_status(db: Arc<Database>) -> Result<RecoveryStatus, PublicError> {
    runtime::recovery_status(db).await.map_err(Into::into)
}
pub(crate) async fn archive(
    db: Arc<Database>,
    revision: String,
) -> Result<&'static str, PublicError> {
    runtime::archive_pending_recovery(db, revision)
        .await
        .map(|result| match result {
            ArchiveOutcome::Archived => "archived",
            ArchiveOutcome::NothingPending => "nothingPending",
        })
        .map_err(Into::into)
}
pub(crate) async fn confirm(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    recovery_revision: String,
) -> Result<(), PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::confirm_archived_recovery(db, input.probe, input.contracts, id, recovery_revision)
        .await
        .map_err(Into::into)
}
pub(crate) async fn recapture(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    recovery_revision: String,
    catalog_revision: String,
) -> Result<CaptureOutcome, PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::capture_and_confirm_recovery(
        db,
        input.probe,
        input.contracts,
        id,
        recovery_revision,
        catalog_revision,
    )
    .await
    .map_err(Into::into)
}
/// Only the explicit, selected-record irreversible-deletion UI action invokes this.
pub(crate) async fn delete(
    db: Arc<Database>,
    id: String,
    revision: String,
) -> Result<(), PublicError> {
    runtime::delete_confirmed_recovery(db, id, revision)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
