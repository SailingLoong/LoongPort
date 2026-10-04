//! Existing LoongPort lifecycle owner for ZCode account operations.
//! Backend probes produce admission evidence; no real probe or IPC bypass lives here.
#[cfg(test)]
use super::admission::ContextObservation;
use super::admission::{BlockedReason, ContextProbe, ContractEntry, VerifiedContext};
#[cfg(test)]
use super::core::AccountIdentity;
use super::transaction::{
    AccountStore, Admission, ArchiveOutcome, CaptureOutcome, CatalogStatus, RecoveryStatus,
    SwitchOutcome, TransactionError, VaultAccountStore,
};
use crate::database::Database;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeError {
    Blocked(BlockedReason),
    VaultUnavailable,
    Transaction(TransactionError),
    TaskFailed,
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
enum Operation {
    #[cfg(test)]
    Switch(AccountIdentity),
    Status,
    Capture {
        revision: String,
    },
    SwitchSaved {
        id: String,
        revision: String,
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
    Switched(SwitchOutcome),
    Confirmed,
}

#[cfg(test)]
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
pub(super) async fn switch_saved_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    id: String,
    revision: String,
) -> Result<SwitchOutcome, RuntimeError> {
    match run(
        db,
        probe,
        contracts,
        Operation::SwitchSaved { id, revision },
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
        let context = VerifiedContext::assess(probe.observe()?, &contracts)?;
        #[cfg(test)]
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
            #[cfg(test)]
            Operation::Switch(target) => store
                .switch(&target)
                .map(OperationResult::Switched)
                .map_err(Into::into),
            Operation::Status => store
                .status()
                .map(OperationResult::Status)
                .map_err(Into::into),
            Operation::Capture { revision } => store
                .capture(context.family(), &revision)
                .map(OperationResult::Captured)
                .map_err(Into::into),
            Operation::SwitchSaved { id, revision } => store
                .switch_saved(&id, &revision, context.family())
                .map(OperationResult::Switched)
                .map_err(Into::into),
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
    tauri::async_runtime::spawn_blocking(move || {
        // Every branch shares the existing physical owner. Cancelling the caller
        // cannot release it before this worker and its session guard finish.
        let _sync = sync;
        operation(&db)
    })
    .await
    .map_err(|_| RuntimeError::TaskFailed)?
}

#[cfg(all(test, unix))]
#[path = "runtime_tests.rs"]
mod tests;
