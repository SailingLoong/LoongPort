//! Existing LoongPort lifecycle owner for ZCode account operations.
//! Backend probes produce admission evidence; no real probe or IPC bypass lives here.
use super::admission::{BlockedReason, ContextObservation, ContractEntry, VerifiedContext};
use super::core::{AccountIdentity, JournalOrigin};
use super::transaction::{
    AccountStore, Admission, CaptureOutcome, CatalogStatus, SwitchOutcome, TransactionError,
};
use crate::database::Database;
use std::sync::Arc;

pub(super) trait ContextProbe: Send + Sync {
    /// Read only installation/storage/process/settings evidence, never credentials.
    fn observe(&self) -> Result<ContextObservation, BlockedReason>;
    /// Trusted local lifecycle classification, not data decoded from a journal or IPC.
    fn journal_origin(&self) -> Option<JournalOrigin>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeError {
    Blocked(BlockedReason),
    VaultUnavailable,
    OriginUnverified,
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
    Switch(AccountIdentity),
    Status,
    Capture { revision: String },
    SwitchSaved { id: String, revision: String },
    Recover,
}
enum OperationResult {
    Status(CatalogStatus),
    Captured(CaptureOutcome),
    Switched(SwitchOutcome),
}

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
pub(super) async fn recover_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
) -> Result<SwitchOutcome, RuntimeError> {
    match run(db, probe, contracts, Operation::Recover).await? {
        OperationResult::Switched(outcome) => Ok(outcome),
        _ => Err(RuntimeError::TaskFailed),
    }
}
async fn run(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    operation: Operation,
) -> Result<OperationResult, RuntimeError> {
    let sync = crate::services::sync_protocol::sync_mutex().lock().await;
    tauri::async_runtime::spawn_blocking(move || {
        // The physical worker owns the existing mutex. Dropping/cancelling its
        // caller future cannot admit another operation while this worker continues.
        let _sync = sync;
        let context = VerifiedContext::assess(probe.observe()?, &contracts)?;
        let origin = match &operation {
            Operation::Switch(target) => {
                context.accept_target(target)?;
                None
            }
            Operation::Recover => match probe.journal_origin() {
                Some(JournalOrigin::Live) => Some(JournalOrigin::Live),
                Some(JournalOrigin::Restored) => {
                    return Err(RuntimeError::Transaction(TransactionError::Imported))
                }
                None => return Err(RuntimeError::OriginUnverified),
            },
            Operation::Status | Operation::Capture { .. } | Operation::SwitchSaved { .. } => None,
        };
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
            Operation::Recover => store
                .recover(origin.ok_or(RuntimeError::OriginUnverified)?)
                .map(OperationResult::Switched)
                .map_err(Into::into),
        }
    })
    .await
    .map_err(|_| RuntimeError::TaskFailed)?
}

#[cfg(all(test, unix))]
#[path = "runtime_tests.rs"]
mod tests;
