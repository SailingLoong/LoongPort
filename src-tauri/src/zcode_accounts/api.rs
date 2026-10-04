//! Thin command-facing account API. Only explicit actions enter native credential IO.
use super::{
    admission::{
        BlockedReason, ContextObservation, ContextProbe, ContractEntry, Remedy, VerifiedContext,
    },
    checkpoint::CheckpointError,
    core::{CoreError, OAuthFamily},
    native_context::{supported_contracts, ContextSelection, NativeContextProbe},
    recovery::RecoveryError,
    runtime::{self, RuntimeError},
    transaction::{
        ArchiveOutcome, CaptureOutcome, CatalogStatus, RecoveryStatus, SwitchOutcome,
        TransactionError,
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
    fn new(code: &'static str, remedy: &'static str) -> Self {
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
                Self::new("zcode.account.not_admitted", "refreshContext")
            }
            TransactionError::UnsupportedScope => {
                Self::new("zcode.account.unsupported_scope", "openNativeSettings")
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
impl ContextProbe for DeferredNativeProbe {
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
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        let mut observed = self.inner.observe()?;
        let checked =
            VerifiedContext::assess(observed.clone(), &self.contracts).and_then(|context| {
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
    let context = VerifiedContext::assess(observed, contracts)?;
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
pub(crate) async fn capture(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    catalog_revision: String,
) -> Result<CaptureOutcome, PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::capture_account(db, input.probe, input.contracts, catalog_revision)
        .await
        .map_err(Into::into)
}
pub(crate) async fn switch_saved(
    db: Arc<Database>,
    source: ContextSelection,
    context_revision: String,
    id: String,
    catalog_revision: String,
) -> Result<&'static str, PublicError> {
    let input = inputs(source, context_revision)?;
    runtime::switch_saved_account(db, input.probe, input.contracts, id, catalog_revision)
        .await
        .map(|result| match result {
            SwitchOutcome::Switched => "switched",
            SwitchOutcome::Refreshed => "refreshed",
        })
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
