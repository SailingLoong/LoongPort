//! Authenticated account checkpoint codecs; no file IO or key-store lifecycle.
//! Uses the single OwnedFile registry and existing vault codec.

use super::core::{
    recovery_action, AccountIdentity, AccountSnapshot, CoreError, CredentialDocument,
    JournalOrigin, RecoveryAction, StrictRecord, SwitchPlan, TransactionPhase,
};
use super::native::{NativeCipher, NativeError};
pub(crate) use crate::secrets::owned_file::{JOURNAL_FILE, PROFILE_FILE};
use crate::secrets::{
    owned_file::{OwnedFile, RECOVERY_FILE},
    VaultContext,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROFILES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointError {
    InvalidPayload,
    WrongContext,
    DuplicateIdentity,
    NoChange,
    RecoveryOnly,
    InvalidPhase,
    ResourceLimit,
    Vault,
    Native(NativeError),
    Core(CoreError),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogPayload {
    version: u32,
    context: String,
    profiles: Vec<StrictRecord<String>>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TransactionBinding {
    pub operation: String,
    pub native_root: [u64; 2],
    pub vault_root: [u64; 2],
    pub source_revision: [u8; 32],
    pub source_profile_revision: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalPayload {
    binding: Option<TransactionBinding>,
    version: u32,
    context: String,
    phase: TransactionPhase,
    source: StrictRecord<String>,
    target: StrictRecord<String>,
    before: StrictRecord<Option<String>>,
}

#[derive(Default)]
pub(crate) struct ProfileCatalog {
    profiles: BTreeMap<AccountIdentity, AccountSnapshot>,
}

impl ProfileCatalog {
    pub(crate) fn upsert(&mut self, snapshot: AccountSnapshot) {
        self.profiles.insert(snapshot.identity().clone(), snapshot);
    }

    pub(crate) fn get(&self, identity: &AccountIdentity) -> Option<&AccountSnapshot> {
        self.profiles.get(identity)
    }

    pub(crate) fn len(&self) -> usize {
        self.profiles.len()
    }

    pub(crate) fn seal(
        &self,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<String, CheckpointError> {
        if self.len() > MAX_PROFILES {
            return Err(CheckpointError::ResourceLimit);
        }
        let mut profiles = Vec::new();
        for snapshot in self.profiles.values() {
            verify_snapshot(snapshot, native)?;
            profiles.push(record(snapshot));
        }
        seal_payload(
            &CatalogPayload {
                version: 1,
                context: native.context().into(),
                profiles,
            },
            PROFILE_FILE,
            vault,
        )
    }

    pub(crate) fn open(
        encoded: &str,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<Self, CheckpointError> {
        let payload: CatalogPayload = open_payload(encoded, PROFILE_FILE, vault)?;
        validate_header(payload.version, &payload.context, native)?;
        if payload.profiles.len() > MAX_PROFILES {
            return Err(CheckpointError::ResourceLimit);
        }
        let mut catalog = Self::default();
        for record in payload.profiles {
            let snapshot = inspect_record(record, native)?;
            if catalog.get(snapshot.identity()).is_some() {
                return Err(CheckpointError::DuplicateIdentity);
            }
            catalog.upsert(snapshot);
        }
        Ok(catalog)
    }
}

pub(crate) struct SwitchCheckpoint {
    phase: TransactionPhase,
    context: String,
    plan: SwitchPlan,
    recovery_only: bool,
    binding: Option<TransactionBinding>,
}

pub(crate) enum RecoveryOutcome {
    Restore(CredentialDocument),
    ReconcileCommit,
    CleanupOnly,
    Quarantine,
}

impl SwitchCheckpoint {
    pub(super) fn bind(&mut self, binding: TransactionBinding) -> Result<(), CheckpointError> {
        if self.recovery_only || self.binding.is_some() || self.phase != TransactionPhase::Prepared
        {
            return Err(CheckpointError::InvalidPhase);
        }
        validate_binding(&binding)?;
        self.binding = Some(binding);
        Ok(())
    }
    pub(super) fn binding(&self) -> Option<&TransactionBinding> {
        self.binding.as_ref()
    }

    pub(crate) fn prepare(
        current: &CredentialDocument,
        target: &AccountSnapshot,
        native: &NativeCipher,
    ) -> Result<Self, CheckpointError> {
        let source = native.inspect(current).map_err(CheckpointError::Native)?;
        verify_snapshot(target, native)?;
        let plan = SwitchPlan::prepare(current, source.identity(), target)
            .map_err(CheckpointError::Core)?;
        if plan.is_noop() {
            return Err(CheckpointError::NoChange);
        }
        Ok(Self {
            phase: TransactionPhase::Prepared,
            context: native.context().into(),
            plan,
            recovery_only: false,
            binding: None,
        })
    }

    pub(crate) fn fresh_source(&self) -> &AccountSnapshot {
        self.plan.fresh_source()
    }

    /// The IO owner may only advance after establishing the corresponding fact.
    /// A known commit/uncertain marker can never be downgraded to a rollback phase.
    pub(crate) fn set_phase(&mut self, phase: TransactionPhase) -> Result<(), CheckpointError> {
        if phase < self.phase {
            return Err(CheckpointError::InvalidPhase);
        }
        self.phase = phase;
        Ok(())
    }

    pub(crate) fn apply(
        &self,
        current: &CredentialDocument,
    ) -> Result<CredentialDocument, CheckpointError> {
        if self.recovery_only
            || !matches!(
                self.phase,
                TransactionPhase::Prepared | TransactionPhase::Captured
            )
        {
            return Err(CheckpointError::RecoveryOnly);
        }
        self.plan.apply(current).map_err(CheckpointError::Core)
    }

    pub(crate) fn seal(
        &self,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<String, CheckpointError> {
        self.seal_at(vault, native, JOURNAL_FILE)
    }

    pub(super) fn seal_recovery(
        &self,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<String, CheckpointError> {
        self.seal_at(vault, native, RECOVERY_FILE)
    }

    fn seal_at(
        &self,
        vault: &VaultContext,
        native: &NativeCipher,
        file: &str,
    ) -> Result<String, CheckpointError> {
        validate_header(1, &self.context, native)?;
        verify_snapshot(self.plan.fresh_source(), native)?;
        verify_snapshot(self.plan.target_snapshot(), native)?;
        seal_payload(
            &JournalPayload {
                binding: self.binding.clone(),
                version: 1,
                context: self.context.clone(),
                phase: self.phase,
                source: record(self.plan.fresh_source()),
                target: record(self.plan.target_snapshot()),
                before: StrictRecord(self.plan.target_preimages()),
            },
            file,
            vault,
        )
    }

    pub(crate) fn open(
        encoded: &str,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<Self, CheckpointError> {
        Self::open_at(encoded, vault, native, JOURNAL_FILE)
    }

    pub(super) fn open_recovery(
        encoded: &str,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<Self, CheckpointError> {
        Self::open_at(encoded, vault, native, RECOVERY_FILE)
    }

    fn open_at(
        encoded: &str,
        vault: &VaultContext,
        native: &NativeCipher,
        file: &str,
    ) -> Result<Self, CheckpointError> {
        let payload: JournalPayload = open_payload(encoded, file, vault)?;
        validate_header(payload.version, &payload.context, native)?;
        if let Some(binding) = &payload.binding {
            validate_binding(binding)?;
        }
        let source = inspect_record(payload.source, native)?;
        let target = inspect_record(payload.target, native)?;
        let target_keys = target.identity().credential_keys();
        if payload.before.0.len() != target_keys.len()
            || payload
                .before
                .0
                .keys()
                .any(|key| !target_keys.contains(key))
        {
            return Err(CheckpointError::InvalidPayload);
        }
        let source_document = source.scoped_document();
        let source_keys = source.identity().credential_keys();
        let mut before = source_document.entries().clone();
        for (key, value) in payload.before.0 {
            if source_keys.contains(&key) && source_document.get(&key) != value.as_deref() {
                return Err(CheckpointError::InvalidPayload);
            }
            match value {
                Some(value) => {
                    before.insert(key, value);
                }
                None => {
                    before.remove(&key);
                }
            }
        }
        let before = document_from_record(StrictRecord(before))?;
        let plan = SwitchPlan::prepare(&before, source.identity(), &target)
            .map_err(CheckpointError::Core)?;
        if plan.is_noop() {
            return Err(CheckpointError::NoChange);
        }
        Ok(Self {
            phase: payload.phase,
            context: payload.context,
            plan,
            recovery_only: true,
            binding: payload.binding,
        })
    }

    pub(super) fn recovery_policy(&self, origin: JournalOrigin) -> RecoveryAction {
        recovery_action(origin, self.phase)
    }

    /// Origin is a trusted lifecycle input, never deserialized from the payload.
    pub(crate) fn recover(
        &self,
        current: &CredentialDocument,
        origin: JournalOrigin,
    ) -> Result<RecoveryOutcome, CheckpointError> {
        match self.recovery_policy(origin) {
            RecoveryAction::RestorePreimage => self
                .plan
                .rollback(current)
                .map(RecoveryOutcome::Restore)
                .map_err(CheckpointError::Core),
            RecoveryAction::ReconcileCommit => Ok(RecoveryOutcome::ReconcileCommit),
            RecoveryAction::CleanupOnly => Ok(RecoveryOutcome::CleanupOnly),
            RecoveryAction::Quarantine => Ok(RecoveryOutcome::Quarantine),
        }
    }
}

fn validate_header(
    version: u32,
    context: &str,
    native: &NativeCipher,
) -> Result<(), CheckpointError> {
    if version != 1 {
        return Err(CheckpointError::InvalidPayload);
    }
    if context != native.context() {
        return Err(CheckpointError::WrongContext);
    }
    Ok(())
}

fn record(snapshot: &AccountSnapshot) -> StrictRecord<String> {
    StrictRecord(snapshot.scoped_document().entries().clone())
}

fn document_from_record(
    record: StrictRecord<String>,
) -> Result<CredentialDocument, CheckpointError> {
    let bytes = serde_json::to_vec(&record).map_err(|_| CheckpointError::InvalidPayload)?;
    CredentialDocument::parse(&bytes).map_err(CheckpointError::Core)
}

fn inspect_record(
    record: StrictRecord<String>,
    native: &NativeCipher,
) -> Result<AccountSnapshot, CheckpointError> {
    let document = document_from_record(record)?;
    let snapshot = native.inspect(&document).map_err(CheckpointError::Native)?;
    let keys = snapshot.identity().credential_keys();
    if document.entries().keys().any(|key| !keys.contains(key)) {
        return Err(CheckpointError::InvalidPayload);
    }
    Ok(snapshot)
}

fn verify_snapshot(
    snapshot: &AccountSnapshot,
    native: &NativeCipher,
) -> Result<(), CheckpointError> {
    let checked = native
        .inspect(&snapshot.scoped_document())
        .map_err(CheckpointError::Native)?;
    if checked.identity() != snapshot.identity() {
        return Err(CheckpointError::WrongContext);
    }
    Ok(())
}

fn seal_payload(
    value: &impl Serialize,
    file: &str,
    vault: &VaultContext,
) -> Result<String, CheckpointError> {
    let bytes =
        Zeroizing::new(serde_json::to_vec(value).map_err(|_| CheckpointError::InvalidPayload)?);
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    let encoded = OwnedFile::registered(file)
        .and_then(|file| file.encode(vault, &bytes))
        .map_err(|_| CheckpointError::Vault)?;
    let encoded = String::from_utf8(encoded).map_err(|_| CheckpointError::Vault)?;
    if encoded.len() > MAX_ENVELOPE_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    Ok(encoded)
}

fn open_payload<T: DeserializeOwned>(
    encoded: &str,
    file: &str,
    vault: &VaultContext,
) -> Result<T, CheckpointError> {
    if encoded.len() > MAX_ENVELOPE_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    let bytes = OwnedFile::registered(file)
        .and_then(|file| file.decode(vault, encoded.as_bytes()))
        .map_err(|_| CheckpointError::Vault)?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    serde_json::from_slice(&bytes).map_err(|_| CheckpointError::InvalidPayload)
}

#[cfg(test)]
#[path = "checkpoint_tests.rs"]
mod tests;

fn validate_binding(binding: &TransactionBinding) -> Result<(), CheckpointError> {
    if uuid::Uuid::parse_str(&binding.operation)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(binding.operation.as_str())
    {
        return Err(CheckpointError::InvalidPayload);
    }
    Ok(())
}
