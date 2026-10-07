//! Authenticated account checkpoint codecs; no file IO or key-store lifecycle.
//! Uses the single OwnedFile registry and existing vault codec.

use super::core::{
    AccountIdentity, AccountSnapshot, CoreError, CredentialDocument, StrictRecord, SwitchPlan,
    TransactionPhase,
};
use super::native::{NativeCipher, NativeError};
use super::session_checks::{
    CheckState, EntitlementState, SessionCheckDisplay, SessionCheckReport,
};
pub(crate) use crate::secrets::owned_file::{JOURNAL_FILE, PROFILE_FILE};
use crate::secrets::{owned_file::OwnedFile, VaultContext};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use zeroize::Zeroizing;

pub(super) const MAX_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROFILES: usize = 64;
const MAX_LOGIN_RECEIPTS: usize = 1024;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unverified: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    login_receipts: Option<Vec<LoginReceipt>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<BTreeMap<String, ProfileDetails>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum IdentitySource {
    NativeCapture,
    OfficialLogin,
    PackageDeclared,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProfileDetails {
    pub label: Option<String>,
    pub identity_source: IdentitySource,
    pub evidence: Option<SessionCheckReport>,
    #[serde(default = "require_capability_default")]
    pub requires_capability_check: bool,
}
fn require_capability_default() -> bool {
    true
}
impl Default for ProfileDetails {
    fn default() -> Self {
        Self {
            label: None,
            identity_source: IdentitySource::PackageDeclared,
            evidence: None,
            requires_capability_check: false,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionKind {
    Start,
    Coding,
}
fn ready(display: &SessionCheckDisplay, kind: ConnectionKind) -> bool {
    match kind {
        ConnectionKind::Start => {
            display.start.check.state == CheckState::Accepted
                && display.start.entitlement == EntitlementState::Available
        }
        ConnectionKind::Coding => {
            display.coding.check.state == CheckState::Accepted
                && display.coding.entitlement == EntitlementState::Available
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LoginReceipt {
    pub request_id: String,
    pub account_id: String,
    pub candidate_revision: String,
    pub outcome: super::transaction::CaptureCommitOutcome,
}
impl LoginReceipt {
    fn validate(&self) -> bool {
        uuid::Uuid::parse_str(&self.request_id).is_ok()
            && [&self.account_id, &self.candidate_revision]
                .iter()
                .all(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
    }
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
    unverified: BTreeSet<AccountIdentity>,
    login_receipts: Vec<LoginReceipt>,
    details: BTreeMap<AccountIdentity, ProfileDetails>,
}

impl ProfileCatalog {
    pub(crate) fn login_receipt(&self, request_id: &str) -> Option<&LoginReceipt> {
        self.login_receipts
            .iter()
            .find(|receipt| receipt.request_id == request_id)
    }
    pub(crate) fn record_login(&mut self, receipt: LoginReceipt) -> Result<(), CheckpointError> {
        if !receipt.validate() || self.login_receipt(&receipt.request_id).is_some() {
            return Err(CheckpointError::InvalidPayload);
        }
        // Absence is used to prove that an uncertain save never committed.
        // Never evict an old receipt and accidentally manufacture that proof.
        if self.login_receipts.len() >= MAX_LOGIN_RECEIPTS {
            return Err(CheckpointError::ResourceLimit);
        }
        self.login_receipts.push(receipt);
        Ok(())
    }
    pub(crate) fn details(
        &self,
        snapshot: &AccountSnapshot,
        native: &NativeCipher,
    ) -> ProfileDetails {
        let mut details = self
            .details
            .get(snapshot.identity())
            .cloned()
            .unwrap_or_else(|| ProfileDetails {
                identity_source: if self.source_verified(snapshot.identity()) {
                    IdentitySource::NativeCapture
                } else {
                    IdentitySource::PackageDeclared
                },
                ..ProfileDetails::default()
            });
        details.evidence = details
            .evidence
            .map(|report| report.retain_matching(native, snapshot));
        details
    }
    pub(crate) fn connection_ready(
        &self,
        snapshot: &AccountSnapshot,
        native: &NativeCipher,
        kind: ConnectionKind,
    ) -> bool {
        self.details(snapshot, native)
            .evidence
            .is_some_and(|evidence| ready(&evidence.display(), kind))
    }
    pub(crate) fn can_activate(
        &self,
        snapshot: &AccountSnapshot,
        native: &NativeCipher,
        selection: Option<(ConnectionKind, &str)>,
    ) -> bool {
        let details = self.details(snapshot, native);
        if self.source_verified(snapshot.identity()) && !details.requires_capability_check {
            return true;
        }
        let Some((kind, version)) = selection else {
            return false;
        };
        details.evidence.is_some_and(|report| {
            ready(&report.display(), kind)
                && (kind != ConnectionKind::Start || report.start_checked_for(version))
        })
    }
    pub(crate) fn set_evidence(
        &mut self,
        identity: &AccountIdentity,
        native: &NativeCipher,
        evidence: SessionCheckReport,
    ) -> Result<(), CheckpointError> {
        let snapshot = self.get(identity).ok_or(CheckpointError::InvalidPayload)?;
        let mut details = self.details(snapshot, native);
        details.requires_capability_check = true;
        let evidence = evidence.retain_matching(native, snapshot);
        details.evidence = Some(match details.evidence.as_ref() {
            Some(previous) => evidence.preserve_previous_acceptance(previous, native, snapshot),
            None => evidence,
        });
        self.details.insert(identity.clone(), details);
        Ok(())
    }
    pub(crate) fn set_label(
        &mut self,
        identity: &AccountIdentity,
        native: &NativeCipher,
        label: Option<String>,
    ) -> Result<(), CheckpointError> {
        if label.as_ref().is_some_and(|value| {
            value.trim().is_empty()
                || value.chars().count() > 80
                || value.chars().any(char::is_control)
        }) {
            return Err(CheckpointError::InvalidPayload);
        }
        let snapshot = self.get(identity).ok_or(CheckpointError::InvalidPayload)?;
        let mut details = self.details(snapshot, native);
        details.label = label;
        self.details.insert(identity.clone(), details);
        Ok(())
    }
    /// Replaces a whole selected session. Failed incoming checks cannot erase
    /// existing working capabilities; no fields are copied between accounts.
    pub(crate) fn upsert_checked(
        &mut self,
        snapshot: AccountSnapshot,
        native: &NativeCipher,
        evidence: Option<SessionCheckReport>,
        origin: IdentitySource,
    ) -> bool {
        let incoming = evidence.map(|report| report.retain_matching(native, &snapshot));
        if let Some(old) = self.get(snapshot.identity()) {
            let old_details = self.details(old, native);
            let incoming_display = incoming.as_ref().map(SessionCheckReport::display);
            let any_ready = incoming_display.as_ref().is_some_and(|display| {
                ready(display, ConnectionKind::Start) || ready(display, ConnectionKind::Coding)
            });
            if self.source_verified(old.identity()) && !any_ready {
                return false;
            }
            if let Some(previous) = old_details.evidence {
                let previous = previous.display();
                for kind in [ConnectionKind::Start, ConnectionKind::Coding] {
                    if ready(&previous, kind)
                        && !incoming_display
                            .as_ref()
                            .is_some_and(|display| ready(display, kind))
                    {
                        return false;
                    }
                }
            }
        }
        let label = self
            .details
            .get(snapshot.identity())
            .and_then(|details| details.label.clone());
        self.details.insert(
            snapshot.identity().clone(),
            ProfileDetails {
                label,
                identity_source: origin,
                evidence: incoming,
                requires_capability_check: true,
            },
        );
        self.upsert_unverified(snapshot);
        true
    }
    pub(crate) fn profiles(&self) -> impl Iterator<Item = &AccountSnapshot> {
        self.profiles.values()
    }

    pub(crate) fn upsert(&mut self, snapshot: AccountSnapshot) {
        // Only the explicit native capture/transaction owners may call this.
        // Replace imported credentials with the freshly inspected local image.
        self.unverified.remove(snapshot.identity());
        if let Some(details) = self.details.get_mut(snapshot.identity()) {
            details.identity_source = IdentitySource::NativeCapture;
        }
        self.profiles.insert(snapshot.identity().clone(), snapshot);
    }

    pub(crate) fn upsert_unverified(&mut self, snapshot: AccountSnapshot) {
        self.unverified.insert(snapshot.identity().clone());
        self.profiles.insert(snapshot.identity().clone(), snapshot);
    }

    pub(crate) fn source_verified(&self, identity: &AccountIdentity) -> bool {
        self.profiles.contains_key(identity) && !self.unverified.contains(identity)
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
                version: if self.login_receipts.is_empty() && self.details.is_empty() {
                    2
                } else {
                    3
                },
                details: (!self.details.is_empty()).then(|| {
                    self.profiles
                        .values()
                        .filter(|snapshot| self.details.contains_key(snapshot.identity()))
                        .map(|snapshot| {
                            (
                                snapshot.identity().opaque_id(),
                                self.details(snapshot, native),
                            )
                        })
                        .collect()
                }),
                login_receipts: (!self.login_receipts.is_empty())
                    .then(|| self.login_receipts.clone()),
                context: native.context().into(),
                profiles,
                unverified: Some(
                    self.unverified
                        .iter()
                        .map(AccountIdentity::opaque_id)
                        .collect(),
                ),
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
        if payload.context != native.context() {
            return Err(CheckpointError::WrongContext);
        }
        match (
            payload.version,
            &payload.unverified,
            &payload.login_receipts,
            &payload.details,
        ) {
            // v1 predates bundle import; all records came from admitted capture.
            (1, None, None, None) | (2, Some(_), None, None) | (3, Some(_), _, _) => (),
            _ => return Err(CheckpointError::InvalidPayload),
        }
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
        for id in payload.unverified.unwrap_or_default() {
            let identity = catalog
                .profiles
                .keys()
                .find(|identity| identity.opaque_id() == id)
                .ok_or(CheckpointError::InvalidPayload)?
                .clone();
            if !catalog.unverified.insert(identity) {
                return Err(CheckpointError::InvalidPayload);
            }
        }
        for (id, details) in payload.details.unwrap_or_default() {
            let snapshot = catalog
                .profiles
                .values()
                .find(|snapshot| snapshot.identity().opaque_id() == id)
                .ok_or(CheckpointError::InvalidPayload)?;
            if details.label.as_ref().is_some_and(|value| {
                value.trim().is_empty()
                    || value.chars().count() > 80
                    || value.chars().any(char::is_control)
            }) {
                return Err(CheckpointError::InvalidPayload);
            }
            let identity = snapshot.identity().clone();
            let evidence = details
                .evidence
                .map(|report| report.retain_matching(native, snapshot));
            catalog.details.insert(
                identity,
                ProfileDetails {
                    evidence,
                    ..details
                },
            );
        }
        let receipts = payload.login_receipts.unwrap_or_default();
        if receipts.len() > MAX_LOGIN_RECEIPTS {
            return Err(CheckpointError::ResourceLimit);
        }
        for receipt in receipts {
            catalog.record_login(receipt)?;
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

    #[cfg(test)]
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
