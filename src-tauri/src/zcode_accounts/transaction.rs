//! Fixed-file account transaction IO. Runtime admission remains the caller's duty.
//! The caller holds sync_mutex then the existing SecretSession read guard through
//! this operation. No home, environment, key store, process shutdown or UI lookup.
//! Process-crash recovery is supported; this does not promise cross-file power-loss
//! atomicity or defend against a malicious process already running as this user.
use super::admission::BlockedReason;
use super::checkpoint::{
    CheckpointError, ProfileCatalog, RecoveryOutcome, SwitchCheckpoint, TransactionBinding,
};
use super::core::{
    AccountIdentity, AccountSnapshot, CredentialDocument, JournalOrigin, OAuthFamily,
    RecoveryAction, TransactionPhase,
};
use super::native::NativeCipher;
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
    Checkpoint(CheckpointError),
}
impl From<CheckpointError> for TransactionError {
    fn from(value: CheckpointError) -> Self {
        Self::Checkpoint(value)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchOutcome {
    Switched,
    Refreshed,
    Recovered,
    NothingPending,
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
        Ok(CatalogStatus {
            revision: catalog_revision(bytes.as_deref()),
            profiles,
            current: None,
            pending: self.read(Role::Journal)?.is_some(),
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
        self.valid_recovery_record()?;
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
        self.publish_profile(&current_bytes, bytes.as_ref(), encoded.as_bytes())?;
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
    pub(crate) fn recover(&self, origin: JournalOrigin) -> Result<SwitchOutcome, TransactionError> {
        let pending = self.read(Role::Journal)?;
        let Some(encoded) = pending else {
            return Ok(SwitchOutcome::NothingPending);
        };
        if origin != JournalOrigin::Live {
            return Err(TransactionError::Imported);
        }
        let _lock = self.lock()?;
        self.expect(Role::Journal, Some(&encoded))?;
        let checkpoint = self.open_checkpoint(&encoded)?;
        self.check_binding(&checkpoint)?;
        let recovery = self.valid_recovery_record()?;
        let current_bytes = self.required(Role::Credentials)?;
        let current = document(&current_bytes)?;
        let outcome = checkpoint.recover(&current, origin)?;
        match outcome {
            RecoveryOutcome::Restore(ref restored) => {
                // Check conflicts before touching either file. A newer saved source is
                // not silently replaced by this older pending transaction.
                self.preserve_fresh_source(&checkpoint, &current_bytes)?;
                self.publish(
                    Role::Credentials,
                    Some(&current_bytes),
                    &restored.to_bytes().map_err(|_| TransactionError::Storage)?,
                )?;
            }
            RecoveryOutcome::CleanupOnly => {}
            RecoveryOutcome::ReconcileCommit | RecoveryOutcome::Quarantine => {
                return Err(TransactionError::RecoveryRequired)
            }
        }
        self.finish(&checkpoint, &encoded, recovery.as_ref(), &mut |_| Ok(()))
            .map_err(|error| {
                if matches!(outcome, RecoveryOutcome::CleanupOnly) {
                    TransactionError::CommittedNeedsCleanup
                } else {
                    error
                }
            })?;
        Ok(SwitchOutcome::Recovered)
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
            self.publish_profile(&current_bytes, catalog_bytes.as_ref(), next.as_bytes())?;
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
        let mut journal = self.publish(Role::Journal, None, prepared.as_bytes())?;
        hook(WritePoint::Prepared)?;
        catalog.upsert(checkpoint.fresh_source().clone());
        self.publish_profile(
            &current_bytes,
            catalog_bytes.as_ref(),
            catalog.seal(self.vault, self.native)?.as_bytes(),
        )?;
        hook(WritePoint::OutgoingProfile)?;
        checkpoint.set_phase(TransactionPhase::Captured)?;
        let captured = checkpoint.seal(self.vault, self.native)?;
        journal = self.publish(Role::Journal, Some(&journal), captured.as_bytes())?;
        hook(WritePoint::Captured)?;
        let applied = checkpoint
            .apply(&current)?
            .to_bytes()
            .map_err(|_| TransactionError::Storage)?;
        self.publish(Role::Credentials, Some(&current_bytes), &applied)?;
        hook(WritePoint::NativeCredentials)?;
        checkpoint.set_phase(TransactionPhase::Committed)?;
        let committed = checkpoint.seal(self.vault, self.native)?;
        let published = self
            .publish(Role::Journal, Some(&journal), committed.as_bytes())
            .and_then(|image| {
                hook(WritePoint::Committed)?;
                Ok(image)
            });
        if published.is_err() {
            // A rename may have succeeded before sync or notification failed. Read
            // the authenticated marker; never downgrade it or auto-rollback here.
            if let Ok(Some(actual)) = self.read(Role::Journal) {
                if let Ok(actual) = self.open_checkpoint(&actual) {
                    if actual.binding() == checkpoint.binding() {
                        // Marker authentication is local-vault work. Do not read native
                        // credentials here: a failed gate may mean its writer restarted.
                        if actual.recovery_policy(JournalOrigin::Live)
                            == RecoveryAction::CleanupOnly
                        {
                            return Err(TransactionError::CommittedNeedsCleanup);
                        }
                    }
                }
            }
            return Err(TransactionError::RecoveryRequired);
        }
        let committed = published?;
        // Native publication and its COMMITTED marker are confirmed. Cleanup or
        // acknowledgment failure cannot be reported as an uncommitted admission error.
        self.finish(&checkpoint, &committed, recovery.as_ref(), hook)
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
    fn preserve_fresh_source(
        &self,
        checkpoint: &SwitchCheckpoint,
        current: &FileImage,
    ) -> Result<(), TransactionError> {
        let before = self.read(Role::Profiles)?;
        let mut catalog = self.open_catalog(before.as_deref())?;
        let fresh = checkpoint.fresh_source();
        let saved = catalog
            .get(fresh.identity())
            .ok_or(TransactionError::SourceChanged)?;
        let saved_hash = snapshot_hash(saved)?;
        if saved_hash
            != checkpoint
                .binding()
                .ok_or(TransactionError::Imported)?
                .source_profile_revision
            && saved_hash != snapshot_hash(fresh)?
        {
            return Err(TransactionError::SourceChanged);
        }
        if saved_hash != snapshot_hash(fresh)? {
            catalog.upsert(fresh.clone());
            self.publish_profile(
                current,
                before.as_ref(),
                catalog.seal(self.vault, self.native)?.as_bytes(),
            )?;
        }
        Ok(())
    }
    fn finish(
        &self,
        checkpoint: &SwitchCheckpoint,
        journal: &FileImage,
        recovery: Option<&FileImage>,
        hook: &mut dyn FnMut(WritePoint) -> Result<(), TransactionError>,
    ) -> Result<(), TransactionError> {
        self.publish(
            Role::Recovery,
            recovery,
            checkpoint
                .seal_recovery(self.vault, self.native)?
                .as_bytes(),
        )?;
        hook(WritePoint::RecoveryRecord)?;
        (self.gate)().map_err(TransactionError::Admission)?;
        self.expect(Role::Journal, Some(journal))?;
        fs::remove_file(self.vault_root.join(JOURNAL_FILE))
            .map_err(|_| TransactionError::Storage)?;
        File::open(self.vault_root)
            .and_then(|file| file.sync_all())
            .map_err(|_| TransactionError::Storage)?;
        hook(WritePoint::JournalRemoved)
    }
    fn valid_recovery_record(&self) -> Result<Option<FileImage>, TransactionError> {
        let bytes = self.read(Role::Recovery)?;
        if let Some(bytes) = &bytes {
            SwitchCheckpoint::open_recovery(text(bytes)?, self.vault, self.native)?;
        }
        Ok(bytes)
    }
    fn open_checkpoint(&self, bytes: &[u8]) -> Result<SwitchCheckpoint, TransactionError> {
        Ok(SwitchCheckpoint::open(
            text(bytes)?,
            self.vault,
            self.native,
        )?)
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
        match (self.read(role)?, before) {
            (None, None) => Ok(()),
            (Some(current), Some(before))
                if current.stamp == before.stamp && current.bytes == before.bytes =>
            {
                Ok(())
            }
            _ => Err(TransactionError::SourceChanged),
        }
    }
    fn publish(
        &self,
        role: Role,
        before: Option<&FileImage>,
        bytes: &[u8],
    ) -> Result<FileImage, TransactionError> {
        self.publish_checked(role, before, bytes, None)
    }
    fn publish_profile(
        &self,
        native_source: &FileImage,
        before: Option<&FileImage>,
        bytes: &[u8],
    ) -> Result<FileImage, TransactionError> {
        self.publish_checked(Role::Profiles, before, bytes, Some(native_source))
    }
    fn publish_checked(
        &self,
        role: Role,
        before: Option<&FileImage>,
        bytes: &[u8],
        native_source: Option<&FileImage>,
    ) -> Result<FileImage, TransactionError> {
        (self.gate)().map_err(TransactionError::Admission)?;
        // The final probe can take time. Revalidate a captured native source only
        // after it returns, alongside the catalog CAS immediately before writing.
        if let Some(source) = native_source {
            self.expect(Role::Credentials, Some(source))?;
        }
        self.expect(role, before)?;
        write_durable(&self.root(role).join(role.name()), bytes)
            .map_err(|_| TransactionError::Storage)?;
        // Our own atomic replacement establishes a new file identity. Retain it
        // for the next write/cleanup, rather than binding recovery to old inodes.
        let current = self.required(role)?;
        if current.bytes != bytes {
            return Err(TransactionError::SourceChanged);
        }
        Ok(current)
    }
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
            assert!(store.recover(JournalOrigin::Live).is_err());
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
