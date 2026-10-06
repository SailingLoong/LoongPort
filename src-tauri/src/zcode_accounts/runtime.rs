//! Existing LoongPort lifecycle owner for ZCode account operations.
//! Backend probes produce admission evidence; no real probe or IPC bypass lives here.
#[cfg(all(test, unix))]
use super::admission::ContextObservation;
use super::admission::{
    BlockedReason, ContextProbe, ContractEntry, ReadOnlyContext, VerifiedContext,
};
use super::capture_reviews::{Binding, Reviews};
#[cfg(all(test, unix))]
use super::core::AccountIdentity;
use super::transaction::{
    AccountStore, Admission, ArchiveOutcome, CaptureCommitOutcome, CaptureOutcome, CatalogStatus,
    RecoveryStatus, SwitchOutcome, TransactionError, VaultAccountStore,
};
use crate::database::Database;
use std::sync::{Arc, Mutex, OnceLock};
use zeroize::Zeroizing;
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapturePreview {
    preview_id: String,
    id: String,
    label: Option<String>,
    family: &'static str,
    duplicate: bool,
}
fn reviews() -> &'static Mutex<Reviews> {
    static REVIEWS: OnceLock<Mutex<Reviews>> = OnceLock::new();
    REVIEWS.get_or_init(|| Mutex::new(Reviews::default()))
}
fn import_reviews() -> &'static Mutex<super::import_reviews::Reviews> {
    static REVIEWS: OnceLock<Mutex<super::import_reviews::Reviews>> = OnceLock::new();
    REVIEWS.get_or_init(|| Mutex::new(super::import_reviews::Reviews::default()))
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundlePreview {
    preview_id: String,
    rows: Vec<super::bundle_import::PreviewRow>,
}
pub(crate) struct BundlePreviewInput {
    pub revision: String,
    pub file: Zeroizing<Vec<u8>>,
    pub password: Zeroizing<String>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportChoice {
    pub index: usize,
    pub update_duplicate: bool,
}
pub(crate) struct BundleCommitInput {
    pub revision: String,
    pub preview_id: String,
    pub selected: Vec<ImportChoice>,
}

pub(super) async fn preview_bundle(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    input: BundlePreviewInput,
) -> Result<BundlePreview, RuntimeError> {
    let password = input.password;
    let file = input.file;
    run_owned(db, move |db| {
        let context = VerifiedContext::assess(probe.observe()?, &contracts)?;
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        let native = context.cipher()?;
        let store = VaultAccountStore::new(session.root(), &vault)?;
        let catalog = store.import_catalog(&native, &input.revision)?;
        let inspected = super::bundle_import::inspect(&file, &password, &native, &catalog)
            .map_err(|error| match error {
                super::bundle::BundleFailure::Authentication => RuntimeError::BundleAuthentication,
                _ => RuntimeError::BundleInvalid,
            })?;
        context.recheck(probe.observe()?, &contracts)?;
        let metadata = vault.metadata();
        let binding = Binding {
            vault_root: session.root().to_owned(),
            vault_id: metadata.vault_id.clone(),
            key_id: metadata.key_id.clone(),
            vault_revision: metadata.revision,
            context_revision: context.context_revision(),
            catalog_revision: input.revision,
        };
        let preview_id = import_reviews()
            .lock()
            .map_err(|_| RuntimeError::TaskFailed)?
            .issue(binding, inspected.accounts, std::time::Instant::now());
        // At most two decrypted candidates, each from a <=10 MiB file. Remove
        // expired secrets even when the user never submits another operation.
        let expires = preview_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
            if let Ok(mut reviews) = import_reviews().lock() {
                reviews.cancel(&expires);
            }
        });
        Ok(BundlePreview {
            preview_id,
            rows: inspected.rows,
        })
    })
    .await
}
pub(super) async fn commit_bundle(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    input: BundleCommitInput,
) -> Result<Vec<CaptureCommitOutcome>, RuntimeError> {
    run_owned(db, move |db| {
        // Consume on every submitted outcome, including admission failure.
        let mut review = import_reviews()
            .lock()
            .map_err(|_| RuntimeError::TaskFailed)?
            .consume(&input.preview_id, std::time::Instant::now())
            .ok_or(TransactionError::SourceChanged)?;
        let context = VerifiedContext::assess(probe.observe()?, &contracts)?;
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        let metadata = vault.metadata();
        let binding = Binding {
            vault_root: session.root().to_owned(),
            vault_id: metadata.vault_id.clone(),
            key_id: metadata.key_id.clone(),
            vault_revision: metadata.revision,
            context_revision: context.context_revision(),
            catalog_revision: input.revision.clone(),
        };
        if review.binding != binding || input.selected.is_empty() || input.selected.len() > 50 {
            return Err(TransactionError::SourceChanged.into());
        }
        let mut items = Vec::with_capacity(input.selected.len());
        for choice in input.selected {
            let account = review
                .accounts
                .get_mut(choice.index)
                .and_then(Option::take)
                .ok_or(TransactionError::SourceChanged)?;
            items.push((account, choice.update_duplicate));
        }
        let native = context.cipher()?;
        context.recheck(probe.observe()?, &contracts)?;
        VaultAccountStore::new(session.root(), &vault)?
            .import_profiles(&native, &input.revision, items)
            .map_err(Into::into)
    })
    .await
}
pub(super) async fn cancel_bundle_preview(
    db: Arc<Database>,
    preview_id: String,
) -> Result<(), RuntimeError> {
    run_owned(db, move |_| {
        import_reviews()
            .lock()
            .map_err(|_| RuntimeError::TaskFailed)?
            .cancel(&preview_id);
        Ok(())
    })
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeError {
    Blocked(BlockedReason),
    VaultUnavailable,
    Transaction(TransactionError),
    TaskFailed,
    BundleAuthentication,
    BundleInvalid,
    CommittedRestartFailed,
    CommittedResultUnknown,
}
impl From<BlockedReason> for RuntimeError {
    fn from(value: BlockedReason) -> Self {
        Self::Blocked(value)
    }
}
impl From<TransactionError> for RuntimeError {
    fn from(value: TransactionError) -> Self {
        Self::Transaction(value)
    }
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OperationStatus {
    pub request_id: String,
    pub phase: super::operation_log::Phase,
    pub target: String,
    pub refreshed: bool,
    pub restart_requested: bool,
}
pub(super) async fn operation_status(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    request_id: String,
) -> Result<Option<OperationStatus>, RuntimeError> {
    run_owned(db, move |db| {
        let context = ReadOnlyContext::assess(probe.observe()?, &contracts)?;
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        let local = VaultAccountStore::new(session.root(), &vault)?;
        if let Some(record) = local.operation(&request_id)? {
            if record.context_id != context.context_id()
                || record.native_root != context.root_identity()
            {
                return Err(BlockedReason::ContextChanged.into());
            }
        }
        let record = local.recover_operation(&request_id)?;
        record
            .map(|record| {
                if record.context_id != context.context_id()
                    || record.native_root != context.root_identity()
                {
                    return Err(BlockedReason::ContextChanged.into());
                }
                Ok(OperationStatus {
                    request_id: record.request_id,
                    phase: record.phase,
                    target: record.target,
                    refreshed: record.refreshed,
                    restart_requested: record.restart_requested,
                })
            })
            .transpose()
    })
    .await
}
fn coordinated_request(
    db: &Database,
    probe: &dyn ContextProbe,
    contracts: &[ContractEntry],
    id: &str,
    revision: &str,
    request_id: &str,
) -> Result<SwitchOutcome, RuntimeError> {
    use super::operation_log::{Phase, Record};
    if uuid::Uuid::parse_str(request_id)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(request_id)
    {
        return Err(TransactionError::SourceChanged.into());
    }
    let context = ReadOnlyContext::assess(probe.observe()?, contracts)?;
    let session = db.secret_session();
    let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
    let local = VaultAccountStore::new(session.root(), &vault)?;
    if let Some(record) = local.operation(request_id)? {
        if record.context_id != context.context_id()
            || record.native_root != context.root_identity()
            || record.target != id
        {
            return Err(TransactionError::SourceChanged.into());
        }
        return Err(TransactionError::OperationAlreadyKnown.into());
    }
    if local.operation_known(request_id)? {
        return Err(TransactionError::OperationAlreadyKnown.into());
    }
    let native = context.vault_cipher()?;
    let catalog = local.import_catalog(&native, revision)?;
    let target = catalog
        .profiles()
        .find(|p| p.identity().opaque_id() == id)
        .ok_or(TransactionError::MissingTarget)?;
    if !target
        .identity()
        .matches_scope(context.context_id(), context.family())
    {
        return Err(TransactionError::UnsupportedScope.into());
    }
    if !catalog.source_verified(target.identity()) {
        return Err(TransactionError::UnverifiedSource.into());
    }
    let recovery = local.status()?;
    if recovery.pending {
        return Err(TransactionError::RecoveryRequired.into());
    }
    if recovery.native_unconfirmed {
        return Err(TransactionError::NativeUnconfirmed.into());
    }
    let mut record = Record {
        request_id: request_id.to_owned(),
        context_id: context.context_id().to_owned(),
        native_root: context.root_identity(),
        target: id.to_owned(),
        phase: Phase::Accepted,
        refreshed: false,
        restart_requested: false,
    };
    local.record_operation(record.clone())?;
    let mut transaction_started = false;
    let result = (|| {
        let restart = probe.prepare_switch()?;
        record.restart_requested = restart;
        let stopped = context.stopped(probe.observe()?, contracts)?;
        record.phase = Phase::Exited;
        local.record_operation(record.clone())?;
        let native = stopped.cipher()?;
        let gate = || stopped.recheck(probe.observe()?, contracts);
        let store = AccountStore::new_guarded(
            stopped.native_root(),
            session.root(),
            &vault,
            &native,
            Admission {
                contract_verified: true,
                app_stopped: true,
                native_gate_passed: true,
                individual_scope_verified: true,
            },
            &gate,
        )?;
        stopped.confirm_root(store.native_root_identity())?;
        record.phase = Phase::TransactionUncertain;
        local.record_operation(record.clone())?;
        let outcome = store.switch_saved_operation_started(
            id,
            revision,
            stopped.family(),
            request_id,
            &mut || {
                transaction_started = true;
            },
        )?;
        record.phase = Phase::Committed;
        record.refreshed = outcome == SwitchOutcome::Refreshed;
        local
            .record_operation(record.clone())
            .map_err(|_| RuntimeError::CommittedResultUnknown)?;
        if restart {
            if probe.restart_after_switch().is_err() {
                record.phase = Phase::RestartFailed;
                local
                    .record_operation(record.clone())
                    .map_err(|_| RuntimeError::CommittedResultUnknown)?;
                return Err(RuntimeError::CommittedRestartFailed);
            }
            record.phase = Phase::RestartVerified;
            local
                .record_operation(record.clone())
                .map_err(|_| RuntimeError::CommittedResultUnknown)?;
        }
        Ok(outcome)
    })();
    if let Err(error) = &result {
        if matches!(
            error,
            RuntimeError::Transaction(TransactionError::CommittedNeedsCleanup)
        ) {
            record.phase = Phase::Committed;
        } else if !matches!(
            record.phase,
            Phase::Committed | Phase::RestartVerified | Phase::RestartFailed
        ) {
            record.phase = if transaction_started {
                Phase::TransactionUncertain
            } else {
                Phase::Failed
            };
        }
        // Preserve the original stage if publishing a failure result itself fails.
        // Its caller can query authenticated journal/recovery evidence later.
        let _ = local.record_operation(record);
    }
    result
}
enum Operation {
    #[cfg(all(test, unix))]
    Switch(AccountIdentity),
    Status,
    #[cfg(all(test, unix))]
    Capture {
        revision: String,
    },
    PreviewCapture {
        revision: String,
    },
    CaptureReviewed {
        revision: String,
        preview_id: String,
        update_duplicate: bool,
    },
    SwitchSaved {
        id: String,
        revision: String,
        request_id: String,
    },
    ConfirmArchived {
        id: String,
        recovery_revision: String,
    },
    CaptureAndConfirm {
        id: String,
        recovery_revision: String,
        catalog_revision: String,
    },
}
enum OperationResult {
    Status(CatalogStatus),
    Captured(CaptureOutcome),
    CapturePreview(CapturePreview),
    CaptureCommitted(CaptureCommitOutcome),
    Switched(SwitchOutcome),
    Confirmed,
}

#[cfg(all(test, unix))]
pub(super) async fn switch_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    target: AccountIdentity,
) -> Result<SwitchOutcome, RuntimeError> {
    match run(db, probe, contracts, Operation::Switch(target)).await? {
        OperationResult::Switched(outcome) => Ok(outcome),
        _ => Err(RuntimeError::TaskFailed),
    }
}
pub(super) async fn account_status(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
) -> Result<CatalogStatus, RuntimeError> {
    match run(db, probe, contracts, Operation::Status).await? {
        OperationResult::Status(status) => Ok(status),
        _ => Err(RuntimeError::TaskFailed),
    }
}
/// Only an explicit capture action calls this entry; passive status never does.
#[cfg(all(test, unix))]
pub(super) async fn capture_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    revision: String,
) -> Result<CaptureOutcome, RuntimeError> {
    match run(db, probe, contracts, Operation::Capture { revision }).await? {
        OperationResult::Captured(outcome) => Ok(outcome),
        _ => Err(RuntimeError::TaskFailed),
    }
}
pub(super) async fn preview_capture_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    revision: String,
) -> Result<CapturePreview, RuntimeError> {
    match run(db, probe, contracts, Operation::PreviewCapture { revision }).await? {
        OperationResult::CapturePreview(preview) => Ok(preview),
        _ => Err(RuntimeError::TaskFailed),
    }
}
pub(super) async fn capture_reviewed_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    revision: String,
    preview_id: String,
    update_duplicate: bool,
) -> Result<CaptureCommitOutcome, RuntimeError> {
    match run(
        db,
        probe,
        contracts,
        Operation::CaptureReviewed {
            revision,
            preview_id,
            update_duplicate,
        },
    )
    .await?
    {
        OperationResult::CaptureCommitted(outcome) => Ok(outcome),
        _ => Err(RuntimeError::TaskFailed),
    }
}
pub(super) async fn cancel_capture_preview(
    db: Arc<Database>,
    preview_id: String,
) -> Result<(), RuntimeError> {
    run_owned(db, move |_| {
        reviews()
            .lock()
            .map_err(|_| RuntimeError::TaskFailed)?
            .cancel(&preview_id);
        Ok(())
    })
    .await
}
#[cfg(all(test, unix))]
pub(super) async fn switch_saved_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    id: String,
    revision: String,
) -> Result<SwitchOutcome, RuntimeError> {
    switch_account_request(
        db,
        probe,
        contracts,
        id,
        revision,
        uuid::Uuid::new_v4().to_string(),
    )
    .await
}
pub(super) async fn switch_account_request(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    id: String,
    revision: String,
    request_id: String,
) -> Result<SwitchOutcome, RuntimeError> {
    match run(
        db,
        probe,
        contracts,
        Operation::SwitchSaved {
            id,
            revision,
            request_id,
        },
    )
    .await?
    {
        OperationResult::Switched(outcome) => Ok(outcome),
        _ => Err(RuntimeError::TaskFailed),
    }
}
/// Local recovery status does not inspect the ZCode installation or credentials.
pub(super) async fn recovery_status(db: Arc<Database>) -> Result<RecoveryStatus, RuntimeError> {
    run_vault(db, |store| store.status()).await
}
/// Explicitly retain the native state and archive the authenticated pending record.
pub(super) async fn archive_pending_recovery(
    db: Arc<Database>,
    revision: String,
) -> Result<ArchiveOutcome, RuntimeError> {
    run_vault(db, move |store| store.archive_pending(&revision)).await
}
/// The caller must obtain per-action confirmation naming this record and explaining
/// permanent loss before invoking this operation. No native credential is changed.
pub(super) async fn delete_confirmed_recovery(
    db: Arc<Database>,
    id: String,
    revision: String,
) -> Result<(), RuntimeError> {
    run_vault(db, move |store| store.delete_confirmed(&id, &revision)).await
}
pub(super) async fn confirm_archived_recovery(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    id: String,
    recovery_revision: String,
) -> Result<(), RuntimeError> {
    match run(
        db,
        probe,
        contracts,
        Operation::ConfirmArchived {
            id,
            recovery_revision,
        },
    )
    .await?
    {
        OperationResult::Confirmed => Ok(()),
        _ => Err(RuntimeError::TaskFailed),
    }
}
/// A new explicit capture authorization saves the current native login before
/// resolving the selected recovery record; passive confirmation never captures.
pub(super) async fn capture_and_confirm_recovery(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    id: String,
    recovery_revision: String,
    catalog_revision: String,
) -> Result<CaptureOutcome, RuntimeError> {
    match run(
        db,
        probe,
        contracts,
        Operation::CaptureAndConfirm {
            id,
            recovery_revision,
            catalog_revision,
        },
    )
    .await?
    {
        OperationResult::Captured(outcome) => Ok(outcome),
        _ => Err(RuntimeError::TaskFailed),
    }
}
async fn run_vault<T: Send + 'static>(
    db: Arc<Database>,
    operation: impl FnOnce(&VaultAccountStore<'_>) -> Result<T, TransactionError> + Send + 'static,
) -> Result<T, RuntimeError> {
    run_owned(db, move |db| {
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        let store = VaultAccountStore::new(session.root(), &vault)?;
        operation(&store).map_err(Into::into)
    })
    .await
}
async fn run(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    operation: Operation,
) -> Result<OperationResult, RuntimeError> {
    run_owned(db, move |db| {
        // Consume before any admission/IO failure. A failed submit must be reviewed anew.
        let capture_review = if let Operation::CaptureReviewed { preview_id, .. } = &operation {
            Some(
                reviews()
                    .lock()
                    .map_err(|_| RuntimeError::TaskFailed)?
                    .consume(preview_id, std::time::Instant::now())
                    .ok_or(TransactionError::SourceChanged)?,
            )
        } else {
            None
        };
        if matches!(operation, Operation::Status) {
            let context = ReadOnlyContext::assess(probe.observe()?, &contracts)?;
            let session = db.secret_session();
            let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
            let native = context.vault_cipher()?;
            let result = VaultAccountStore::new(session.root(), &vault)?.catalog_status(&native)?;
            let fresh = ReadOnlyContext::assess(probe.observe()?, &contracts)?;
            if fresh.context_revision() != context.context_revision() {
                return Err(BlockedReason::ContextChanged.into());
            }
            return Ok(OperationResult::Status(result));
        }
        if let Operation::SwitchSaved {
            id,
            revision,
            request_id,
        } = &operation
        {
            return coordinated_request(db, probe.as_ref(), &contracts, id, revision, request_id)
                .map(OperationResult::Switched);
        }
        let context = VerifiedContext::assess(probe.observe()?, &contracts)?;
        #[cfg(all(test, unix))]
        if let Operation::Switch(target) = &operation {
            context.accept_target(target)?;
        }
        // Admission is established before obtaining a cipher or reading a native
        // credential. The existing session guard fixes the vault generation through IO.
        let session = db.secret_session();
        let vault = session.read().map_err(|_| RuntimeError::VaultUnavailable)?;
        let native = context.cipher()?;
        let gate = || context.recheck(probe.observe()?, &contracts);
        let store = AccountStore::new_guarded(
            context.native_root(),
            session.root(),
            &vault,
            &native,
            Admission {
                contract_verified: true,
                app_stopped: true,
                native_gate_passed: true,
                individual_scope_verified: true,
            },
            &gate,
        )?;
        context.confirm_root(store.native_root_identity())?;
        match operation {
            #[cfg(all(test, unix))]
            Operation::Switch(target) => store
                .switch(&target)
                .map(OperationResult::Switched)
                .map_err(Into::into),
            Operation::Status => store
                .status()
                .map(OperationResult::Status)
                .map_err(Into::into),
            #[cfg(all(test, unix))]
            Operation::Capture { revision } => store
                .capture(context.family(), &revision)
                .map(OperationResult::Captured)
                .map_err(Into::into),
            Operation::PreviewCapture { revision } => {
                let preview = store.preview_capture(context.family(), &revision)?;
                let metadata = vault.metadata();
                let binding = Binding {
                    vault_root: session.root().to_owned(),
                    vault_id: metadata.vault_id.clone(),
                    key_id: metadata.key_id.clone(),
                    vault_revision: metadata.revision,
                    context_revision: context.context_revision(),
                    catalog_revision: revision,
                };
                let preview_id = reviews()
                    .lock()
                    .map_err(|_| RuntimeError::TaskFailed)?
                    .issue(
                        binding,
                        preview.native_revision,
                        preview.id.clone(),
                        std::time::Instant::now(),
                    );
                Ok(OperationResult::CapturePreview(CapturePreview {
                    preview_id,
                    id: preview.id,
                    label: preview.label,
                    family: preview.family,
                    duplicate: preview.duplicate,
                }))
            }
            Operation::CaptureReviewed {
                revision,
                preview_id: _,
                update_duplicate,
            } => {
                let metadata = vault.metadata();
                let binding = Binding {
                    vault_root: session.root().to_owned(),
                    vault_id: metadata.vault_id.clone(),
                    key_id: metadata.key_id.clone(),
                    vault_revision: metadata.revision,
                    context_revision: context.context_revision(),
                    catalog_revision: revision.clone(),
                };
                let review = capture_review.ok_or(TransactionError::SourceChanged)?;
                if review.binding != binding {
                    return Err(TransactionError::SourceChanged.into());
                }
                store
                    .capture_reviewed(
                        context.family(),
                        &revision,
                        &review.native_revision,
                        &review.identity,
                        update_duplicate,
                    )
                    .map(OperationResult::CaptureCommitted)
                    .map_err(Into::into)
            }
            Operation::SwitchSaved { .. } => Err(RuntimeError::TaskFailed),
            Operation::ConfirmArchived {
                id,
                recovery_revision,
            } => store
                .confirm_archived(&id, &recovery_revision)
                .map(|()| OperationResult::Confirmed)
                .map_err(Into::into),
            Operation::CaptureAndConfirm {
                id,
                recovery_revision,
                catalog_revision,
            } => store
                .capture_and_confirm(&id, &recovery_revision, &catalog_revision, context.family())
                .map(OperationResult::Captured)
                .map_err(Into::into),
        }
    })
    .await
}
pub(super) async fn run_owned<T: Send + 'static>(
    db: Arc<Database>,
    operation: impl FnOnce(&Database) -> Result<T, RuntimeError> + Send + 'static,
) -> Result<T, RuntimeError> {
    let sync = crate::services::sync_protocol::sync_mutex().lock().await;
    #[cfg(all(test, unix))]
    let owner_entered = tests::OWNER_ENTERED.try_with(Arc::clone).ok();
    tauri::async_runtime::spawn_blocking(move || {
        // Every branch shares the existing physical owner. Cancelling the caller
        // cannot release it before this worker and its session guard finish.
        let _sync = sync;
        #[cfg(all(test, unix))]
        if let Some(owner_entered) = owner_entered {
            owner_entered.notify_one();
        }
        operation(&db)
    })
    .await
    .map_err(|_| RuntimeError::TaskFailed)?
}

#[cfg(all(test, unix))]
#[path = "runtime_tests.rs"]
mod tests;
