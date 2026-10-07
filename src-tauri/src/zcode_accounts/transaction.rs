//! Fixed-file account transaction IO. Runtime admission remains the caller's duty.
//! The caller holds sync_mutex then the existing SecretSession read guard through
//! this operation. No home, environment, key store, process shutdown or UI lookup.
//! Process-crash recovery is supported; this does not promise cross-file power-loss
//! atomicity or defend against a malicious process already running as this user.
use super::admission::BlockedReason;
use super::checkpoint::{CheckpointError, ProfileCatalog, SwitchCheckpoint, TransactionBinding};
#[cfg(test)]
use super::core::AccountIdentity;
use super::core::{AccountSnapshot, CredentialDocument, OAuthFamily, TransactionPhase};
use super::key_intent::{FreshIntent, KeyIntent, KeyIntentLedger, KeyScope, Reservation};
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
    UnverifiedSource,
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
    OperationAlreadyKnown,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CaptureCommitOutcome {
    Saved,
    Refreshed,
    Kept,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapturePreview {
    pub id: String,
    pub label: Option<String>,
    pub family: &'static str,
    pub duplicate: bool,
    /// A content concurrency check, never an authorization token. Commit reads
    /// and inspects the admitted native session again under the physical owner.
    pub native_revision: String,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedAccount {
    pub id: String,
    pub family: &'static str,
    pub label: Option<String>,
    pub source_verified: bool,
    pub identity_source: super::checkpoint::IdentitySource,
    pub official_label: Option<String>,
    pub capabilities: Option<super::session_checks::SessionCheckDisplay>,
    pub can_activate: bool,
    pub activation_blocked_reason: Option<AccountActionError>,
    pub can_check_connections: bool,
    pub check_connections_blocked_reason: Option<AccountActionError>,
    pub needs_key: bool,
    pub can_complete_coding: bool,
    pub complete_coding_blocked_reason: Option<AccountActionError>,
}

pub(crate) struct LoginProfileSave<'a> {
    pub revision: &'a str,
    pub request_id: &'a str,
    pub snapshot: AccountSnapshot,
    pub evidence: Option<super::session_checks::SessionCheckReport>,
    pub update_duplicate: bool,
    pub completion: Option<&'a super::oauth::SavedCodingTarget>,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountActionError {
    pub code: &'static str,
    pub remedy: &'static str,
    pub committed: bool,
}
impl AccountActionError {
    fn new(code: &'static str, remedy: &'static str) -> Self {
        Self {
            code,
            remedy,
            committed: false,
        }
    }
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogActions {
    pub can_add: bool,
    pub can_import: bool,
    pub can_backup: bool,
    pub can_edit_labels: bool,
    pub blocked_reason: Option<AccountActionError>,
}
impl CatalogActions {
    fn new(recovery: &RecoveryStatus, has_profiles: bool) -> Self {
        let can_change = !recovery.pending && !recovery.native_unconfirmed;
        Self {
            can_add: can_change,
            can_import: can_change,
            can_backup: has_profiles,
            can_edit_labels: can_change,
            blocked_reason: (!can_change).then(|| {
                AccountActionError::new("zcode.account.recovery_required", "reviewRecovery")
            }),
        }
    }
}
pub(crate) struct IncomingProfile {
    pub snapshot: AccountSnapshot,
    pub update_duplicate: bool,
    pub evidence: Option<super::session_checks::SessionCheckReport>,
    pub origin: super::checkpoint::IdentitySource,
}
fn saved_account(
    native: &NativeCipher,
    catalog: &ProfileCatalog,
    snapshot: &AccountSnapshot,
) -> Result<SavedAccount, TransactionError> {
    let details = catalog.details(snapshot, native);
    let official_label = native
        .profile_label(snapshot)
        .map_err(|error| TransactionError::Checkpoint(CheckpointError::Native(error)))?;
    let needs_key = super::oauth_account::saved_coding_key(native, snapshot).is_none();
    let business_available = super::oauth_account::saved_business_token(native, snapshot).is_ok();
    Ok(SavedAccount {
        id: snapshot.identity().opaque_id(),
        family: match snapshot.identity().family() {
            OAuthFamily::Zai => "zai",
            OAuthFamily::BigModel => "bigmodel",
        },
        label: details.label.or_else(|| official_label.clone()),
        official_label,
        source_verified: catalog.source_verified(snapshot.identity()),
        identity_source: details.identity_source,
        capabilities: details.evidence.map(|report| report.display()),
        can_activate: false,
        activation_blocked_reason: Some(AccountActionError::new(
            "zcode.account.native_context_required",
            "chooseContext",
        )),
        can_check_connections: true,
        check_connections_blocked_reason: None,
        needs_key,
        can_complete_coding: needs_key && business_available,
        complete_coding_blocked_reason: (needs_key && !business_available)
            .then(|| AccountActionError::new("zcode.account.official_unavailable", "addAccount")),
    })
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
    pub actions: CatalogActions,
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
    #[cfg(test)]
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
    Operations,
    KeyIntents,
    #[cfg(any(feature = "gui", test))]
    BundleExports,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Credentials => "credentials.json",
            Self::Profiles => PROFILE_FILE,
            Self::Journal => JOURNAL_FILE,
            Self::Recovery => RECOVERY_FILE,
            Self::Operations => crate::secrets::owned_file::OPERATION_FILE,
            Self::KeyIntents => crate::secrets::owned_file::KEY_INTENT_FILE,
            #[cfg(any(feature = "gui", test))]
            Self::BundleExports => crate::secrets::owned_file::BUNDLE_EXPORT_FILE,
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
struct CaptureCandidate {
    catalog_bytes: Option<FileImage>,
    catalog: ProfileCatalog,
    native_bytes: FileImage,
    snapshot: AccountSnapshot,
    recovery: Option<FileImage>,
}

/// Vault-only IO: no native path, process control or credential publication.
pub(crate) struct VaultAccountStore<'a> {
    root: &'a Path,
    root_id: [u64; 2],
    vault: &'a VaultContext,
}
impl<'a> VaultAccountStore<'a> {
    pub(crate) fn reserve_key_intent(
        &self,
        scope: KeyScope,
    ) -> Result<Reservation, TransactionError> {
        let (before, mut ledger) = self.read_key_intents()?;
        let reservation = ledger
            .reserve(scope)
            .map_err(|_| TransactionError::Storage)?;
        if matches!(reservation, Reservation::Fresh(_)) {
            // Return the fresh grant only after durable publication and read-back.
            self.publish_key_intents(before.as_ref(), &ledger)?;
        } else {
            self.expect(Role::KeyIntents, before.as_ref())?;
        }
        Ok(reservation)
    }
    pub(crate) fn key_intent(
        &self,
        scope: &KeyScope,
    ) -> Result<Option<KeyIntent>, TransactionError> {
        let (_, ledger) = self.read_key_intents()?;
        Ok(ledger.get(scope).cloned())
    }
    pub(crate) fn mark_key_created(&self, grant: &FreshIntent) -> Result<(), TransactionError> {
        let (before, mut ledger) = self.read_key_intents()?;
        ledger
            .mark_created(grant)
            .map_err(|_| TransactionError::Storage)?;
        self.publish_key_intents(before.as_ref(), &ledger)
    }
    pub(crate) fn clear_unsubmitted_key(
        &self,
        grant: &FreshIntent,
    ) -> Result<(), TransactionError> {
        let (before, mut ledger) = self.read_key_intents()?;
        if !ledger.contains_request(&grant.receipt()) {
            // A previous cleanup can have committed despite a failed read-back.
            // Authenticate absence of this request, not absence of all projects.
            self.expect(Role::KeyIntents, before.as_ref())?;
            return Ok(());
        }
        ledger
            .clear_unsubmitted(grant)
            .map_err(|_| TransactionError::Storage)?;
        self.publish_key_intents(before.as_ref(), &ledger)
    }
    pub(crate) fn clear_resolved_key(&self, record: &KeyIntent) -> Result<(), TransactionError> {
        let (before, mut ledger) = self.read_key_intents()?;
        if !ledger.contains_request(record) {
            self.expect(Role::KeyIntents, before.as_ref())?;
            return Ok(());
        }
        ledger
            .clear_resolved(record)
            .map_err(|_| TransactionError::Storage)?;
        self.publish_key_intents(before.as_ref(), &ledger)
    }
    fn read_key_intents(&self) -> Result<(Option<FileImage>, KeyIntentLedger), TransactionError> {
        let before = self.read(Role::KeyIntents)?;
        let ledger = match before.as_ref() {
            Some(bytes) => {
                KeyIntentLedger::open(bytes, self.vault).map_err(|_| TransactionError::Storage)?
            }
            None => KeyIntentLedger::default(),
        };
        Ok((before, ledger))
    }
    fn publish_key_intents(
        &self,
        before: Option<&FileImage>,
        ledger: &KeyIntentLedger,
    ) -> Result<(), TransactionError> {
        if ledger.is_empty() {
            if let Some(before) = before {
                self.remove(Role::KeyIntents, before)?;
            }
        } else {
            let encoded = ledger
                .seal(self.vault)
                .map_err(|_| TransactionError::Storage)?;
            self.publish(Role::KeyIntents, before, &encoded)?;
        }
        Ok(())
    }
    pub(crate) fn operation(
        &self,
        id: &str,
    ) -> Result<Option<super::operation_log::Record>, TransactionError> {
        match self.read(Role::Operations)? {
            None => Ok(None),
            Some(bytes) => Ok(super::operation_log::Log::open(&bytes, self.vault)
                .map_err(|_| TransactionError::Storage)?
                .get(id)
                .cloned()),
        }
    }
    pub(crate) fn operation_known(&self, id: &str) -> Result<bool, TransactionError> {
        match self.read(Role::Operations)? {
            None => Ok(false),
            Some(bytes) => Ok(super::operation_log::Log::open(&bytes, self.vault)
                .map_err(|_| TransactionError::Storage)?
                .known(id)),
        }
    }
    pub(crate) fn record_operation(
        &self,
        record: super::operation_log::Record,
    ) -> Result<(), TransactionError> {
        let before = self.read(Role::Operations)?;
        let mut log = match before.as_ref() {
            None => super::operation_log::Log::default(),
            Some(bytes) => super::operation_log::Log::open(bytes, self.vault)
                .map_err(|_| TransactionError::Storage)?,
        };
        log.put(record).map_err(|_| TransactionError::Storage)?;
        self.publish(
            Role::Operations,
            before.as_ref(),
            &log.seal(self.vault)
                .map_err(|_| TransactionError::Storage)?,
        )?;
        Ok(())
    }
    pub(crate) fn recover_operation(
        &self,
        id: &str,
    ) -> Result<Option<super::operation_log::Record>, TransactionError> {
        use super::operation_log::Phase;
        let Some(mut record) = self.operation(id)? else {
            return Ok(None);
        };
        if matches!(
            record.phase,
            Phase::Accepted | Phase::Exited | Phase::TransactionUncertain
        ) {
            let journal = self.read(Role::Journal)?;
            let recovery = self.read(Role::Recovery)?;
            let pending = journal
                .as_ref()
                .map(|bytes| {
                    JournalEvidence::open_journal(text(bytes)?, self.vault)
                        .map_err(TransactionError::from)
                })
                .transpose()?;
            let ledger = self.open_ledger(recovery.as_deref())?;
            let matches = |evidence: &JournalEvidence| {
                evidence.context() == record.context_id
                    && evidence.binding().is_some_and(|binding| {
                        binding.operation == id && binding.native_root == record.native_root
                    })
            };
            if pending.as_ref().is_some_and(|evidence| {
                matches(evidence) && evidence.phase() >= TransactionPhase::Committed
            }) || ledger
                .latest_completed()
                .into_iter()
                .chain(ledger.archived())
                .any(|r| {
                    matches(r.evidence()) && r.evidence().phase() >= TransactionPhase::Committed
                })
            {
                record.phase = Phase::Committed;
            } else if pending.as_ref().is_some_and(matches) {
                record.phase = Phase::TransactionUncertain;
            } else if record.phase != Phase::TransactionUncertain {
                record.phase = Phase::Failed;
            }
            self.record_operation(record.clone())?;
        }
        Ok(Some(record))
    }
    pub(crate) fn new(root: &'a Path, vault: &'a VaultContext) -> Result<Self, TransactionError> {
        Ok(Self {
            root,
            root_id: root_identity(root)?,
            vault,
        })
    }
    pub(crate) fn import_catalog(
        &self,
        native: &NativeCipher,
        revision: &str,
    ) -> Result<ProfileCatalog, TransactionError> {
        let bytes = self.read(Role::Profiles)?;
        if catalog_revision(bytes.as_deref()) != revision {
            return Err(TransactionError::CatalogChanged);
        }
        match bytes {
            None => Ok(ProfileCatalog::default()),
            Some(bytes) => Ok(ProfileCatalog::open(text(&bytes)?, self.vault, native)?),
        }
    }
    pub(crate) fn catalog_status(
        &self,
        native: &NativeCipher,
    ) -> Result<CatalogStatus, TransactionError> {
        let bytes = self.read(Role::Profiles)?;
        let revision = catalog_revision(bytes.as_deref());
        let catalog = self.import_catalog(native, &revision)?;
        let mut profiles = catalog
            .profiles()
            .map(|snapshot| saved_account(native, &catalog, snapshot))
            .collect::<Result<Vec<_>, TransactionError>>()?;
        let recovery = self.status()?;
        if recovery.pending || recovery.native_unconfirmed {
            for profile in &mut profiles {
                profile.can_complete_coding = false;
                profile.complete_coding_blocked_reason = Some(AccountActionError::new(
                    "zcode.account.recovery_required",
                    "reviewRecovery",
                ));
            }
        }
        Ok(CatalogStatus {
            revision,
            actions: CatalogActions::new(&recovery, !profiles.is_empty()),
            profiles,
            current: None,
            pending: recovery.pending,
            native_unconfirmed: recovery.native_unconfirmed,
        })
    }
    /// Caller has admitted this exact native provider and version. This grants
    /// only UI eligibility; the physical switch still performs its own preflight.
    pub(crate) fn catalog_status_for_connection(
        &self,
        native: &NativeCipher,
        family: OAuthFamily,
        kind: super::checkpoint::ConnectionKind,
        version: &str,
    ) -> Result<CatalogStatus, TransactionError> {
        let mut status = self.catalog_status(native)?;
        let catalog = self.import_catalog(native, &status.revision)?;
        for row in &mut status.profiles {
            let snapshot = catalog
                .profiles()
                .find(|item| item.identity().opaque_id() == row.id)
                .ok_or(CheckpointError::InvalidPayload)?;
            let reason = if status.pending || status.native_unconfirmed {
                Some(AccountActionError::new(
                    "zcode.account.recovery_required",
                    "reviewRecovery",
                ))
            } else if snapshot.identity().family() != family {
                Some(AccountActionError::new(
                    "zcode.account.unsupported_scope",
                    "chooseContext",
                ))
            } else if !catalog.can_activate(snapshot, native, Some((kind, version))) {
                Some(AccountActionError::new(
                    "zcode.account.connection_not_ready",
                    "checkConnection",
                ))
            } else {
                None
            };
            row.can_activate = reason.is_none();
            row.activation_blocked_reason = reason;
        }
        Ok(status)
    }
    pub(crate) fn set_profile_label(
        &self,
        native: &NativeCipher,
        revision: &str,
        id: &str,
        label: Option<String>,
    ) -> Result<(), TransactionError> {
        let before = self.read(Role::Profiles)?;
        let mut catalog = self.import_catalog(native, revision)?;
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if journal.is_some() || self.open_ledger(recovery.as_deref())?.needs_confirmation() {
            return Err(TransactionError::RecoveryRequired);
        }
        let identity = catalog
            .profiles()
            .find(|item| item.identity().opaque_id() == id)
            .ok_or(CheckpointError::InvalidPayload)?
            .identity()
            .clone();
        catalog.set_label(&identity, native, label)?;
        let encoded = catalog.seal(self.vault, native)?;
        self.expect(Role::Journal, journal.as_ref())?;
        self.expect(Role::Recovery, recovery.as_ref())?;
        self.publish(Role::Profiles, before.as_ref(), encoded.as_bytes())?;
        Ok(())
    }
    pub(crate) fn update_profile_evidence(
        &self,
        native: &NativeCipher,
        revision: &str,
        id: &str,
        evidence: super::session_checks::SessionCheckReport,
    ) -> Result<CatalogStatus, TransactionError> {
        let before = self.read(Role::Profiles)?;
        let mut catalog = self.import_catalog(native, revision)?;
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if journal.is_some() || self.open_ledger(recovery.as_deref())?.needs_confirmation() {
            return Err(TransactionError::RecoveryRequired);
        }
        let identity = catalog
            .profiles()
            .find(|snapshot| snapshot.identity().opaque_id() == id)
            .ok_or(CheckpointError::InvalidPayload)?
            .identity()
            .clone();
        if evidence.display().selected_profile_id != id {
            return Err(TransactionError::SourceChanged);
        }
        catalog.set_evidence(&identity, native, evidence)?;
        let encoded = catalog.seal(self.vault, native)?;
        self.expect(Role::Journal, journal.as_ref())?;
        self.expect(Role::Recovery, recovery.as_ref())?;
        self.publish(Role::Profiles, before.as_ref(), encoded.as_bytes())?;
        self.catalog_status(native)
    }
    pub(crate) fn login_receipt(
        &self,
        native: &NativeCipher,
        request_id: &str,
    ) -> Result<Option<super::checkpoint::LoginReceipt>, TransactionError> {
        let bytes = self.read(Role::Profiles)?;
        let catalog = self.import_catalog(native, &catalog_revision(bytes.as_deref()))?;
        Ok(catalog.login_receipt(request_id).cloned())
    }
    pub(crate) fn save_login_profile(
        &self,
        native: &NativeCipher,
        revision: &str,
        request_id: &str,
        snapshot: AccountSnapshot,
        evidence: Option<super::session_checks::SessionCheckReport>,
        update_duplicate: bool,
    ) -> Result<CaptureCommitOutcome, TransactionError> {
        self.save_login_profile_for(
            native,
            LoginProfileSave {
                revision,
                request_id,
                snapshot,
                evidence,
                update_duplicate,
                completion: None,
            },
        )
    }
    pub(crate) fn save_login_profile_for(
        &self,
        native: &NativeCipher,
        request: LoginProfileSave<'_>,
    ) -> Result<CaptureCommitOutcome, TransactionError> {
        let LoginProfileSave {
            revision,
            request_id,
            snapshot,
            evidence,
            update_duplicate,
            completion,
        } = request;
        if let Some(target) = completion {
            if revision != target.revision {
                return Err(TransactionError::CatalogChanged);
            }
            if !update_duplicate
                || !super::oauth_account::coding_only_change(&target.snapshot, &snapshot)
            {
                return Err(TransactionError::SourceChanged);
            }
        }
        let before = self.read(Role::Profiles)?;
        let current_revision = catalog_revision(before.as_deref());
        let mut catalog = self.import_catalog(native, &current_revision)?;
        let candidate_revision: String = Sha256::digest(
            snapshot
                .scoped_document()
                .to_bytes()
                .map_err(CheckpointError::Core)?,
        )
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
        let account_id = snapshot.identity().opaque_id();
        if let Some(receipt) = catalog.login_receipt(request_id) {
            if receipt.account_id != account_id || receipt.candidate_revision != candidate_revision
            {
                return Err(TransactionError::OperationAlreadyKnown);
            }
            return Ok(receipt.outcome);
        }
        if current_revision != revision {
            return Err(TransactionError::CatalogChanged);
        }
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if journal.is_some() || self.open_ledger(recovery.as_deref())?.needs_confirmation() {
            return Err(TransactionError::RecoveryRequired);
        }
        let duplicate = catalog.get(snapshot.identity()).is_some();
        let outcome = if let Some(target) = completion {
            if !duplicate {
                return Err(TransactionError::MissingTarget);
            }
            if catalog.replace_coding_profile(&target.snapshot, snapshot, native, evidence)? {
                CaptureCommitOutcome::Refreshed
            } else {
                CaptureCommitOutcome::Kept
            }
        } else if duplicate && !update_duplicate {
            CaptureCommitOutcome::Kept
        } else {
            if catalog.upsert_checked(
                snapshot,
                native,
                evidence,
                super::checkpoint::IdentitySource::OfficialLogin,
            ) {
                if duplicate {
                    CaptureCommitOutcome::Refreshed
                } else {
                    CaptureCommitOutcome::Saved
                }
            } else {
                CaptureCommitOutcome::Kept
            }
        };
        catalog.record_login(super::checkpoint::LoginReceipt {
            request_id: request_id.into(),
            account_id,
            candidate_revision,
            outcome,
        })?;
        // Receipt and candidate share one authenticated atomic catalog image.
        // An uncertain IPC reply can be resolved without replaying the save.
        let encoded = catalog.seal(self.vault, native)?;
        self.expect(Role::Journal, journal.as_ref())?;
        self.expect(Role::Recovery, recovery.as_ref())?;
        self.publish(Role::Profiles, before.as_ref(), encoded.as_bytes())?;
        Ok(outcome)
    }
    #[cfg(test)]
    pub(crate) fn import_profiles(
        &self,
        native: &NativeCipher,
        revision: &str,
        items: Vec<(AccountSnapshot, bool)>,
    ) -> Result<Vec<CaptureCommitOutcome>, TransactionError> {
        self.import_checked_profiles(
            native,
            revision,
            items
                .into_iter()
                .map(|(snapshot, update_duplicate)| IncomingProfile {
                    snapshot,
                    update_duplicate,
                    evidence: None,
                    origin: super::checkpoint::IdentitySource::PackageDeclared,
                })
                .collect(),
        )
    }
    pub(crate) fn import_checked_profiles(
        &self,
        native: &NativeCipher,
        revision: &str,
        items: Vec<IncomingProfile>,
    ) -> Result<Vec<CaptureCommitOutcome>, TransactionError> {
        if items.is_empty() || items.len() > 50 {
            return Err(CheckpointError::ResourceLimit.into());
        }
        let before = self.read(Role::Profiles)?;
        let mut catalog = self.import_catalog(native, revision)?;
        let journal = self.read(Role::Journal)?;
        let recovery = self.read(Role::Recovery)?;
        if journal.is_some() || self.open_ledger(recovery.as_deref())?.needs_confirmation() {
            return Err(TransactionError::RecoveryRequired);
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut outcomes = Vec::with_capacity(items.len());
        for IncomingProfile {
            snapshot,
            update_duplicate,
            evidence,
            origin,
        } in items
        {
            if !seen.insert(snapshot.identity().clone()) {
                return Err(CheckpointError::DuplicateIdentity.into());
            }
            let duplicate = catalog.get(snapshot.identity()).is_some();
            if duplicate && !update_duplicate {
                outcomes.push(CaptureCommitOutcome::Kept);
            } else {
                if !catalog.upsert_checked(snapshot, native, evidence, origin) {
                    outcomes.push(CaptureCommitOutcome::Kept);
                    continue;
                }
                outcomes.push(if duplicate {
                    CaptureCommitOutcome::Refreshed
                } else {
                    CaptureCommitOutcome::Saved
                });
            }
        }
        if outcomes
            .iter()
            .all(|outcome| *outcome == CaptureCommitOutcome::Kept)
        {
            self.expect(Role::Profiles, before.as_ref())?;
            self.expect(Role::Journal, journal.as_ref())?;
            self.expect(Role::Recovery, recovery.as_ref())?;
            return Ok(outcomes);
        }
        // Authenticate every candidate and enforce catalog capacity before any IO.
        let encoded = catalog.seal(self.vault, native)?;
        self.expect(Role::Journal, journal.as_ref())?;
        self.expect(Role::Recovery, recovery.as_ref())?;
        self.publish(Role::Profiles, before.as_ref(), encoded.as_bytes())?;
        Ok(outcomes)
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
    connection_kind: Option<(super::checkpoint::ConnectionKind, String)>,
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
        let native_id = native_root_identity(native_root)?;
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
            connection_kind: None,
        })
    }
    pub(super) fn for_connection(
        mut self,
        kind: super::checkpoint::ConnectionKind,
        app_version: &str,
    ) -> Self {
        self.connection_kind = Some((kind, app_version.into()));
        self
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
            .map(|snapshot| saved_account(self.native, &catalog, snapshot))
            .collect::<Result<Vec<_>, TransactionError>>()?;
        let recovery = VaultAccountStore::new(self.vault_root, self.vault)?.status()?;
        Ok(CatalogStatus {
            revision: catalog_revision(bytes.as_deref()),
            actions: CatalogActions::new(&recovery, !profiles.is_empty()),
            profiles,
            current: None,
            pending: recovery.pending,
            native_unconfirmed: recovery.native_unconfirmed,
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn capture(
        &self,
        family: OAuthFamily,
        expected_revision: &str,
    ) -> Result<CaptureOutcome, TransactionError> {
        let _lock = self.lock()?;
        let candidate = self.capture_candidate(family, expected_revision)?;
        self.publish_capture(candidate)
    }

    /// Explicitly authorized read only: no native write or profile persistence.
    pub(crate) fn preview_capture(
        &self,
        family: OAuthFamily,
        expected_revision: &str,
    ) -> Result<CapturePreview, TransactionError> {
        let _lock = self.lock()?;
        let candidate = self.capture_candidate(family, expected_revision)?;
        let label = self
            .native
            .profile_label(&candidate.snapshot)
            .map_err(|error| TransactionError::Checkpoint(CheckpointError::Native(error)))?
            .and_then(|label| label.chars().next().map(|first| format!("{first}…")));
        Ok(CapturePreview {
            id: candidate.snapshot.identity().opaque_id(),
            label,
            family: match family {
                OAuthFamily::Zai => "zai",
                OAuthFamily::BigModel => "bigmodel",
            },
            duplicate: candidate
                .catalog
                .get(candidate.snapshot.identity())
                .is_some(),
            native_revision: catalog_revision(Some(&candidate.native_bytes)),
        })
    }

    pub(crate) fn capture_reviewed(
        &self,
        family: OAuthFamily,
        expected_revision: &str,
        expected_native_revision: &str,
        expected_id: &str,
        update_duplicate: bool,
    ) -> Result<CaptureCommitOutcome, TransactionError> {
        let _lock = self.lock()?;
        let candidate = self.capture_candidate(family, expected_revision)?;
        if catalog_revision(Some(&candidate.native_bytes)) != expected_native_revision
            || candidate.snapshot.identity().opaque_id() != expected_id
        {
            return Err(TransactionError::SourceChanged);
        }
        if candidate
            .catalog
            .get(candidate.snapshot.identity())
            .is_some()
            && !update_duplicate
        {
            return Ok(CaptureCommitOutcome::Kept);
        }
        self.publish_capture(candidate)
            .map(|outcome| match outcome {
                CaptureOutcome::Saved => CaptureCommitOutcome::Saved,
                CaptureOutcome::Refreshed => CaptureCommitOutcome::Refreshed,
            })
    }

    // Every caller holds the existing native file lock and runtime owner.
    fn capture_candidate(
        &self,
        family: OAuthFamily,
        expected_revision: &str,
    ) -> Result<CaptureCandidate, TransactionError> {
        if self.read(Role::Journal)?.is_some() {
            return Err(TransactionError::RecoveryRequired);
        }
        let recovery = self.valid_recovery_record()?;
        let (bytes, catalog) = self.checked_catalog(expected_revision)?;
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
        Ok(CaptureCandidate {
            catalog_bytes: bytes,
            catalog,
            native_bytes: current_bytes,
            snapshot: fresh,
            recovery,
        })
    }

    fn publish_capture(
        &self,
        candidate: CaptureCandidate,
    ) -> Result<CaptureOutcome, TransactionError> {
        let CaptureCandidate {
            catalog_bytes,
            mut catalog,
            native_bytes,
            snapshot,
            recovery,
        } = candidate;
        let outcome = if catalog.get(snapshot.identity()).is_some() {
            CaptureOutcome::Refreshed
        } else {
            CaptureOutcome::Saved
        };
        catalog.upsert(snapshot);
        let encoded = catalog.seal(self.vault, self.native)?;
        self.publish_checked(
            Role::Profiles,
            catalog_bytes.as_ref(),
            encoded.as_bytes(),
            &[
                (Role::Credentials, Some(&native_bytes)),
                (Role::Journal, None),
                (Role::Recovery, recovery.as_ref()),
            ],
        )?;
        Ok(outcome)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn switch_saved(
        &self,
        profile_id: &str,
        expected_revision: &str,
        family: OAuthFamily,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_saved_operation(
            profile_id,
            expected_revision,
            family,
            &uuid::Uuid::new_v4().to_string(),
        )
    }
    #[cfg(all(test, unix))]
    pub(crate) fn switch_saved_operation(
        &self,
        profile_id: &str,
        expected_revision: &str,
        family: OAuthFamily,
        operation_id: &str,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_saved_operation_started(
            profile_id,
            expected_revision,
            family,
            operation_id,
            &mut || {},
        )
    }
    pub(crate) fn switch_saved_operation_started(
        &self,
        profile_id: &str,
        expected_revision: &str,
        family: OAuthFamily,
        operation_id: &str,
        started: &mut dyn FnMut(),
    ) -> Result<SwitchOutcome, TransactionError> {
        if uuid::Uuid::parse_str(operation_id).is_err() {
            return Err(CheckpointError::InvalidPayload.into());
        }
        self.switch_selected(
            SwitchTarget::Saved {
                id: profile_id,
                revision: expected_revision,
                family,
            },
            operation_id,
            started,
            &mut |_| Ok(()),
        )
    }

    #[cfg(test)]
    pub(crate) fn switch(
        &self,
        target: &AccountIdentity,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_with_hook(target, &mut |_| Ok(()))
    }
    #[cfg(test)]
    fn switch_with_hook(
        &self,
        target_id: &AccountIdentity,
        hook: &mut dyn FnMut(WritePoint) -> Result<(), TransactionError>,
    ) -> Result<SwitchOutcome, TransactionError> {
        self.switch_selected(
            SwitchTarget::Identity(target_id),
            &uuid::Uuid::new_v4().to_string(),
            &mut || {},
            hook,
        )
    }
    fn switch_selected(
        &self,
        selection: SwitchTarget<'_>,
        operation_id: &str,
        started: &mut dyn FnMut(),
        hook: &mut dyn FnMut(WritePoint) -> Result<(), TransactionError>,
    ) -> Result<SwitchOutcome, TransactionError> {
        let _lock = self.lock()?;
        if self.read(Role::Journal)?.is_some() {
            return Err(TransactionError::RecoveryRequired);
        }
        let recovery = self.valid_recovery_record()?;
        let (catalog_bytes, mut catalog, target_id) = match selection {
            #[cfg(test)]
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
        // Imported complete sessions use the capability actually selected in
        // the admitted native settings. Display identity is not authorization.
        if !catalog.can_activate(
            &target,
            self.native,
            self.connection_kind
                .as_ref()
                .map(|(kind, version)| (*kind, version.as_str())),
        ) {
            return Err(TransactionError::UnverifiedSource);
        }
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
            started();
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
            operation: operation_id.to_owned(),
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
        started();
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
        if native_root_identity(self.native_root)? != self.native_id
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
/// Explicit read action only. This does not grant native write admission, capture
/// the session, or change the catalog. The caller checks selected source metadata
/// around this stable, private-file read.
pub(super) fn read_current_profile(
    root: &Path,
    expected_root: [u64; 2],
    native: &NativeCipher,
    family: OAuthFamily,
    gate: &dyn Fn() -> Result<(), TransactionError>,
) -> Result<Option<AccountSnapshot>, TransactionError> {
    if native_root_identity(root)? != expected_root {
        return Err(TransactionError::SourceChanged);
    }
    let path = root.join("credentials.json");
    let before = read_private(&path)?;
    let snapshot = before
        .as_ref()
        .map(|image| {
            let document = CredentialDocument::parse(image).map_err(CheckpointError::Core)?;
            let snapshot = native.inspect(&document).map_err(CheckpointError::Native)?;
            if snapshot.identity().family() != family {
                return Err(TransactionError::SourceChanged);
            }
            Ok(snapshot)
        })
        .transpose()?;
    gate()?;
    if native_root_identity(root)? != expected_root {
        return Err(TransactionError::SourceChanged);
    }
    expect_image(read_private(&path)?, before.as_ref())?;
    Ok(snapshot)
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

#[cfg(any(feature = "gui", test))]
impl VaultAccountStore<'_> {
    /// The caller still holds the shared physical owner and vault session guard.
    pub(super) fn bundle_export_root(&self) -> Result<&Path, TransactionError> {
        self.validate_root()?;
        Ok(self.root)
    }

    pub(super) fn bundle_export_receipt(
        &self,
        request_id: &str,
    ) -> Result<Option<super::bundle_export::Receipt>, super::bundle_export::ExportFailure> {
        use super::bundle_export::{ExportFailure, ReceiptLedger};
        match self
            .read(Role::BundleExports)
            .map_err(|_| ExportFailure::Storage)?
        {
            None => Ok(None),
            Some(bytes) => Ok(ReceiptLedger::open(&bytes, self.vault)?
                .get(request_id)
                .cloned()),
        }
    }

    pub(super) fn record_bundle_export(
        &self,
        receipt: super::bundle_export::Receipt,
    ) -> Result<(), super::bundle_export::ExportFailure> {
        use super::bundle_export::{ExportFailure, ReceiptLedger};
        let before = self
            .read(Role::BundleExports)
            .map_err(|_| ExportFailure::Storage)?;
        let mut ledger = match before.as_ref() {
            Some(bytes) => ReceiptLedger::open(bytes, self.vault)?,
            None => ReceiptLedger::default(),
        };
        ledger.put(receipt.clone())?;
        let encoded = ledger.seal(self.vault)?;
        let published = self
            .publish(Role::BundleExports, before.as_ref(), &encoded)
            .map_err(|_| ExportFailure::Storage)?;
        let verified = ReceiptLedger::open(&published, self.vault)?;
        if verified.get(&receipt.request_id) != Some(&receipt) {
            return Err(ExportFailure::SavedDataInvalid);
        }
        Ok(())
    }
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

pub(super) fn root_identity(path: &Path) -> Result<[u64; 2], TransactionError> {
    checked_root_identity(path, 0o077)
}
/// Official ZCode creates directories with ordinary mkdir permissions; its
/// credential files are private. Other users must not be able to replace entries.
pub(super) fn native_root_identity(path: &Path) -> Result<[u64; 2], TransactionError> {
    checked_root_identity(path, 0o022)
}
#[cfg(unix)]
fn directory_metadata_is_safe(meta: &fs::Metadata, expected_uid: u32, forbidden: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.is_dir()
        && !meta.file_type().is_symlink()
        && meta.uid() == expected_uid
        && meta.mode() & forbidden == 0
}
#[cfg(unix)]
fn checked_root_identity(path: &Path, forbidden: u32) -> Result<[u64; 2], TransactionError> {
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
    if !directory_metadata_is_safe(&meta, unsafe { libc::geteuid() }, forbidden) {
        return Err(TransactionError::UnsafePath);
    }
    Ok([meta.dev(), meta.ino()])
}
#[cfg(not(unix))]
fn checked_root_identity(_path: &Path, _forbidden: u32) -> Result<[u64; 2], TransactionError> {
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
