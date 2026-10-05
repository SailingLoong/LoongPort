//! Bounded authenticated recovery evidence. This module has no IO, native replay,
//! key-store access, or authority to establish remote token validity.
use super::checkpoint::{
    encode_payload, open_payload, open_payload_bytes, parse_journal_payload, seal_payload,
    CheckpointError, SwitchCheckpoint, TouchedImage, TransactionBinding, JOURNAL_FILE,
};
use super::core::{AccountSnapshot, CredentialDocument, TransactionPhase};
use super::native::NativeCipher;
use crate::secrets::{owned_file::RECOVERY_FILE, VaultContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const MAX_ARCHIVED: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryError {
    Checkpoint(CheckpointError),
    ArchiveFull,
    DuplicateRecord,
    Unconfirmed,
    ConfirmationMismatch,
    NotFound,
}
impl From<CheckpointError> for RecoveryError {
    fn from(error: CheckpointError) -> Self {
        Self::Checkpoint(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispositionKind {
    NativeUnconfirmed,
    FullBefore,
    FullAfter,
    ExplicitCapture,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Disposition {
    NativeUnconfirmed {},
    FullBefore {},
    FullAfter {},
    ExplicitCapture {
        profile_id: String,
        snapshot_revision: [u8; 32],
        profile_revision: [u8; 32],
        native_revision: [u8; 32],
    },
}

/// Backend-only proof. No deserializer or public flag can create a confirmation.
/// The IO owner must enforce native admission, user capture intent, durable
/// profile publication and final native/recovery FileImage CAS before persisting.
pub(crate) struct RecoveryConfirmation {
    evidence_id: String,
    disposition: Disposition,
}
impl RecoveryConfirmation {
    pub(crate) fn from_image(
        evidence: &JournalEvidence,
        current: &CredentialDocument,
        native: &NativeCipher,
    ) -> Result<Self, RecoveryError> {
        let checkpoint = evidence.native_checkpoint(native)?;
        let disposition = match checkpoint.match_touched_image(current) {
            Some(TouchedImage::Before) => Disposition::FullBefore {},
            Some(TouchedImage::After) => Disposition::FullAfter {},
            None => return Err(RecoveryError::ConfirmationMismatch),
        };
        Ok(Self {
            evidence_id: evidence.id().to_owned(),
            disposition,
        })
    }

    pub(crate) fn after_persisted_capture(
        evidence: &JournalEvidence,
        current: &CredentialDocument,
        persisted: &AccountSnapshot,
        native: &NativeCipher,
        profile_revision: [u8; 32],
        native_revision: [u8; 32],
    ) -> Result<Self, RecoveryError> {
        if evidence.context() != native.context() {
            return Err(CheckpointError::WrongContext.into());
        }
        let fresh = native.inspect(current).map_err(CheckpointError::Native)?;
        if fresh.identity() != persisted.identity()
            || fresh.scoped_document() != persisted.scoped_document()
        {
            return Err(RecoveryError::ConfirmationMismatch);
        }
        let snapshot = fresh
            .scoped_document()
            .to_bytes()
            .map_err(CheckpointError::Core)?;
        Ok(Self {
            evidence_id: evidence.id().to_owned(),
            disposition: Disposition::ExplicitCapture {
                profile_id: fresh.identity().opaque_id(),
                snapshot_revision: Sha256::digest(&snapshot).into(),
                profile_revision,
                native_revision,
            },
        })
    }
}

/// Original supported plaintext bytes, authenticated by the existing journal AAD.
/// No native semantic check or remote validity is implied by this type.
#[derive(Clone)]
pub(crate) struct JournalEvidence {
    raw: Zeroizing<String>,
    id: String,
    binding: Option<TransactionBinding>,
    phase: TransactionPhase,
    context: String,
}
impl JournalEvidence {
    pub(crate) fn open_journal(encoded: &str, vault: &VaultContext) -> Result<Self, RecoveryError> {
        Self::from_authenticated_payload(&open_payload_bytes(encoded, JOURNAL_FILE, vault)?)
    }
    fn from_authenticated_payload(raw: &[u8]) -> Result<Self, RecoveryError> {
        let payload = parse_journal_payload(raw)?;
        let raw = Zeroizing::new(
            std::str::from_utf8(raw)
                .map_err(|_| CheckpointError::InvalidPayload)?
                .to_owned(),
        );
        let mut digest = Sha256::new();
        digest.update(b"zcode-recovery-evidence:v1\0");
        digest.update(raw.as_bytes());
        let id = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Self {
            raw,
            id,
            binding: payload.binding,
            phase: payload.phase,
            context: payload.context,
        })
    }
    pub(crate) fn raw_payload(&self) -> &[u8] {
        self.raw.as_bytes()
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn context(&self) -> &str {
        &self.context
    }
    pub(super) fn binding(&self) -> Option<&TransactionBinding> {
        self.binding.as_ref()
    }
    pub(crate) fn phase(&self) -> TransactionPhase {
        self.phase
    }
    pub(crate) fn native_checkpoint(
        &self,
        native: &NativeCipher,
    ) -> Result<SwitchCheckpoint, RecoveryError> {
        SwitchCheckpoint::from_payload_bytes(self.raw_payload(), native).map_err(Into::into)
    }
}

#[derive(Clone)]
pub(crate) struct RecoveryRecord {
    evidence: JournalEvidence,
    disposition: Disposition,
}
impl RecoveryRecord {
    pub(crate) fn evidence(&self) -> &JournalEvidence {
        &self.evidence
    }
    pub(crate) fn disposition(&self) -> DispositionKind {
        match self.disposition {
            Disposition::NativeUnconfirmed {} => DispositionKind::NativeUnconfirmed,
            Disposition::FullBefore {} => DispositionKind::FullBefore,
            Disposition::FullAfter {} => DispositionKind::FullAfter,
            Disposition::ExplicitCapture { .. } => DispositionKind::ExplicitCapture,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordPayload {
    journal: String,
    disposition: Disposition,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerPayload {
    version: u32,
    #[serde(deserialize_with = "required_optional_record")]
    latest_completed: Option<RecordPayload>,
    archived: Vec<RecordPayload>,
}

fn required_optional_record<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RecordPayload>, D::Error> {
    Option::<RecordPayload>::deserialize(deserializer)
}

#[derive(Clone, Default)]
pub(crate) struct RecoveryLedger {
    latest_completed: Option<RecoveryRecord>,
    archived: Vec<RecoveryRecord>,
}
impl RecoveryLedger {
    /// Reserve both interruption and completion shapes before any native/profile
    /// publication. The binding must already contain all operation metadata.
    pub(crate) fn ensure_switch_capacity(
        &self,
        candidate: &JournalEvidence,
        vault: &VaultContext,
    ) -> Result<(), RecoveryError> {
        if self.needs_confirmation() {
            return Err(RecoveryError::Unconfirmed);
        }
        if candidate.phase() != TransactionPhase::Prepared || candidate.binding().is_none() {
            return Err(CheckpointError::InvalidPhase.into());
        }
        // Include original bytes as well as every supported canonical later phase.
        // Compare actual serialized retention shapes, then encrypt the largest one:
        // the existing codec has fixed-length nonce and authenticated envelope IDs.
        let mut largest_archive = None;
        self.check_archive_capacity(candidate.clone(), &mut largest_archive)?;
        let mut completion = None;
        for phase in [
            TransactionPhase::Prepared,
            TransactionPhase::Captured,
            TransactionPhase::CredentialsPublished,
            TransactionPhase::CommitUncertain,
            TransactionPhase::Committed,
        ] {
            let mut payload = parse_journal_payload(candidate.raw_payload())?;
            payload.phase = phase;
            let evidence = JournalEvidence::from_authenticated_payload(&encode_payload(&payload)?)?;
            self.check_archive_capacity(evidence.clone(), &mut largest_archive)?;
            if phase == TransactionPhase::Committed {
                let mut next = self.clone();
                next.record_completion(evidence)?;
                completion = Some(next);
            }
        }
        if let Some((_, largest)) = largest_archive {
            largest.seal(vault)?;
        }
        if let Some(completion) = completion {
            completion.seal(vault)?;
        }
        Ok(())
    }

    fn check_archive_capacity(
        &self,
        evidence: JournalEvidence,
        largest: &mut Option<(usize, Self)>,
    ) -> Result<(), RecoveryError> {
        let mut archive = self.clone();
        archive.archive(evidence)?;
        let archive = archive.with_largest_confirmations();
        let length = encode_payload(&archive.payload())?.len();
        if largest
            .as_ref()
            .is_none_or(|(previous, _)| *previous < length)
        {
            *largest = Some((length, archive));
        }
        Ok(())
    }

    pub(crate) fn confirm(
        &mut self,
        id: &str,
        confirmation: RecoveryConfirmation,
    ) -> Result<(), RecoveryError> {
        if id != confirmation.evidence_id {
            return Err(RecoveryError::ConfirmationMismatch);
        }
        let mut next = self.clone();
        next.records_mut()
            .find(|r| r.evidence.id() == id)
            .ok_or(RecoveryError::NotFound)?
            .disposition = confirmation.disposition;
        next.validate_size()?;
        *self = next;
        Ok(())
    }

    /// This only edits the in-memory candidate. The IO owner requires explicit
    /// irreversible-deletion approval and a current whole-container revision CAS.
    pub(crate) fn delete_confirmed(&mut self, id: &str) -> Result<(), RecoveryError> {
        let record = self
            .records()
            .find(|r| r.evidence.id() == id)
            .ok_or(RecoveryError::NotFound)?;
        if matches!(record.disposition, Disposition::NativeUnconfirmed {}) {
            return Err(RecoveryError::Unconfirmed);
        }
        if self
            .latest_completed
            .as_ref()
            .is_some_and(|r| r.evidence.id() == id)
        {
            self.latest_completed = None;
        } else {
            self.archived.retain(|r| r.evidence.id() != id);
        }
        Ok(())
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.latest_completed.is_none() && self.archived.is_empty()
    }

    pub(crate) fn archive(&mut self, evidence: JournalEvidence) -> Result<String, RecoveryError> {
        let id = evidence.id().to_owned();
        let mut next = self.clone();
        let existing = next.records_mut().find(|r| r.evidence.id() == id);
        if let Some(existing) = existing {
            if existing.evidence.raw_payload() != evidence.raw_payload() {
                return Err(RecoveryError::DuplicateRecord);
            }
            // A repeated/restored J is not proof that an earlier confirmation is current.
            existing.disposition = Disposition::NativeUnconfirmed {};
        } else {
            if next.archived.len() == MAX_ARCHIVED {
                return Err(RecoveryError::ArchiveFull);
            }
            next.archived.push(RecoveryRecord {
                evidence,
                disposition: Disposition::NativeUnconfirmed {},
            });
        }
        next.with_largest_confirmations().validate_size()?;
        next.validate_size()?;
        *self = next;
        Ok(id)
    }
    pub(crate) fn record_completion(
        &mut self,
        evidence: JournalEvidence,
    ) -> Result<(), RecoveryError> {
        if self.needs_confirmation() {
            return Err(RecoveryError::Unconfirmed);
        }
        if evidence.phase() != TransactionPhase::Committed {
            return Err(CheckpointError::InvalidPhase.into());
        }
        if let Some(existing) = self.records().find(|r| r.evidence.id() == evidence.id()) {
            return if existing.evidence.raw_payload() == evidence.raw_payload() {
                Ok(())
            } else {
                Err(RecoveryError::DuplicateRecord)
            };
        }
        let mut next = self.clone();
        next.latest_completed = Some(RecoveryRecord {
            evidence,
            disposition: Disposition::FullAfter {},
        });
        next.validate_size()?;
        *self = next;
        Ok(())
    }
    pub(crate) fn needs_confirmation(&self) -> bool {
        self.records()
            .any(|r| matches!(r.disposition, Disposition::NativeUnconfirmed {}))
    }
    pub(crate) fn seal(&self, vault: &VaultContext) -> Result<String, RecoveryError> {
        seal_payload(&self.payload(), RECOVERY_FILE, vault).map_err(Into::into)
    }
    pub(crate) fn open(encoded: &str, vault: &VaultContext) -> Result<Self, RecoveryError> {
        let payload: LedgerPayload = open_payload(encoded, RECOVERY_FILE, vault)?;
        if payload.version != 1 {
            return Err(CheckpointError::InvalidPayload.into());
        }
        if payload.archived.len() > MAX_ARCHIVED {
            return Err(RecoveryError::ArchiveFull);
        }
        let record = |p: RecordPayload| -> Result<RecoveryRecord, RecoveryError> {
            if let Disposition::ExplicitCapture { profile_id, .. } = &p.disposition {
                if profile_id.len() != 64
                    || !profile_id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(CheckpointError::InvalidPayload.into());
                }
            }
            Ok(RecoveryRecord {
                evidence: JournalEvidence::from_authenticated_payload(p.journal.as_bytes())?,
                disposition: p.disposition,
            })
        };
        let ledger = Self {
            latest_completed: payload.latest_completed.map(record).transpose()?,
            archived: payload
                .archived
                .into_iter()
                .map(record)
                .collect::<Result<_, _>>()?,
        };
        let mut ids = std::collections::BTreeSet::new();
        if ledger.records().any(|r| !ids.insert(r.evidence.id())) {
            return Err(RecoveryError::DuplicateRecord);
        }
        if ledger
            .latest_completed
            .as_ref()
            .is_some_and(|r| r.evidence.phase() != TransactionPhase::Committed)
        {
            return Err(CheckpointError::InvalidPhase.into());
        }
        Ok(ledger)
    }
    pub(crate) fn archived(&self) -> impl Iterator<Item = &RecoveryRecord> {
        self.archived.iter()
    }
    pub(crate) fn latest_completed(&self) -> Option<&RecoveryRecord> {
        self.latest_completed.as_ref()
    }
    fn records(&self) -> impl Iterator<Item = &RecoveryRecord> {
        self.latest_completed.iter().chain(self.archived.iter())
    }
    fn records_mut(&mut self) -> impl Iterator<Item = &mut RecoveryRecord> {
        self.latest_completed
            .iter_mut()
            .chain(self.archived.iter_mut())
    }
    fn payload(&self) -> LedgerPayload {
        let record = |r: &RecoveryRecord| RecordPayload {
            journal: r.evidence.raw.to_string(),
            disposition: r.disposition.clone(),
        };
        LedgerPayload {
            version: 1,
            latest_completed: self.latest_completed.as_ref().map(record),
            archived: self.archived.iter().map(record).collect(),
        }
    }
    fn with_largest_confirmations(&self) -> Self {
        let mut largest = self.clone();
        for record in largest.records_mut() {
            if matches!(record.disposition, Disposition::NativeUnconfirmed {}) {
                record.disposition = Disposition::ExplicitCapture {
                    profile_id: "f".repeat(64),
                    snapshot_revision: [255; 32],
                    profile_revision: [255; 32],
                    native_revision: [255; 32],
                };
            }
        }
        largest
    }
    fn validate_size(&self) -> Result<(), RecoveryError> {
        encode_payload(&self.payload())
            .map(|_| ())
            .map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
