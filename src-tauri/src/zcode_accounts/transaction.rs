//! Fixed-file account transaction IO. Runtime admission remains the caller's duty.
//! The caller holds sync_mutex then the existing SecretSession read guard through
//! this operation. No home, environment, key store, process shutdown or UI lookup.
//! Process-crash recovery is supported; this does not promise cross-file power-loss
//! atomicity or defend against a malicious process already running as this user.
use super::admission::BlockedReason;
use super::checkpoint::{CheckpointError, ProfileCatalog, SwitchCheckpoint, TransactionBinding};
use super::core::{
    AccountIdentity, AccountSnapshot, CredentialDocument, OAuthFamily, TransactionPhase,
};
use super::native::NativeCipher;
use super::recovery::{
    DispositionKind, JournalEvidence, RecoveryConfirmation, RecoveryError, RecoveryLedger,
};
use crate::config_file_io::write_durable;
use crate::secrets::{
    owned_file::{JOURNAL_FILE, PROFILE_FILE, RECOVERY_FILE},
    VaultContext,
};
use crate::zcode_file_lock::FileLock;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
#[cfg(unix)]
use std::io::Read;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy)]
pub(crate) struct Admission {
    pub contract_verified: bool,
    pub app_stopped: bool,
    pub native_gate_passed: bool,
    /// Verified from runtime account/provider selection, not inferred from an API-key card.
    pub individual_scope_verified: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionError {
    NotAdmitted,
    Admission(BlockedReason),
    CommittedNeedsCleanup,
    UnsupportedScope,
    UnsafePath,
    Storage,
    SourceChanged,
    RecoveryRequired,
    Imported,
    MissingSavedSource,
    MissingTarget,
    CatalogChanged,
    RecoveryChanged,
    NativeUnconfirmed,
    ArchiveNeedsCleanup,
    Recovery(RecoveryError),
    Checkpoint(CheckpointError),
}
impl From<CheckpointError> for TransactionError {
    fn from(value: CheckpointError) -> Self {
        Self::Checkpoint(value)
    }
}
impl From<RecoveryError> for TransactionError {
    fn from(value: RecoveryError) -> Self {
        Self::Recovery(value)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchOutcome {
    Switched,
    Refreshed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CaptureOutcome {
    Saved,
    Refreshed,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedAccount {
    pub id: String,
    pub family: &'static str,
    pub label: Option<String>,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogStatus {
    pub revision: String,
    pub profiles: Vec<SavedAccount>,
    /// Passive status never reads native credentials to claim a current identity.
    pub current: Option<String>,
    pub pending: bool,
    pub native_unconfirmed: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveOutcome {
    Archived,
    NothingPending,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchivePoint {
    RecoveryPublished,
    JournalRemoved,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryRecordStatus {
    pub id: String,
    pub disposition: &'static str,
    pub latest_completed: bool,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryStatus {
    pub revision: String,
    pub pending: bool,
    pub native_unconfirmed: bool,
    pub records: Vec<RecoveryRecordStatus>,
}
enum SwitchTarget<'a> {
    Identity(&'a AccountIdentity),
    Saved {
        id: &'a str,
        revision: &'a str,
        family: OAuthFamily,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WritePoint {
    Prepared,
    OutgoingProfile,
    Captured,
    NativeCredentials,
    Committed,
    RecoveryRecord,
    JournalRemoved,
}
#[derive(Clone, Copy)]
enum Role {
    Credentials,
    Profiles,
    Journal,
    Recovery,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Credentials => "credentials.json",
            Self::Profiles => PROFILE_FILE,
            Self::Journal => JOURNAL_FILE,
            Self::Recovery => RECOVERY_FILE,
        }
    }
}
#[cfg(unix)]
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

// A per-operation observation, never serialized into the journal. Keeping the
// descriptor open prevents inode reuse while a later CAS relies on its identity.
struct FileImage {
    bytes: Vec<u8>,
    stamp: [u64; 7],
    _file: File,
}

/// Local preservation has no native path, cipher, process probe or replay route.
pub(crate) struct VaultAccountStore<'a> {
    root: &'a Path,
    root_id: [u64; 2],
    vault: &'a VaultContext,
}
impl<'a> VaultAccountStore<'a> {
    pub(crate) fn new(root: &'a Path, vault: &'a VaultContext) -> Result<Self, TransactionError> {
        Ok(Self {
            root,
            root_id: root_identity(root)?,
            vault,
        })
    }
    pub(crate) fn status(&self) -> Result<RecoveryStatus, TransactionError> {
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        let pending = journal
            .as_ref()
            .map(|journal| {
                JournalEvidence::open_journal(text(journal)?, self.vault)
                    .map_err(TransactionError::from)
            })
            .transpose()?;
        let ledger = self.open_ledger(recovery.as_deref())?;
        let record_status =
            |record: &super::recovery::RecoveryRecord, latest_completed| RecoveryRecordStatus {
                id: record.evidence().id().to_owned(),
                disposition: match if pending
                    .as_ref()
                    .is_some_and(|journal: &JournalEvidence| journal.id() == record.evidence().id())
                {
                    DispositionKind::NativeUnconfirmed
                } else {
                    record.disposition()
                } {
                    DispositionKind::NativeUnconfirmed => "native-unconfirmed",
                    DispositionKind::FullBefore => "full-before",
                    DispositionKind::FullAfter => "full-after",
                    DispositionKind::ExplicitCapture => "explicit-capture",
                },
                latest_completed,
            };
        let records = ledger
            .latest_completed()
            .into_iter()
            .map(|record| record_status(record, true))
            .chain(ledger.archived().map(|record| record_status(record, false)))
            .collect();
        Ok(RecoveryStatus {
            revision: recovery_revision(journal.as_deref(), recovery.as_deref()),
            pending: journal.is_some(),
            native_unconfirmed: journal.is_some() || ledger.needs_confirmation(),
            records,
        })
    }
    pub(crate) fn archive_pending(
        &self,
        revision: &str,
    ) -> Result<ArchiveOutcome, TransactionError> {
        self.archive_with_hook(revision, &mut |_| Ok(()))
    }
    fn archive_with_hook(
        &self,
        revision: &str,
        hook: &mut dyn FnMut(ArchivePoint) -> Result<(), TransactionError>,
    ) -> Result<ArchiveOutcome, TransactionError> {
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if recovery_revision(journal.as_deref(), recovery.as_deref()) != revision {
            return Err(TransactionError::RecoveryChanged);
        }
        let mut ledger = self.open_ledger(recovery.as_deref())?;
        let Some(journal) = journal else {
            return Ok(ArchiveOutcome::NothingPending);
        };
        let evidence = JournalEvidence::open_journal(text(&journal)?, self.vault)?;
        let id = ledger.archive(evidence.clone())?;
        let encoded = ledger.seal(self.vault)?;
        self.expect(Role::Journal, Some(&journal))?;
        let published = self.publish(Role::Recovery, recovery.as_ref(), encoded.as_bytes())?;
        let verified = self.open_ledger(Some(&published))?;
        let retained = verified
            .latest_completed()
            .into_iter()
            .chain(verified.archived())
            .find(|record| record.evidence().id() == id)
            .ok_or(TransactionError::Storage)?;
        if retained.evidence().raw_payload() != evidence.raw_payload()
            || retained.disposition() != DispositionKind::NativeUnconfirmed
        {
            return Err(TransactionError::Storage);
        }
        hook(ArchivePoint::RecoveryPublished)?;
        // Deleting J is allowed only while its authenticated replacement is still
        // the exact file that was read back, as well as the same original J.
        self.expect(Role::Recovery, Some(&published))?;
        self.remove(Role::Journal, &journal)?;
        hook(ArchivePoint::JournalRemoved).map_err(|_| TransactionError::ArchiveNeedsCleanup)?;
        Ok(ArchiveOutcome::Archived)
    }
    pub(crate) fn delete_confirmed(
        &self,
        id: &str,
        revision: &str,
    ) -> Result<(), TransactionError> {
        self.delete_with_hook(id, revision, &mut || Ok(()))
    }
    fn delete_with_hook(
        &self,
        id: &str,
        revision: &str,
        before_publication: &mut dyn FnMut() -> Result<(), TransactionError>,
    ) -> Result<(), TransactionError> {
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if recovery_revision(journal.as_deref(), recovery.as_deref()) != revision {
            return Err(TransactionError::RecoveryChanged);
        }
        if let Some(journal) = &journal {
            let pending = JournalEvidence::open_journal(text(journal)?, self.vault)?;
            if pending.id() == id {
                return Err(RecoveryError::Unconfirmed.into());
            }
        }
        let mut ledger = self.open_ledger(recovery.as_deref())?;
        ledger.delete_confirmed(id)?;
        let encoded = if ledger.is_empty() {
            None
        } else {
            Some(ledger.seal(self.vault)?)
        };
        before_publication()?;
        // A full archive may be freed while another journal is pending. Preserve
        // that journal exactly; only the selected, already-confirmed record changes.
        self.expect(Role::Journal, journal.as_ref())?;
        if encoded.is_none() {
            self.remove(
                Role::Recovery,
                recovery.as_ref().ok_or(TransactionError::Storage)?,
            )?;
        } else {
            self.publish(
                Role::Recovery,
                recovery.as_ref(),
                encoded
                    .as_ref()
                    .ok_or(TransactionError::Storage)?
                    .as_bytes(),
            )?;
        }
        Ok(())
    }
    fn open_ledger(&self, bytes: Option<&[u8]>) -> Result<RecoveryLedger, TransactionError> {
        match bytes {
            None => Ok(RecoveryLedger::default()),
            Some(bytes) => Ok(RecoveryLedger::open(text(bytes)?, self.vault)?),
        }
    }
    fn validate_root(&self) -> Result<(), TransactionError> {
        if root_identity(self.root)? != self.root_id {
            return Err(TransactionError::UnsafePath);
        }
        Ok(())
    }
    fn read(&self, role: Role) -> Result<Option<FileImage>, TransactionError> {
        if matches!(role, Role::Credentials) {
            return Err(TransactionError::NotAdmitted);
        }
        self.validate_root()?;
        read_private(&self.root.join(role.name()))
    }
    fn expect(&self, role: Role, before: Option<&FileImage>) -> Result<(), TransactionError> {
        expect_image(self.read(role)?, before)
    }
    fn publish(
        &self,
        role: Role,
        before: Option<&FileImage>,
        bytes: &[u8],
    ) -> Result<FileImage, TransactionError> {
        self.expect(role, before)?;
        publish_private(&self.root.join(role.name()), bytes, || {
            self.read(role)?.ok_or(TransactionError::Storage)
        })
    }
    fn remove(&self, role: Role, before: &FileImage) -> Result<(), TransactionError> {
        self.expect(role, Some(before))?;
        fs::remove_file(self.root.join(role.name())).map_err(|_| TransactionError::Storage)?;
        File::open(self.root)
            .and_then(|file| file.sync_all())
            .map_err(|_| TransactionError::ArchiveNeedsCleanup)
    }
}
impl std::ops::Deref for FileImage {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

pub(crate) struct AccountStore<'a> {
    native_root: &'a Path,
    vault_root: &'a Path,
    native_id: [u64; 2],
    vault_id: [u64; 2],
    vault: &'a VaultContext,
    native: &'a NativeCipher,
    gate: &'a dyn Fn() -> Result<(), BlockedReason>,
}
impl<'a> AccountStore<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        native_root: &'a Path,
        vault_root: &'a Path,
        vault: &'a VaultContext,
        native: &'a NativeCipher,
        admission: Admission,
    ) -> Result<Self, TransactionError> {
        static SYNTHETIC_GATE: fn() -> Result<(), BlockedReason> = || Ok(());
        Self::new_guarded(
            native_root,
            vault_root,
            vault,
            native,
            admission,
            &SYNTHETIC_GATE,
        )
    }

    pub(super) fn new_guarded(
        native_root: &'a Path,
        vault_root: &'a Path,
        vault: &'a VaultContext,
        native: &'a NativeCipher,
        admission: Admission,
        gate: &'a dyn Fn() -> Result<(), BlockedReason>,
    ) -> Result<Self, TransactionError> {
        if !admission.contract_verified || !admission.app_stopped || !admission.native_gate_passed {
            return Err(TransactionError::NotAdmitted);
        }
        if !admission.individual_scope_verified {
            return Err(TransactionError::UnsupportedScope);
        }
        gate().map_err(TransactionError::Admission)?;
        let native_id = root_identity(native_root)?;
        let vault_id = root_identity(vault_root)?;
        if native_id == vault_id {
            return Err(TransactionError::UnsafePath);
        }
        Ok(Self {
            native_root,
            vault_root,
            native_id,
            vault_id,
            vault,
            native,
            gate,
        })
    }
    pub(super) fn native_root_identity(&self) -> [u64; 2] {
        self.native_id
    }

    pub(crate) fn confirm_archived(
        &self,
        id: &str,
        revision: &str,
    ) -> Result<(), TransactionError> {
        let _lock = self.lock()?;
        let (journal, recovery, mut ledger) = self.archived_candidate(revision)?;
        let evidence = find_evidence(&ledger, id)?.clone();
        self.check_binding(&evidence.native_checkpoint(self.native)?)?;
        let current_bytes = self.required(Role::Credentials)?;
        let current = document(&current_bytes)?;
        let proof = RecoveryConfirmation::from_image(&evidence, &current, self.native)?;
        ledger.confirm(id, proof)?;
        self.publish_checked(
            Role::Recovery,
            recovery.as_ref(),
            ledger.seal(self.vault)?.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Journal, journal.as_ref()),
            ],
        )?;
        Ok(())
    }

    pub(crate) fn capture_and_confirm(
        &self,
        id: &str,
        recovery_revision: &str,
        catalog_revision: &str,
        family: OAuthFamily,
    ) -> Result<CaptureOutcome, TransactionError> {
        let _lock = self.lock()?;
        let (journal, recovery, mut ledger) = self.archived_candidate(recovery_revision)?;
        let evidence = find_evidence(&ledger, id)?.clone();
        let (catalog_bytes, mut catalog) = self.checked_catalog(catalog_revision)?;
        let current_bytes = self.required(Role::Credentials)?;
        let current = document(&current_bytes)?;
        let fresh = self
            .native
            .inspect(&current)
            .map_err(|error| TransactionError::Checkpoint(CheckpointError::Native(error)))?;
        if !fresh
            .identity()
            .matches_scope(self.native.context(), family)
        {
            return Err(TransactionError::UnsupportedScope);
        }
        let outcome = if catalog.get(fresh.identity()).is_some() {
            CaptureOutcome::Refreshed
        } else {
            CaptureOutcome::Saved
        };
        catalog.upsert(fresh.clone());
        let encoded = catalog.seal(self.vault, self.native)?;
        // Validate the exact prospective resolution before saving anything, then
        // publish the profile first. An interruption cannot resolve first/save later.
        let proof = RecoveryConfirmation::after_persisted_capture(
            &evidence,
            &current,
            &fresh,
            self.native,
            hash(encoded.as_bytes()),
            hash(&current_bytes),
        )?;
        let mut preview = ledger.clone();
        preview.confirm(id, proof)?;
        preview.seal(self.vault)?;
        let published_profile = self.publish_checked(
            Role::Profiles,
            catalog_bytes.as_ref(),
            encoded.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Journal, journal.as_ref()),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        let proof = RecoveryConfirmation::after_persisted_capture(
            &evidence,
            &current,
            &fresh,
            self.native,
            hash(&published_profile),
            hash(&current_bytes),
        )?;
        ledger.confirm(id, proof)?;
        self.publish_checked(
            Role::Recovery,
            recovery.as_ref(),
            ledger.seal(self.vault)?.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Journal, journal.as_ref()),
                (Role::Profiles, Some(&published_profile)),
            ],
        )?;
        Ok(outcome)
    }
    fn archived_candidate(
        &self,
        revision: &str,
    ) -> Result<(Option<FileImage>, Option<FileImage>, RecoveryLedger), TransactionError> {
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if recovery_revision(journal.as_deref(), recovery.as_deref()) != revision {
            return Err(TransactionError::RecoveryChanged);
        }
        if let Some(journal) = &journal {
            JournalEvidence::open_journal(text(journal)?, self.vault)?;
        }
        let ledger = VaultAccountStore::new(self.vault_root, self.vault)?
            .open_ledger(recovery.as_deref())?;
        Ok((journal, recovery, ledger))
    }

    pub(crate) fn status(&self) -> Result<CatalogStatus, TransactionError> {
        (self.gate)().map_err(TransactionError::Admission)?;
        let bytes = self.read(Role::Profiles)?;
        let catalog = self.open_catalog(bytes.as_deref())?;
        let profiles = catalog
            .profiles()
            .map(|snapshot| {
                Ok(SavedAccount {
                    id: snapshot.identity().opaque_id(),
                    family: match snapshot.identity().family() {
                        OAuthFamily::Zai => "zai",
                        OAuthFamily::BigModel => "bigmodel",
                    },
                    label: self.native.profile_label(snapshot).map_err(|error| {
                        TransactionError::Checkpoint(CheckpointError::Native(error))
                    })?,
                })
            })
            .collect::<Result<Vec<_>, TransactionError>>()?;
        let recovery = VaultAccountStore::new(self.vault_root, self.vault)?.status()?;
        Ok(CatalogStatus {
            revision: catalog_revision(bytes.as_deref()),
            profiles,
            current: None,
            pending: recovery.pending,
            native_unconfirmed: recovery.native_unconfirmed,
        })
    }

    pub(crate) fn capture(
        &self,
        family: OAuthFamily,
        expected_revision: &str,
    ) -> Result<CaptureOutcome, TransactionError> {
        let _lock = self.lock()?;
        if self.read(Role::Journal)?.is_some() {
            return Err(TransactionError::RecoveryRequired);
        }
        let recovery = self.valid_recovery_record()?;
        let (bytes, mut catalog) = self.checked_catalog(expected_revision)?;
        let current_bytes = self.required(Role::Credentials)?;
        let current = document(&current_bytes)?;
        let fresh = self
            .native
            .inspect(&current)
            .map_err(|error| TransactionError::Checkpoint(CheckpointError::Native(error)))?;
        if !fresh
            .identity()
            .matches_scope(self.native.context(), family)
        {
            return Err(TransactionError::UnsupportedScope);
        }
        let outcome = if catalog.get(fresh.identity()).is_some() {
            CaptureOutcome::Refreshed
        } else {
            CaptureOutcome::Saved
        };
        catalog.upsert(fresh);
        let encoded = catalog.seal(self.vault, self.native)?;
        self.publish_checked(
            Role::Profiles,
            bytes.as_ref(),
            encoded.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Journal, None),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        Ok(outcome)
    }

    pub(crate) fn switch_saved(
        &self,
        profile_id: &str,
        expected_revision: &str,
        family: OAuthFamily,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_selected(
            SwitchTarget::Saved {
                id: profile_id,
                revision: expected_revision,
                family,
            },
            &mut |_| Ok(()),
        )
    }

    pub(crate) fn switch(
        &self,
        target: &AccountIdentity,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_with_hook(target, &mut |_| Ok(()))
    }
    fn switch_with_hook(
        &self,
        target_id: &AccountIdentity,
        hook: &mut dyn FnMut(WritePoint) -> Result<(), TransactionError>,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_selected(SwitchTarget::Identity(target_id), hook)
    }
    fn switch_selected(
        &self,
        selection: SwitchTarget<'_>,
        hook: &mut dyn FnMut(WritePoint) -> Result<(), TransactionError>,
    ) -> Result<SwitchOutcome, TransactionError> {
        let _lock = self.lock()?;
        if self.read(Role::Journal)?.is_some() {
            return Err(TransactionError::RecoveryRequired);
        }
        let recovery = self.valid_recovery_record()?;
        let (catalog_bytes, mut catalog, target_id) = match selection {
            SwitchTarget::Identity(identity) => {
                let bytes = self.read(Role::Profiles)?;
                let catalog = self.open_catalog(bytes.as_deref())?;
                (bytes, catalog, identity.clone())
            }
            SwitchTarget::Saved {
                id,
                revision,
                family,
            } => {
                let (bytes, catalog) = self.checked_catalog(revision)?;
                let identity = catalog
                    .profiles()
                    .find(|profile| profile.identity().opaque_id() == id)
                    .ok_or(TransactionError::MissingTarget)?
                    .identity()
                    .clone();
                if !identity.matches_scope(self.native.context(), family) {
                    return Err(TransactionError::UnsupportedScope);
                }
                (bytes, catalog, identity)
            }
        };
        let target = catalog
            .get(&target_id)
            .ok_or(TransactionError::MissingTarget)?
            .clone();
        let current_bytes = self.required(Role::Credentials)?;
        let current = document(&current_bytes)?;
        let fresh = self
            .native
            .inspect(&current)
            .map_err(|e| TransactionError::Checkpoint(CheckpointError::Native(e)))?;
        let saved_source = catalog
            .get(fresh.identity())
            .ok_or(TransactionError::MissingSavedSource)?;
        let source_profile_revision = snapshot_hash(saved_source)?;
        if fresh.identity() == &target_id {
            catalog.upsert(fresh);
            let next = catalog.seal(self.vault, self.native)?;
            self.publish_checked(
                Role::Profiles,
                catalog_bytes.as_ref(),
                next.as_bytes(),
                &[
                    (Role::Credentials, Some(&current_bytes)),
                    (Role::Journal, None),
                    (Role::Recovery, recovery.as_ref()),
                ],
            )?;
            return Ok(SwitchOutcome::Refreshed);
        }
        let mut checkpoint = SwitchCheckpoint::prepare(&current, &target, self.native)?;
        checkpoint.bind(TransactionBinding {
            operation: uuid::Uuid::new_v4().to_string(),
            native_root: self.native_id,
            vault_root: self.vault_id,
            source_revision: hash(&current_bytes),
            source_profile_revision,
        })?;
        let prepared = checkpoint.seal(self.vault, self.native)?;
        let ledger = VaultAccountStore::new(self.vault_root, self.vault)?
            .open_ledger(recovery.as_deref())?;
        ledger.ensure_switch_capacity(
            &JournalEvidence::open_journal(&prepared, self.vault)?,
            self.vault,
        )?;
        let mut journal = self.publish_checked(
            Role::Journal,
            None,
            prepared.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Profiles, catalog_bytes.as_ref()),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        hook(WritePoint::Prepared)?;
        catalog.upsert(checkpoint.fresh_source().clone());
        let captured_profile = self.publish_checked(
            Role::Profiles,
            catalog_bytes.as_ref(),
            catalog.seal(self.vault, self.native)?.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Journal, Some(&journal)),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        hook(WritePoint::OutgoingProfile)?;
        checkpoint.set_phase(TransactionPhase::Captured)?;
        let captured = checkpoint.seal(self.vault, self.native)?;
        journal = self.publish_checked(
            Role::Journal,
            Some(&journal),
            captured.as_bytes(),
            &[
                (Role::Credentials, Some(&current_bytes)),
                (Role::Profiles, Some(&captured_profile)),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        hook(WritePoint::Captured)?;
        let applied = checkpoint
            .apply(&current)?
            .to_bytes()
            .map_err(|_| TransactionError::Storage)?;
        let published_native = self.publish_checked(
            Role::Credentials,
            Some(&current_bytes),
            &applied,
            &[
                (Role::Journal, Some(&journal)),
                (Role::Profiles, Some(&captured_profile)),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        hook(WritePoint::NativeCredentials)?;
        checkpoint.set_phase(TransactionPhase::Committed)?;
        let committed = checkpoint.seal(self.vault, self.native)?;
        let published = self
            .publish_checked(
                Role::Journal,
                Some(&journal),
                committed.as_bytes(),
                &[
                    (Role::Credentials, Some(&published_native)),
                    (Role::Profiles, Some(&captured_profile)),
                    (Role::Recovery, recovery.as_ref()),
                ],
            )
            .and_then(|image| {
                hook(WritePoint::Committed)?;
                Ok(image)
            });
        if published.is_err() {
            // A rename may have succeeded before sync or notification failed. Read
            // the authenticated marker; never downgrade it or auto-rollback here.
            let actual = VaultAccountStore::new(self.vault_root, self.vault)
                .ok()
                .and_then(|local| local.read(Role::Journal).ok().flatten())
                .and_then(|bytes| {
                    JournalEvidence::open_journal(text(&bytes).ok()?, self.vault).ok()
                });
            if actual.is_some_and(|actual| {
                actual.binding() == checkpoint.binding()
                    && actual.phase() == TransactionPhase::Committed
            }) {
                return Err(TransactionError::CommittedNeedsCleanup);
            }
            return Err(TransactionError::RecoveryRequired);
        }
        let committed = published?;
        // Native publication and its COMMITTED marker are confirmed. Cleanup or
        // acknowledgment failure cannot be reported as an uncommitted admission error.
        self.finish(
            &committed,
            recovery.as_ref(),
            &published_native,
            &captured_profile,
            hook,
        )
        .map_err(|_| TransactionError::CommittedNeedsCleanup)?;
        Ok(SwitchOutcome::Switched)
    }
    fn checked_catalog(
        &self,
        expected_revision: &str,
    ) -> Result<(Option<FileImage>, ProfileCatalog), TransactionError> {
        let bytes = self.read(Role::Profiles)?;
        if catalog_revision(bytes.as_deref()) != expected_revision {
            return Err(TransactionError::CatalogChanged);
        }
        let catalog = self.open_catalog(bytes.as_deref())?;
        Ok((bytes, catalog))
    }
    fn finish(
        &self,
        journal: &FileImage,
        recovery: Option<&FileImage>,
        native: &FileImage,
        profile: &FileImage,
        hook: &mut dyn FnMut(WritePoint) -> Result<(), TransactionError>,
    ) -> Result<(), TransactionError> {
        let mut ledger = VaultAccountStore::new(self.vault_root, self.vault)?
            .open_ledger(recovery.map(|bytes| &**bytes))?;
        let evidence = JournalEvidence::open_journal(text(journal)?, self.vault)?;
        ledger.record_completion(evidence.clone())?;
        let published = self.publish_checked(
            Role::Recovery,
            recovery,
            ledger.seal(self.vault)?.as_bytes(),
            &[
                (Role::Journal, Some(journal)),
                (Role::Credentials, Some(native)),
                (Role::Profiles, Some(profile)),
            ],
        )?;
        let verified = RecoveryLedger::open(text(&published)?, self.vault)?;
        if find_evidence(&verified, evidence.id())?.raw_payload() != evidence.raw_payload() {
            return Err(TransactionError::Storage);
        }
        hook(WritePoint::RecoveryRecord)?;
        (self.gate)().map_err(TransactionError::Admission)?;
        self.expect(Role::Recovery, Some(&published))?;
        self.expect(Role::Journal, Some(journal))?;
        self.expect(Role::Credentials, Some(native))?;
        self.expect(Role::Profiles, Some(profile))?;
        fs::remove_file(self.vault_root.join(JOURNAL_FILE))
            .map_err(|_| TransactionError::Storage)?;
        File::open(self.vault_root)
            .and_then(|file| file.sync_all())
            .map_err(|_| TransactionError::Storage)?;
        hook(WritePoint::JournalRemoved)
    }
    fn valid_recovery_record(&self) -> Result<Option<FileImage>, TransactionError> {
        let bytes = self.read(Role::Recovery)?;
        let ledger =
            VaultAccountStore::new(self.vault_root, self.vault)?.open_ledger(bytes.as_deref())?;
        if ledger.needs_confirmation() {
            return Err(TransactionError::NativeUnconfirmed);
        }
        Ok(bytes)
    }
    fn check_binding(&self, checkpoint: &SwitchCheckpoint) -> Result<(), TransactionError> {
        let binding = checkpoint.binding().ok_or(TransactionError::Imported)?;
        if binding.native_root != self.native_id || binding.vault_root != self.vault_id {
            return Err(TransactionError::Imported);
        }
        Ok(())
    }
    fn open_catalog(&self, bytes: Option<&[u8]>) -> Result<ProfileCatalog, TransactionError> {
        match bytes {
            None => Ok(ProfileCatalog::default()),
            Some(bytes) => Ok(ProfileCatalog::open(text(bytes)?, self.vault, self.native)?),
        }
    }
    fn lock(&self) -> Result<FileLock, TransactionError> {
        (self.gate)().map_err(TransactionError::Admission)?;
        self.validate_roots()?;
        let lock = FileLock::acquire_recoverable(
            &self.native_root.join("credentials.json"),
            Duration::from_millis(500),
        )
        .map_err(|_| TransactionError::RecoveryRequired)?;
        // The app may have restarted while the operation waited for its native lock.
        (self.gate)().map_err(TransactionError::Admission)?;
        Ok(lock)
    }
    fn root(&self, role: Role) -> &Path {
        match role {
            Role::Credentials => self.native_root,
            _ => self.vault_root,
        }
    }
    fn validate_roots(&self) -> Result<(), TransactionError> {
        if root_identity(self.native_root)? != self.native_id
            || root_identity(self.vault_root)? != self.vault_id
        {
            return Err(TransactionError::UnsafePath);
        }
        Ok(())
    }
    fn read(&self, role: Role) -> Result<Option<FileImage>, TransactionError> {
        self.validate_roots()?;
        read_private(&self.root(role).join(role.name()))
    }
    fn required(&self, role: Role) -> Result<FileImage, TransactionError> {
        self.read(role)?.ok_or(TransactionError::Storage)
    }
    fn expect(&self, role: Role, before: Option<&FileImage>) -> Result<(), TransactionError> {
        expect_image(self.read(role)?, before)
    }
    fn publish_checked(
        &self,
        role: Role,
        before: Option<&FileImage>,
        bytes: &[u8],
        dependencies: &[(Role, Option<&FileImage>)],
    ) -> Result<FileImage, TransactionError> {
        (self.gate)().map_err(TransactionError::Admission)?;
        // The final probe can take time. Revalidate every fixed-file dependency
        // after it returns, alongside the destination CAS immediately before writing.
        for (role, source) in dependencies {
            self.expect(*role, *source)?;
        }
        self.expect(role, before)?;
        publish_private(&self.root(role).join(role.name()), bytes, || {
            self.required(role)
        })
    }
}
fn find_evidence<'a>(
    ledger: &'a RecoveryLedger,
    id: &str,
) -> Result<&'a JournalEvidence, TransactionError> {
    ledger
        .latest_completed()
        .into_iter()
        .chain(ledger.archived())
        .find(|record| record.evidence().id() == id)
        .map(|record| record.evidence())
        .ok_or(TransactionError::Recovery(RecoveryError::NotFound))
}
fn expect_image(
    current: Option<FileImage>,
    before: Option<&FileImage>,
) -> Result<(), TransactionError> {
    match (current, before) {
        (None, None) => Ok(()),
        (Some(current), Some(before))
            if current.stamp == before.stamp && current.bytes == before.bytes =>
        {
            Ok(())
        }
        _ => Err(TransactionError::SourceChanged),
    }
}
fn publish_private(
    path: &Path,
    bytes: &[u8],
    readback: impl FnOnce() -> Result<FileImage, TransactionError>,
) -> Result<FileImage, TransactionError> {
    write_durable(path, bytes).map_err(|_| TransactionError::Storage)?;
    let current = readback()?;
    if current.bytes != bytes {
        return Err(TransactionError::SourceChanged);
    }
    Ok(current)
}
fn recovery_revision(journal: Option<&[u8]>, recovery: Option<&[u8]>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"zcode-recovery-revision:v1\0");
    for bytes in [journal, recovery] {
        match bytes {
            None => digest.update([0]),
            Some(bytes) => {
                digest.update([1]);
                digest.update((bytes.len() as u64).to_be_bytes());
                digest.update(hash(bytes));
            }
        }
    }
    format!("sha256:{}", URL_SAFE_NO_PAD.encode(digest.finalize()))
}
fn text(bytes: &[u8]) -> Result<&str, TransactionError> {
    std::str::from_utf8(bytes).map_err(|_| TransactionError::Storage)
}
fn document(bytes: &[u8]) -> Result<CredentialDocument, TransactionError> {
    CredentialDocument::parse(bytes)
        .map_err(|e| TransactionError::Checkpoint(CheckpointError::Core(e)))
}
fn catalog_revision(bytes: Option<&[u8]>) -> String {
    match bytes {
        Some(bytes) => format!("sha256:{}", URL_SAFE_NO_PAD.encode(hash(bytes))),
        None => "absent".to_owned(),
    }
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn snapshot_hash(snapshot: &AccountSnapshot) -> Result<[u8; 32], TransactionError> {
    Ok(hash(
        &snapshot
            .scoped_document()
            .to_bytes()
            .map_err(|_| TransactionError::Storage)?,
    ))
}

#[cfg(unix)]
fn root_identity(path: &Path) -> Result<[u64; 2], TransactionError> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || fs::canonicalize(path).map_err(|_| TransactionError::UnsafePath)? != path
    {
        return Err(TransactionError::UnsafePath);
    }
    let meta = fs::symlink_metadata(path).map_err(|_| TransactionError::UnsafePath)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err(TransactionError::UnsafePath);
    }
    Ok([meta.dev(), meta.ino()])
}
#[cfg(not(unix))]
fn root_identity(_path: &Path) -> Result<[u64; 2], TransactionError> {
    Err(TransactionError::NotAdmitted)
}
#[cfg(unix)]
fn read_private(path: &Path) -> Result<Option<FileImage>, TransactionError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(TransactionError::UnsafePath),
    };
    let meta = file.metadata().map_err(|_| TransactionError::Storage)?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_FILE_BYTES
    {
        return Err(TransactionError::UnsafePath);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| TransactionError::Storage)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(TransactionError::UnsafePath);
    }
    let after = file.metadata().map_err(|_| TransactionError::Storage)?;
    if file_stamp(&meta) != file_stamp(&after) {
        return Err(TransactionError::SourceChanged);
    }
    Ok(Some(FileImage {
        bytes,
        stamp: file_stamp(&after),
        _file: file,
    }))
}
#[cfg(not(unix))]
fn read_private(_path: &Path) -> Result<Option<FileImage>, TransactionError> {
    Err(TransactionError::NotAdmitted)
}
#[cfg(all(test, unix))]
#[path = "transaction_tests.rs"]
mod tests;

