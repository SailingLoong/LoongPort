//! Authenticated account checkpoint codecs; no file IO or key-store lifecycle.
//! Uses the single OwnedFile registry and existing vault codec.

use super::core::{
    AccountIdentity, AccountSnapshot, CoreError, CredentialDocument, StrictRecord, SwitchPlan,
    TransactionPhase,
};
use super::native::{NativeCipher, NativeError};
pub(crate) use crate::secrets::owned_file::{JOURNAL_FILE, PROFILE_FILE};
use crate::secrets::{owned_file::OwnedFile, VaultContext};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub(super) const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;
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
pub(super) struct JournalPayload {
    pub(super) binding: Option<TransactionBinding>,
    version: u32,
    pub(super) context: String,
    pub(super) phase: TransactionPhase,
    source: StrictRecord<String>,
    target: StrictRecord<String>,
    before: StrictRecord<Option<String>>,
}

#[derive(Default)]
pub(crate) struct ProfileCatalog {
    profiles: BTreeMap<AccountIdentity, AccountSnapshot>,
}

impl ProfileCatalog {
    pub(crate) fn profiles(&self) -> impl Iterator<Item = &AccountSnapshot> {
        self.profiles.values()
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TouchedImage {
    Before,
    After,
}

pub(crate) struct SwitchCheckpoint {
    phase: TransactionPhase,
    context: String,
    plan: SwitchPlan,
    recovery_only: bool,
    binding: Option<TransactionBinding>,
}

impl SwitchCheckpoint {
    /// Compare complete touched images, including missing values and target-only
    /// preimages. A mixture of individually authentic before/after values fails.
    pub(crate) fn match_touched_image(&self, current: &CredentialDocument) -> Option<TouchedImage> {
        let before = self.plan.target_preimages();
        if before
            .iter()
            .all(|(key, value)| current.get(key) == value.as_deref())
        {
            return Some(TouchedImage::Before);
        }
        let after = self.plan.target_snapshot().scoped_document();
        if before.keys().all(|key| current.get(key) == after.get(key)) {
            return Some(TouchedImage::After);
        }
        None
    }

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
    /// A known commit/uncertain marker can never be downgraded to an earlier phase.
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
            JOURNAL_FILE,
            vault,
        )
    }

    pub(crate) fn open(
        encoded: &str,
        vault: &VaultContext,
        native: &NativeCipher,
    ) -> Result<Self, CheckpointError> {
        Self::from_payload_bytes(&open_payload_bytes(encoded, JOURNAL_FILE, vault)?, native)
    }

    pub(super) fn from_payload_bytes(
        bytes: &[u8],
        native: &NativeCipher,
    ) -> Result<Self, CheckpointError> {
        let payload = parse_journal_payload(bytes)?;
        validate_header(payload.version, &payload.context, native)?;
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

pub(super) fn encode_payload(
    value: &impl Serialize,
) -> Result<Zeroizing<Vec<u8>>, CheckpointError> {
    let bytes =
        Zeroizing::new(serde_json::to_vec(value).map_err(|_| CheckpointError::InvalidPayload)?);
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    Ok(bytes)
}

pub(super) fn seal_payload(
    value: &impl Serialize,
    file: &str,
    vault: &VaultContext,
) -> Result<String, CheckpointError> {
    let bytes = encode_payload(value)?;
    let encoded = OwnedFile::registered(file)
        .and_then(|file| file.encode(vault, &bytes))
        .map_err(|_| CheckpointError::Vault)?;
    let encoded = String::from_utf8(encoded).map_err(|_| CheckpointError::Vault)?;
    if encoded.len() > MAX_ENVELOPE_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    Ok(encoded)
}

pub(super) fn open_payload_bytes(
    encoded: &str,
    file: &str,
    vault: &VaultContext,
) -> Result<Zeroizing<Vec<u8>>, CheckpointError> {
    if encoded.len() > MAX_ENVELOPE_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    let bytes = OwnedFile::registered(file)
        .and_then(|file| file.decode(vault, encoded.as_bytes()))
        .map_err(|_| CheckpointError::Vault)?;
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    Ok(bytes)
}

pub(super) fn open_payload<T: DeserializeOwned>(
    encoded: &str,
    file: &str,
    vault: &VaultContext,
) -> Result<T, CheckpointError> {
    serde_json::from_slice(&open_payload_bytes(encoded, file, vault)?)
        .map_err(|_| CheckpointError::InvalidPayload)
}

/// A supported authenticated journal can be preserved without decrypting native
/// credentials. Account identity and scope are checked separately with NativeCipher.
pub(super) fn parse_journal_payload(bytes: &[u8]) -> Result<JournalPayload, CheckpointError> {
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CheckpointError::ResourceLimit);
    }
    let payload: JournalPayload =
        serde_json::from_slice(bytes).map_err(|_| CheckpointError::InvalidPayload)?;
    if payload.version != 1 || payload.context.trim().is_empty() {
        return Err(CheckpointError::InvalidPayload);
    }
    if let Some(binding) = &payload.binding {
        validate_binding(binding)?;
    }
    Ok(payload)
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
