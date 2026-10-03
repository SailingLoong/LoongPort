//! Existing LoongPort lifecycle owner for ZCode account operations.
//! Backend probes produce admission evidence; no real probe or IPC bypass lives here.
use super::admission::{BlockedReason, ContextObservation, ContractEntry};
use super::core::{AccountIdentity, JournalOrigin};
use super::transaction::{SwitchOutcome, TransactionError};
use crate::database::Database;
use std::sync::Arc;

pub(super) trait ContextProbe: Send + Sync {
    fn observe(&self) -> Result<ContextObservation, BlockedReason>;
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
pub(super) async fn switch_account(
    _db: Arc<Database>,
    _probe: Arc<dyn ContextProbe>,
    _contracts: Vec<ContractEntry>,
    _target: AccountIdentity,
) -> Result<SwitchOutcome, RuntimeError> {
    Err(RuntimeError::TaskFailed)
}
pub(super) async fn recover_account(
    _db: Arc<Database>,
    _probe: Arc<dyn ContextProbe>,
    _contracts: Vec<ContractEntry>,
) -> Result<SwitchOutcome, RuntimeError> {
    Err(RuntimeError::TaskFailed)
}

#[cfg(all(test, unix))]
#[path = "runtime_tests.rs"]
mod tests;