#[cfg(all(test, not(unix)))]
mod unsupported_platform_tests {
    use super::*;
    #[test]
    fn native_writes_remain_disabled_without_a_platform_adapter() {
        let vault = VaultContext::generate().unwrap();
        let native = NativeCipher::new("synthetic-context", "synthetic-secret").unwrap();
        let admission = Admission {
            contract_verified: true,
            app_stopped: true,
            native_gate_passed: true,
            individual_scope_verified: true,
        };
        let result = AccountStore::new(
            Path::new("synthetic-native"),
            Path::new("synthetic-vault"),
            &vault,
            &native,
            admission,
        );
        assert!(matches!(result, Err(TransactionError::NotAdmitted)));
        // Keep the shared adapter call graph type-checked on the disabled platform.
        if let Ok(store) = result {
            let identity = AccountIdentity::new(
                "synthetic-context",
                super::super::core::OAuthFamily::Zai,
                "a",
            )
            .unwrap();
            assert!(store.switch(&identity).is_err());
            assert!(store
                .confirm_archived("synthetic-record", "synthetic-revision")
                .is_err());
        }
    }
}

#[cfg(unix)]
fn file_stamp(meta: &fs::Metadata) -> [u64; 7] {
    use std::os::unix::fs::MetadataExt;
    [
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime() as u64,
        meta.mtime_nsec() as u64,
        meta.ctime() as u64,
        meta.ctime_nsec() as u64,
    ]
}
