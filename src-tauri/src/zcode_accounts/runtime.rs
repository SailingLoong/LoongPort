//! Existing LoongPort lifecycle owner for ZCode account operations.
//! Backend probes produce admission evidence; no real probe or IPC bypass lives here.
use super::admission::{BlockedReason, ContextObservation, ContractEntry, VerifiedContext};
use super::core::{AccountIdentity, JournalOrigin};
use super::transaction::{AccountStore, Admission, SwitchOutcome, TransactionError};
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
    Recover,
}

pub(super) async fn switch_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    target: AccountIdentity,
) -> Result<SwitchOutcome, RuntimeError> {
    run(db, probe, contracts, Operation::Switch(target)).await
}
pub(super) async fn recover_account(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
) -> Result<SwitchOutcome, RuntimeError> {
    run(db, probe, contracts, Operation::Recover).await
}
async fn run(
    db: Arc<Database>,
    probe: Arc<dyn ContextProbe>,
    contracts: Vec<ContractEntry>,
    operation: Operation,
) -> Result<SwitchOutcome, RuntimeError> {
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
            Operation::Switch(target) => store.switch(&target).map_err(Into::into),
            Operation::Recover => store
                .recover(origin.ok_or(RuntimeError::OriginUnverified)?)
                .map_err(Into::into),
        }
    })
    .await
    .map_err(|_| RuntimeError::TaskFailed)?
}

#[cfg(all(test, unix))]
#[path = "runtime_tests.rs"]
mod tests;
