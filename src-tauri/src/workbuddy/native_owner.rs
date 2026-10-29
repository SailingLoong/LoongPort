//! Registered journal owner with session/selector and passive lifecycle admission.
//! No native activation command; forward lifecycle integration remains incomplete.
use super::native_transaction::Failure;
use crate::secrets::storage::{StorageContext, StorageIdentity};
use std::path::Path;
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    Absent,
    Clear,
    Pending,
}
/// Pure phase read for lifecycle callers already holding verified OLD storage/locks.
/// No session lookup, lock acquisition, directory creation or credential write.
pub(crate) fn inspect(root: &Path, old_storage: &StorageContext) -> Result<Disposition, Failure> {
    Ok(match read_state(root, old_storage)? {
        None => Disposition::Absent,
        Some(state) => {
            if state.pending() {
                Disposition::Pending
            } else {
                Disposition::Clear
            }
        }
    })
}
fn read_state(
    root: &Path,
    storage: &StorageContext,
) -> Result<Option<super::native_transaction::owner_wire::State>, Failure> {
    match storage {
        StorageContext::Plaintext(p) => p.validate(),
        StorageContext::Encrypted(v) => v.metadata().validate(),
    }
    .map_err(|_| Failure::Journal)?;
    if !crate::config_file_io::directory_exists_no_alias(root).map_err(|_| Failure::Journal)? {
        return Err(Failure::Journal);
    }
    let file = crate::secrets::owned_file::OwnedFile::registered(
        crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE,
    )
    .map_err(|_| Failure::Journal)?;
    let Some(bytes) = crate::config_file_io::read_regular_file(
        &root.join(file.relative_path()),
        112 * 1024 * 1024,
    )
    .map_err(|_| Failure::Journal)?
    else {
        return Ok(None);
    };
    let bytes = zeroize::Zeroizing::new(bytes);
    let decoded = file
        .decode_storage(storage, &bytes)
        .map_err(|_| Failure::Journal)?;
    Ok(Some(super::native_transaction::owner_wire::State::decode(
        &decoded,
    )?))
}

/// Production construction borrows the actual session read guard and selector owner.
/// Synthetic standalone construction exists only inside tests.
pub(crate) struct Owner<'a> {
    root: &'a Path,
    storage: &'a StorageContext,
    identity: StorageIdentity,
    epoch: u64,
    binding: &'a dyn Fn() -> (StorageIdentity, u64),
    selected: Option<(
        &'a crate::secrets::session::SecretSession,
        &'a crate::app_store::isolated_scope::SelectedSourceGuard,
        &'a crate::app_store::isolated_scope::SelectionSnapshot,
    )>,
}
impl<'a> Owner<'a> {
    #[cfg(test)]
    fn new(
        root: &'a Path,
        storage: &'a StorageContext,
        epoch: u64,
        binding: &'a dyn Fn() -> (StorageIdentity, u64),
    ) -> Self {
        Self {
            root,
            storage,
            identity: storage.identity(),
            epoch,
            binding,
            selected: None,
        }
    }
    fn check(&self) -> Result<(), Failure> {
        if let Some((session, selected, selection)) = self.selected {
            session
                .ensure_available()
                .map_err(|_| Failure::SourceChanged)?;
            if session.root() != self.root || selected.source_data_root() != self.root {
                return Err(Failure::SourceChanged);
            }
            selected
                .verify_unchanged()
                .map_err(|_| Failure::SourceChanged)?;
            // Lock availability is not proof that an abandoned generation is absent.
            // Reuse the original strict inspector under the same held selector owner.
            if crate::app_store::isolated_scope::inspect_pending_generation_at(selection)
                .map_err(|_| Failure::RecoveryRequired)?
                .is_some()
            {
                return Err(Failure::RecoveryRequired);
            }
        }
        // Transition owns authentication/recovery of its intent. Native writes
        // only consume its passive admission reader; unknown bytes/errors block.
        // Original passive intent readers only: no decoding, cleanup or recovery.
        // Inverse presence admission does not replace each owner's forward lock gate.
        if crate::secrets::transition::pending_record(self.root)
            .map_err(|_| Failure::RecoveryRequired)?
            .is_some()
            || crate::secrets::reset::pending_record(self.root)
                .map_err(|_| Failure::RecoveryRequired)?
                .is_some()
            || crate::secrets::bootstrap_restore::pending_record(self.root)
                .map_err(|_| Failure::RecoveryRequired)?
                .is_some()
            || crate::secrets::rewrap::pending_record(self.root)
                .map_err(|_| Failure::RecoveryRequired)?
                .is_some()
        {
            return Err(Failure::RecoveryRequired);
        }
        if (self.binding)() != (self.identity.clone(), self.epoch)
            || self.storage.identity() != self.identity
        {
            return Err(Failure::SourceChanged);
        }
        match self.storage {
            StorageContext::Plaintext(p) => p.validate(),
            StorageContext::Encrypted(v) => v.metadata().validate(),
        }
        .map_err(|_| Failure::Journal)?;
        if !crate::config_file_io::directory_exists_no_alias(self.root)
            .map_err(|_| Failure::Journal)?
        {
            return Err(Failure::Journal);
        }
        Ok(())
    }
    fn file(&self) -> Result<crate::secrets::owned_file::OwnedFile, Failure> {
        crate::secrets::owned_file::OwnedFile::registered(
            crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE,
        )
        .map_err(|_| Failure::Journal)
    }
    fn read(&self) -> Result<Option<super::native_transaction::owner_wire::State>, Failure> {
        self.check()?;
        let state = read_state(self.root, self.storage)?;
        self.check()?;
        Ok(state)
    }
    // Remove when the native capture/controller initializes its registered owner.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn initialize(&mut self) -> Result<(), Failure> {
        self.check()?;
        if self.read()?.is_some() {
            return Err(Failure::RequestConflict);
        }
        let state =
            super::native_transaction::owner_wire::State::initial(uuid::Uuid::new_v4().to_string());
        self.write(&state)
    }
    fn write(&self, state: &super::native_transaction::owner_wire::State) -> Result<(), Failure> {
        let file = self.file()?;
        let plain = state.encode()?;
        let encoded = zeroize::Zeroizing::new(
            file.encode_storage(self.storage, &plain)
                .map_err(|_| Failure::Journal)?,
        );
        self.check()?;
        crate::secrets::files::ensure_plaintext_destination(
            self.storage,
            &self.root.join(file.relative_path()),
        )
        .map_err(|_| Failure::Journal)?;
        self.check()?;
        crate::config_file_io::write_durable(&self.root.join(file.relative_path()), &encoded)
            .map_err(|_| Failure::Journal)?;
        let read = self.read()?.ok_or(Failure::Journal)?;
        if read.encode()?.as_slice() != plain.as_slice() {
            return Err(Failure::Journal);
        }
        Ok(())
    }
    // Remove when the native recovery controller reads this locked owner view.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn disposition(&self) -> Result<Disposition, Failure> {
        Ok(match self.read()? {
            None => Disposition::Absent,
            Some(state) => {
                if state.pending() {
                    Disposition::Pending
                } else {
                    Disposition::Clear
                }
            }
        })
    }
}
impl super::native_transaction::JournalOwner for Owner<'_> {
    fn load(
        &mut self,
    ) -> Result<
        (
            super::native_transaction::Stamp,
            Option<super::native_transaction::Journal>,
        ),
        Failure,
    > {
        let state = self.read()?.ok_or(Failure::Journal)?;
        Ok((state.stamp, state.journal))
    }
    fn persist(
        &mut self,
        expected: &super::native_transaction::Stamp,
        journal: &super::native_transaction::Journal,
    ) -> Result<super::native_transaction::Stamp, Failure> {
        let current = self.read()?.ok_or(Failure::Journal)?;
        let next = current.next(expected, journal)?;
        self.write(&next)?;
        Ok(next.stamp)
    }
}
/// Session-bound adapter with original pending-generation read admission.
/// Global lifecycle gates are not connected: NOT an IPC-ready activation entry.
// Remove when the native operation controller consumes this guarded adapter.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) async fn with_selected_owner<R>(
    session: &crate::secrets::session::SecretSession,
    expected_identity: &StorageIdentity,
    expected_epoch: u64,
    selection: &crate::app_store::isolated_scope::SelectionSnapshot,
    operation: impl FnOnce(&mut Owner<'_>) -> Result<R, Failure>,
) -> Result<R, Failure> {
    let _sync = crate::services::sync_protocol::sync_mutex().lock().await;
    let storage = session.read().map_err(|_| Failure::SourceChanged)?;
    if storage.identity() != *expected_identity
        || session.lifecycle_epoch() != expected_epoch
        || selection.source_data_root() != session.root()
    {
        return Err(Failure::SourceChanged);
    }
    let selected = crate::app_store::isolated_scope::lock_selected_source_at(selection)
        .map_err(|_| Failure::SourceChanged)?;
    let binding = || (storage.identity(), session.lifecycle_epoch());
    let mut owner = Owner {
        root: session.root(),
        storage: &storage,
        identity: expected_identity.clone(),
        epoch: expected_epoch,
        binding: &binding,
        selected: Some((session, &selected, selection)),
    };
    owner.check()?;
    let result = operation(&mut owner);
    owner.check()?;
    selected
        .verify_unchanged()
        .map_err(|_| Failure::SourceChanged)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{storage::PlaintextIdentity, VaultContext};
    fn plain() -> StorageContext {
        StorageContext::Plaintext(PlaintextIdentity {
            format_version: 1,
            store_id: "11111111-1111-4111-8111-111111111111".into(),
            generation_id: "22222222-2222-4222-8222-222222222222".into(),
            revision: 1,
        })
    }
    #[test]
    fn encrypted_initialization_reopens_using_actual_registered_codec() {
        let dir = crate::secrets::testing::tempdir().unwrap();
        let storage = StorageContext::Encrypted(VaultContext::generate().unwrap());
        let binding = || (storage.identity(), 7);
        let mut owner = Owner::new(dir.path(), &storage, 7, &binding);
        assert_eq!(owner.disposition().unwrap(), Disposition::Absent);
        owner.initialize().unwrap();
        assert_eq!(owner.disposition().unwrap(), Disposition::Clear);
        let bytes = std::fs::read(dir.path().join("workbuddy_native_transaction.json")).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
        let other = StorageContext::Encrypted(VaultContext::generate().unwrap());
        let bad = || (other.identity(), 7);
        assert!(Owner::new(dir.path(), &other, 7, &bad)
            .disposition()
            .is_err());
    }
    #[test]
    fn explicit_plaintext_is_registered_and_epoch_changes_refuse_writes() {
        let dir = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let epoch = std::cell::Cell::new(3);
        let binding = || (storage.identity(), epoch.get());
        let mut owner = Owner::new(dir.path(), &storage, 3, &binding);
        owner.initialize().unwrap();
        let path = dir.path().join("workbuddy_native_transaction.json");
        let before = std::fs::read(&path).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&before).is_ok());
        epoch.set(4);
        assert!(owner.initialize().is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
    #[test]
    fn invalid_explicit_mode_and_unreadable_documents_never_fallback_or_reinitialize() {
        let dir = crate::secrets::testing::tempdir().unwrap();
        let mut storage = plain();
        if let StorageContext::Plaintext(ref mut p) = storage {
            p.format_version = 999;
        }
        let binding = || (storage.identity(), 0);
        assert!(Owner::new(dir.path(), &storage, 0, &binding)
            .initialize()
            .is_err());
        assert!(Owner::new(dir.path(), &storage, 0, &binding)
            .disposition()
            .is_err());
        assert!(!dir
            .path()
            .join("workbuddy_native_transaction.json")
            .exists());
        let storage = plain();
        let binding = || (storage.identity(), 0);
        let path = dir.path().join("workbuddy_native_transaction.json");
        std::fs::write(&path, b"lpenc:damaged").unwrap();
        let mut owner = Owner::new(dir.path(), &storage, 0, &binding);
        assert!(owner.disposition().is_err());
        assert!(owner.initialize().is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"lpenc:damaged");
    }
    #[cfg(unix)]
    fn context() -> super::super::native::NativeContext {
        super::super::native::NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "synthetic-same-machine".into(),
        }
    }
    #[test]
    #[cfg(unix)]
    fn disk_owner_commit_receipt_survives_reopen_and_aba_rejects_old_ticket() {
        use super::super::native_transaction::*;
        let store = crate::secrets::testing::tempdir().unwrap();
        let client = crate::secrets::testing::tempdir().unwrap();
        let storage = StorageContext::Encrypted(VaultContext::generate().unwrap());
        let binding = || (storage.identity(), 1);
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        owner.initialize().unwrap();
        let path = client.path().join("workbuddy-desktop.info");
        std::fs::write(&path, b"synthetic-A").unwrap();
        let paths = Paths::at(client.path()).unwrap();
        let context = context();
        let mut io = Files;
        let old_stamp = owner.load().unwrap().0;
        let ticket = Admission::preview(
            &paths,
            &context,
            &old_stamp,
            "A-to-B",
            &observe(&mut io, &paths).unwrap(),
            b"synthetic-B",
        )
        .unwrap();
        apply(&paths, &mut io, &mut owner, &mut || true, &context, &ticket).unwrap();
        assert_eq!(owner.disposition().unwrap(), Disposition::Clear);
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        assert_eq!(
            apply(&paths, &mut io, &mut owner, &mut || true, &context, &ticket).unwrap(),
            Outcome::AlreadyApplied
        );
        let stamp = owner.load().unwrap().0;
        let back = Admission::preview(
            &paths,
            &context,
            &stamp,
            "B-to-A",
            &observe(&mut io, &paths).unwrap(),
            b"synthetic-A",
        )
        .unwrap();
        apply(&paths, &mut io, &mut owner, &mut || true, &context, &back).unwrap();
        let before = std::fs::read(store.path().join("workbuddy_native_transaction.json")).unwrap();
        assert!(apply(&paths, &mut io, &mut owner, &mut || true, &context, &ticket).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic-A");
        assert_eq!(
            std::fs::read(store.path().join("workbuddy_native_transaction.json")).unwrap(),
            before
        );
    }
    #[test]
    #[cfg(unix)]
    fn durable_prepared_reopens_and_restores_without_permanent_pending_receipt() {
        use super::super::native_transaction::*;
        struct Crash(Files);
        impl FileIo for Crash {
            fn read(&mut self, p: &Path) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, Failure> {
                self.0.read(p)
            }
            fn write(&mut self, p: &Path, b: &[u8]) -> Result<(), Failure> {
                self.0.write(p, b)?;
                panic!("synthetic crash after native auth write")
            }
            fn remove(&mut self, p: &Path) -> Result<(), Failure> {
                self.0.remove(p)
            }
        }
        let store = crate::secrets::testing::tempdir().unwrap();
        let client = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let binding = || (storage.identity(), 1);
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        owner.initialize().unwrap();
        let auth = client.path().join("workbuddy-desktop.info");
        let marker = client.path().join("workbuddy-desktop.info.logged-out");
        std::fs::write(&auth, b"synthetic-A").unwrap();
        std::fs::write(&marker, b"logged out").unwrap();
        let paths = Paths::at(client.path()).unwrap();
        let context = context();
        let mut io = Crash(Files);
        let stamp = owner.load().unwrap().0;
        let ticket = Admission::preview(
            &paths,
            &context,
            &stamp,
            "crash",
            &observe(&mut io, &paths).unwrap(),
            b"synthetic-B",
        )
        .unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(
                &paths,
                &mut io,
                &mut owner,
                &mut || true,
                &context,
                &ticket
            )))
            .is_err()
        );
        drop(owner);
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        assert_eq!(owner.disposition().unwrap(), Disposition::Pending);
        let stamp = owner.load().unwrap().0;
        assert_eq!(
            recover(
                &paths,
                &mut Files,
                &mut owner,
                &mut || true,
                &stamp,
                &context
            )
            .unwrap(),
            Outcome::Restored
        );
        assert_eq!(owner.disposition().unwrap(), Disposition::Clear);
        assert_eq!(std::fs::read(auth).unwrap(), b"synthetic-A");
        assert_eq!(std::fs::read(marker).unwrap(), b"logged out");
        let mut doc: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store.path().join("workbuddy_native_transaction.json")).unwrap(),
        )
        .unwrap();
        doc["version"] = 999.into();
        let bytes = serde_json::to_vec(&doc).unwrap();
        std::fs::write(
            store.path().join("workbuddy_native_transaction.json"),
            &bytes,
        )
        .unwrap();
        assert!(owner.disposition().is_err());
        assert!(owner.initialize().is_err());
        assert_eq!(
            std::fs::read(store.path().join("workbuddy_native_transaction.json")).unwrap(),
            bytes
        );
    }
    #[test]
    fn aliases_and_identity_change_refuse_owner_writes() {
        let store = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let changed = std::cell::Cell::new(false);
        let binding = || {
            let mut id = storage.identity();
            if changed.get() {
                if let StorageIdentity::Plaintext(ref mut p) = id {
                    p.revision += 1;
                }
            }
            (id, 1)
        };
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        owner.initialize().unwrap();
        let path = store.path().join("workbuddy_native_transaction.json");
        let bytes = std::fs::read(&path).unwrap();
        changed.set(true);
        assert!(owner.disposition().is_err());
        assert!(owner.initialize().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        changed.set(false);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(owner.initialize().is_err());
        #[cfg(unix)]
        {
            std::fs::remove_dir(&path).unwrap();
            let outside = store.path().join("unknown");
            std::fs::write(&outside, b"sentinel").unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert!(owner.initialize().is_err());
            assert_eq!(std::fs::read(outside).unwrap(), b"sentinel");
        }
    }

    #[test]
    #[cfg(unix)]
    fn durable_write_then_binding_failure_keeps_pending_and_no_native_write() {
        use super::super::native_transaction::*;
        let store = crate::secrets::testing::tempdir().unwrap();
        let client = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let armed = std::cell::Cell::new(false);
        let file = store.path().join("workbuddy_native_transaction.json");
        let binding = || {
            let prepared = std::fs::read(&file)
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .is_some_and(|v| v["journal"]["phase"] == 0);
            (
                storage.identity(),
                if armed.get() && prepared { 2 } else { 1 },
            )
        };
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        owner.initialize().unwrap();
        let auth = client.path().join("workbuddy-desktop.info");
        std::fs::write(&auth, b"synthetic-A").unwrap();
        let paths = Paths::at(client.path()).unwrap();
        let context = context();
        let stamp = owner.load().unwrap().0;
        let ticket = Admission::preview(
            &paths,
            &context,
            &stamp,
            "ambiguous",
            &observe(&mut Files, &paths).unwrap(),
            b"synthetic-B",
        )
        .unwrap();
        armed.set(true);
        assert!(apply(
            &paths,
            &mut Files,
            &mut owner,
            &mut || true,
            &context,
            &ticket
        )
        .is_err());
        assert_eq!(std::fs::read(&auth).unwrap(), b"synthetic-A");
        armed.set(false);
        assert_eq!(owner.disposition().unwrap(), Disposition::Pending);
        let pending = owner.load().unwrap().0;
        assert!(pending != stamp);
        assert!(owner.initialize().is_err());
        recover(
            &paths,
            &mut Files,
            &mut owner,
            &mut || true,
            &pending,
            &context,
        )
        .unwrap();
        assert_eq!(owner.disposition().unwrap(), Disposition::Clear);
    }
    #[test]
    fn strict_owned_wire_rejects_duplicate_unknown_phase_and_bad_terminal_root() {
        let store = crate::secrets::testing::tempdir().unwrap();
        let client = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let binding = || (storage.identity(), 1);
        let owner = Owner::new(store.path(), &storage, 1, &binding);
        let (_, _, original) =
            super::testing::phase_receipts(store.path(), &storage, client.path());
        let path = store.path().join("workbuddy_native_transaction.json");
        for (key, value) in [
            ("phase", serde_json::json!(99)),
            ("root", serde_json::json!("/tmp/../unknown")),
            ("version", serde_json::json!(99)),
        ] {
            let mut doc: serde_json::Value = serde_json::from_slice(&original).unwrap();
            doc["journal"][key] = value;
            let altered = serde_json::to_vec(&doc).unwrap();
            std::fs::write(&path, &altered).unwrap();
            assert!(
                owner.disposition().is_err(),
                "must reject malformed owned journal field {key}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), altered);
        }
        let mut duplicate = b"{\"version\":1,".to_vec();
        duplicate.extend_from_slice(&original[1..]);
        std::fs::write(&path, &duplicate).unwrap();
        assert!(owner.disposition().is_err());
    }

    #[test]
    fn registered_journal_identity_rejects_other_file_ciphertext_without_fallback() {
        use crate::secrets::owned_file::{
            OwnedFile, AUTH_FILES, WORKBUDDY_FILE, WORKBUDDY_NATIVE_JOURNAL_FILE,
        };
        let store = crate::secrets::testing::tempdir().unwrap();
        let storage = StorageContext::Encrypted(VaultContext::generate().unwrap());
        let binding = || (storage.identity(), 1);
        let journal = OwnedFile::registered(WORKBUDDY_NATIVE_JOURNAL_FILE).unwrap();
        assert!(!journal.allows_legacy_plaintext());
        assert!(AUTH_FILES
            .iter()
            .all(|f| f.filename() != WORKBUDDY_NATIVE_JOURNAL_FILE));
        let wrong = OwnedFile::registered(WORKBUDDY_FILE)
            .unwrap()
            .encode_storage(&storage, b"synthetic-other-file")
            .unwrap();
        let path = store.path().join(WORKBUDDY_NATIVE_JOURNAL_FILE);
        std::fs::write(&path, &wrong).unwrap();
        let mut owner = Owner::new(store.path(), &storage, 1, &binding);
        assert!(owner.disposition().is_err());
        assert!(owner.initialize().is_err());
        assert_eq!(std::fs::read(path).unwrap(), wrong);
    }
    #[test]
    fn pure_inspect_uses_borrowed_old_context_while_original_write_guard_is_held() {
        let root = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let binding = || (storage.identity(), 0);
        assert_eq!(inspect(root.path(), &storage).unwrap(), Disposition::Absent);
        let mut owner = Owner::new(root.path(), &storage, 0, &binding);
        owner.initialize().unwrap();
        let path = root.path().join("workbuddy_native_transaction.json");
        let before = std::fs::read(&path).unwrap();
        let session = crate::secrets::session::SecretSession::from_storage_context(
            root.path().to_path_buf(),
            storage.clone(),
        );
        let write = session.write().unwrap();
        assert_eq!(inspect(root.path(), &write).unwrap(), Disposition::Clear);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut invalid = plain();
        if let StorageContext::Plaintext(ref mut p) = invalid {
            p.format_version = 999;
        }
        assert!(inspect(root.path(), &invalid).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn pure_inspect_distinguishes_actual_prepared_restored_and_committed_without_writing() {
        use super::super::native_transaction::*;
        struct Crash(Files);
        impl FileIo for Crash {
            fn read(&mut self, p: &Path) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, Failure> {
                self.0.read(p)
            }
            fn write(&mut self, p: &Path, b: &[u8]) -> Result<(), Failure> {
                self.0.write(p, b)?;
                panic!("synthetic auth interruption")
            }
            fn remove(&mut self, p: &Path) -> Result<(), Failure> {
                self.0.remove(p)
            }
        }
        let root = crate::secrets::testing::tempdir().unwrap();
        let client = crate::secrets::testing::tempdir().unwrap();
        let storage = plain();
        let binding = || (storage.identity(), 0);
        let mut owner = Owner::new(root.path(), &storage, 0, &binding);
        owner.initialize().unwrap();
        std::fs::write(client.path().join("workbuddy-desktop.info"), b"synthetic-A").unwrap();
        let paths = Paths::at(client.path()).unwrap();
        let context = context();
        let mut io = Crash(Files);
        let stamp = owner.load().unwrap().0;
        let ticket = Admission::preview(
            &paths,
            &context,
            &stamp,
            "inspect-crash",
            &observe(&mut io, &paths).unwrap(),
            b"synthetic-B",
        )
        .unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply(
                &paths,
                &mut io,
                &mut owner,
                &mut || true,
                &context,
                &ticket
            )))
            .is_err()
        );
        let path = root.path().join("workbuddy_native_transaction.json");
        let prepared = std::fs::read(&path).unwrap();
        assert_eq!(
            inspect(root.path(), &storage).unwrap(),
            Disposition::Pending
        );
        assert_eq!(std::fs::read(&path).unwrap(), prepared);
        let stamp = owner.load().unwrap().0;
        recover(
            &paths,
            &mut Files,
            &mut owner,
            &mut || true,
            &stamp,
            &context,
        )
        .unwrap();
        let restored = std::fs::read(&path).unwrap();
        assert_eq!(inspect(root.path(), &storage).unwrap(), Disposition::Clear);
        assert_eq!(std::fs::read(&path).unwrap(), restored);
        let stamp = owner.load().unwrap().0;
        let ticket = Admission::preview(
            &paths,
            &context,
            &stamp,
            "inspect-commit",
            &observe(&mut Files, &paths).unwrap(),
            b"synthetic-B",
        )
        .unwrap();
        apply(
            &paths,
            &mut Files,
            &mut owner,
            &mut || true,
            &context,
            &ticket,
        )
        .unwrap();
        let committed = std::fs::read(&path).unwrap();
        assert_eq!(inspect(root.path(), &storage).unwrap(), Disposition::Clear);
        assert_eq!(std::fs::read(&path).unwrap(), committed);
    }
    #[test]
    fn pure_inspect_distinguishes_synthetic_codec_phases_on_every_platform() {
        let root = crate::secrets::testing::tempdir().unwrap();
        let client = crate::secrets::testing::tempdir().unwrap();
        for storage in [
            plain(),
            StorageContext::Encrypted(crate::secrets::VaultContext::generate().unwrap()),
        ] {
            let (prepared, restored, committed) =
                super::testing::phase_receipts(root.path(), &storage, client.path());
            let path = root
                .path()
                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
            for (bytes, expected) in [
                (prepared, Disposition::Pending),
                (restored, Disposition::Clear),
                (committed, Disposition::Clear),
            ] {
                std::fs::write(&path, &bytes).unwrap();
                assert_eq!(inspect(root.path(), &storage).unwrap(), expected);
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
            std::fs::remove_file(&path).unwrap();
        }
    }
    #[test]
    fn pure_inspect_never_treats_wrong_key_corruption_or_unknown_version_as_absent() {
        let root = crate::secrets::testing::tempdir().unwrap();
        let storage = StorageContext::Encrypted(VaultContext::generate().unwrap());
        let binding = || (storage.identity(), 0);
        let mut owner = Owner::new(root.path(), &storage, 0, &binding);
        owner.initialize().unwrap();
        let path = root.path().join("workbuddy_native_transaction.json");
        let original = std::fs::read(&path).unwrap();
        assert!(inspect(
            root.path(),
            &StorageContext::Encrypted(VaultContext::generate().unwrap())
        )
        .is_err());
        assert!(inspect(root.path(), &plain()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let file = owner.file().unwrap();
        let raw = file.decode_storage(&storage, &original).unwrap();
        let mut doc: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        doc["version"] = 999.into();
        let bytes = file
            .encode_storage(&storage, &serde_json::to_vec(&doc).unwrap())
            .unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(inspect(root.path(), &storage).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::write(&path, b"lpenc:corrupt").unwrap();
        assert!(inspect(root.path(), &storage).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"lpenc:corrupt");
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::app_store::isolated_scope::{
        capture_selection_at, lock_selection_at, SelectionSnapshot,
    };
    use crate::secrets::{session::SecretSession, storage::PlaintextIdentity};
    use std::sync::{mpsc, Arc};
    struct Fixture {
        _root: tempfile::TempDir,
        store: std::path::PathBuf,
        session: Arc<SecretSession>,
        selection: SelectionSnapshot,
    }
    impl Fixture {
        fn new() -> Self {
            let root = crate::secrets::testing::tempdir().unwrap();
            let data = root.path().join("data");
            let device = root.path().join("device");
            let control = root.path().join("control");
            for p in [&data, &device, &control] {
                std::fs::create_dir(p).unwrap();
            }
            let store = control.join("app_paths.json");
            let selection = capture_selection_at(&store, &data, &device).unwrap();
            let storage = StorageContext::Plaintext(PlaintextIdentity {
                format_version: 1,
                store_id: "11111111-1111-4111-8111-111111111111".into(),
                generation_id: "22222222-2222-4222-8222-222222222222".into(),
                revision: 1,
            });
            let session = SecretSession::from_storage_context(data, storage);
            Self {
                _root: root,
                store,
                session,
                selection,
            }
        }
        fn identity(&self) -> StorageIdentity {
            self.session.read().unwrap().identity()
        }
        fn path(&self) -> std::path::PathBuf {
            self.session
                .root()
                .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        }
    }
    fn run<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }
    #[test]
    fn adapter_holds_original_sync_session_and_selection_guards_through_operation() {
        let f = Fixture::new();
        let id = f.identity();
        let session = f.session.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let thread = run(with_selected_owner(
            &f.session,
            &id,
            0,
            &f.selection,
            |owner| {
                assert!(crate::services::sync_protocol::sync_mutex()
                    .try_lock()
                    .is_err());
                assert!(lock_selection_at(&f.store).is_err());
                owner.initialize()?;
                let thread = std::thread::spawn(move || {
                    started_tx.send(()).unwrap();
                    let _write = session.write().unwrap();
                    finished_tx.send(()).unwrap();
                });
                started_rx.recv().unwrap();
                assert!(finished_rx.try_recv().is_err());
                Ok(thread)
            },
        ))
        .unwrap();
        thread.join().unwrap();
        finished_rx.recv().unwrap();
        assert!(lock_selection_at(&f.store).is_ok());
        assert!(f.path().exists());
    }
    #[test]
    fn adapter_rechecks_epoch_and_selector_before_owner_write() {
        for selector in [false, true] {
            let f = Fixture::new();
            let id = f.identity();
            let result = run(with_selected_owner(
                &f.session,
                &id,
                0,
                &f.selection,
                |owner| {
                    if selector {
                        std::fs::write(&f.store, b"{\"changed\":true}").unwrap();
                    } else {
                        f.session.invalidate_database_leases();
                    }
                    owner.initialize()
                },
            ));
            assert!(result.is_err());
            assert!(!f.path().exists());
        }
    }
    #[test]
    fn adapter_rejects_stale_identity_epoch_root_and_blocked_session_before_callback() {
        for kind in 0..4 {
            let f = Fixture::new();
            let mut id = f.identity();
            let mut epoch = 0;
            let mut selection = f.selection.clone();
            match kind {
                0 => {
                    if let StorageIdentity::Plaintext(ref mut p) = id {
                        p.revision += 1
                    }
                }
                1 => epoch = 1,
                2 => {
                    let other = f._root.path().join("other");
                    std::fs::create_dir(&other).unwrap();
                    selection = capture_selection_at(&f.store, &other, &other).unwrap();
                }
                _ => f.session.set_blocked(true),
            }
            let called = std::cell::Cell::new(false);
            assert!(run(with_selected_owner(
                &f.session,
                &id,
                epoch,
                &selection,
                |_| {
                    called.set(true);
                    Ok(())
                }
            ))
            .is_err());
            assert!(!called.get());
            assert!(!f.path().exists());
        }
    }
    #[test]
    fn adapter_fails_when_original_selector_consumer_owns_lock() {
        let f = Fixture::new();
        let id = f.identity();
        let _lock = lock_selection_at(&f.store).unwrap();
        let called = std::cell::Cell::new(false);
        assert!(run(with_selected_owner(
            &f.session,
            &id,
            0,
            &f.selection,
            |_| {
                called.set(true);
                Ok(())
            }
        ))
        .is_err());
        assert!(!called.get());
        assert!(!f.path().exists());
    }
    #[test]
    fn adapter_waits_for_existing_sync_owner_before_entering_callback() {
        use std::future::Future;
        let f = Fixture::new();
        let id = f.identity();
        let called = std::cell::Cell::new(false);
        let held = run(crate::services::sync_protocol::sync_mutex().lock());
        let mut future = Box::pin(with_selected_owner(
            &f.session,
            &id,
            0,
            &f.selection,
            |_| {
                called.set(true);
                Ok(())
            },
        ));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(!called.get());
        drop(held);
        run(future).unwrap();
        assert!(called.get());
    }
    #[test]
    fn adapter_preserves_selected_encrypted_mode_and_releases_on_failure() {
        let f = Fixture::new();
        let encrypted =
            StorageContext::Encrypted(crate::secrets::VaultContext::generate().unwrap());
        let session =
            SecretSession::from_storage_context(f.session.root().to_path_buf(), encrypted);
        let id = session.read().unwrap().identity();
        run(with_selected_owner(
            &session,
            &id,
            0,
            &f.selection,
            |owner| owner.initialize(),
        ))
        .unwrap();
        let bytes = std::fs::read(f.path()).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
        assert_eq!(
            run(with_selected_owner(
                &session,
                &id,
                0,
                &f.selection,
                |owner| owner.disposition()
            ))
            .unwrap(),
            Disposition::Clear
        );
        let result: Result<(), Failure> =
            run(with_selected_owner(&session, &id, 0, &f.selection, |_| {
                Err(Failure::Io)
            }));
        assert!(result.is_err());
        assert!(lock_selection_at(&f.store).is_ok());
        assert_eq!(std::fs::read(f.path()).unwrap(), bytes);
        // Explicit plaintext context must not reinterpret an existing encrypted owner.
        assert!(run(with_selected_owner(
            &f.session,
            &f.identity(),
            0,
            &f.selection,
            |owner| owner.disposition()
        ))
        .is_err());
        assert_eq!(std::fs::read(f.path()).unwrap(), bytes);
    }
    #[test]
    fn transition_intent_presence_blocks_callback_without_interpreting_bytes() {
        for bytes in [
            b"".as_slice(),
            b"{}",
            b"not-json synthetic transition intent",
        ] {
            let f = Fixture::new();
            let intent = f.session.root().join(crate::secrets::transition::INTENT);
            std::fs::write(&intent, bytes).unwrap();
            let called = std::cell::Cell::new(false);
            let result = run(with_selected_owner(
                &f.session,
                &f.identity(),
                0,
                &f.selection,
                |owner| {
                    called.set(true);
                    owner.initialize()
                },
            ));
            assert_eq!(result, Err(Failure::RecoveryRequired));
            assert!(!called.get());
            assert!(!f.path().exists());
            assert_eq!(std::fs::read(&intent).unwrap(), bytes);
            // The fixture simulates removal by the original transition owner;
            // native admission itself neither removes nor recovers that intent.
            std::fs::remove_file(&intent).unwrap();
            run(with_selected_owner(
                &f.session,
                &f.identity(),
                0,
                &f.selection,
                |owner| owner.initialize(),
            ))
            .unwrap();
            assert!(f.path().is_file());
        }
    }
    #[test]
    fn transition_reader_errors_block_callback_and_preserve_foreign_artifacts() {
        for directory in [false, true] {
            let f = Fixture::new();
            let intent = f.session.root().join(crate::secrets::transition::INTENT);
            if directory {
                std::fs::create_dir(&intent).unwrap();
                std::fs::write(intent.join("sentinel"), b"preserve").unwrap();
            } else {
                std::fs::File::create(&intent)
                    .unwrap()
                    .set_len(32 * 1024 * 1024 + 1)
                    .unwrap();
            }
            let called = std::cell::Cell::new(false);
            let result = run(with_selected_owner(
                &f.session,
                &f.identity(),
                0,
                &f.selection,
                |_| {
                    called.set(true);
                    Ok(())
                },
            ));
            assert_eq!(result, Err(Failure::RecoveryRequired));
            assert!(!called.get());
            assert!(!f.path().exists());
            if directory {
                assert_eq!(std::fs::read(intent.join("sentinel")).unwrap(), b"preserve");
            } else {
                assert_eq!(
                    std::fs::metadata(intent).unwrap().len(),
                    32 * 1024 * 1024 + 1
                );
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn transition_alias_blocks_without_reading_or_modifying_target() {
        let f = Fixture::new();
        let target = f._root.path().join("foreign-intent");
        std::fs::write(&target, b"preserve foreign intent").unwrap();
        let intent = f.session.root().join(crate::secrets::transition::INTENT);
        std::os::unix::fs::symlink(&target, &intent).unwrap();
        let result = run(with_selected_owner(
            &f.session,
            &f.identity(),
            0,
            &f.selection,
            |owner| owner.initialize(),
        ));
        assert_eq!(result, Err(Failure::RecoveryRequired));
        assert!(!f.path().exists());
        assert!(std::fs::symlink_metadata(&intent)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(target).unwrap(), b"preserve foreign intent");
    }
    #[test]
    fn transition_appearing_inside_callback_blocks_initial_and_existing_owner_writes() {
        for initialized in [false, true] {
            let f = Fixture::new();
            let intent = f.session.root().join(crate::secrets::transition::INTENT);
            let mut before = None;
            let result = run(with_selected_owner(
                &f.session,
                &f.identity(),
                0,
                &f.selection,
                |owner| {
                    let state = if initialized {
                        owner.initialize()?;
                        before = Some(std::fs::read(f.path()).unwrap());
                        owner.read()?.unwrap()
                    } else {
                        super::super::native_transaction::owner_wire::State::initial(
                            uuid::Uuid::new_v4().to_string(),
                        )
                    };
                    std::fs::write(&intent, b"synthetic competing transition").unwrap();
                    // Direct write proves a guard at persistence, not only at load/constructor.
                    assert_eq!(owner.write(&state), Err(Failure::RecoveryRequired));
                    assert!(owner.read().is_err());
                    Ok(())
                },
            ));
            assert_eq!(result, Err(Failure::RecoveryRequired));
            assert_eq!(std::fs::read(f.path()).ok(), before);
            assert_eq!(
                std::fs::read(&intent).unwrap(),
                b"synthetic competing transition"
            );
        }
    }
    #[derive(Clone, Copy)]
    enum LifecycleIntent {
        Reset,
        Bootstrap,
        Rewrap,
    }
    const LIFECYCLE_INTENTS: [LifecycleIntent; 3] = [
        LifecycleIntent::Reset,
        LifecycleIntent::Bootstrap,
        LifecycleIntent::Rewrap,
    ];
    impl LifecycleIntent {
        fn path(self, root: &Path) -> std::path::PathBuf {
            match self {
                Self::Reset => root.parent().unwrap().join(format!(
                    ".{}-vault-reset",
                    root.file_name().unwrap().to_str().unwrap()
                )),
                Self::Bootstrap => root.join(".vault-bootstrap"),
                Self::Rewrap => root.join(".vault-rewrap"),
            }
        }
        fn read(self, root: &Path) -> Result<Option<Vec<u8>>, crate::error::AppError> {
            match self {
                Self::Reset => crate::secrets::reset::pending_record(root),
                Self::Bootstrap => crate::secrets::bootstrap_restore::pending_record(root),
                Self::Rewrap => crate::secrets::rewrap::pending_record(root),
            }
        }
    }
    fn lifecycle_run<R>(
        f: &Fixture,
        operation: impl FnOnce(&mut Owner<'_>) -> Result<R, Failure>,
    ) -> Result<R, Failure> {
        run(with_selected_owner(
            &f.session,
            &f.identity(),
            0,
            &f.selection,
            operation,
        ))
    }
    #[test]
    fn lifecycle_intents_block_callback_and_original_owner_removal_reopens_admission() {
        for kind in LIFECYCLE_INTENTS {
            for bytes in [b"".as_slice(), b"{}", b"unknown synthetic lifecycle intent"] {
                let f = Fixture::new();
                let path = kind.path(f.session.root());
                std::fs::write(&path, bytes).unwrap();
                assert_eq!(kind.read(f.session.root()).unwrap().as_deref(), Some(bytes));
                let called = std::cell::Cell::new(false);
                let result = lifecycle_run(&f, |owner| {
                    called.set(true);
                    owner.initialize()
                });
                assert_eq!(result, Err(Failure::RecoveryRequired));
                assert!(!called.get());
                assert!(!f.path().exists());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                // Fixture simulates original owner's completion. Native code never removes it.
                std::fs::remove_file(&path).unwrap();
                assert!(kind.read(f.session.root()).unwrap().is_none());
                lifecycle_run(&f, |owner| owner.initialize()).unwrap();
                assert!(f.path().is_file());
            }
        }
    }
    #[test]
    fn lifecycle_reader_errors_preserve_directories_and_oversized_records() {
        for kind in LIFECYCLE_INTENTS {
            for directory in [false, true] {
                let f = Fixture::new();
                let path = kind.path(f.session.root());
                if directory {
                    std::fs::create_dir(&path).unwrap();
                    std::fs::write(path.join("sentinel"), b"preserve").unwrap();
                } else {
                    std::fs::File::create(&path)
                        .unwrap()
                        .set_len(1024 * 1024 + 1)
                        .unwrap();
                }
                assert!(kind.read(f.session.root()).is_err());
                let called = std::cell::Cell::new(false);
                assert_eq!(
                    lifecycle_run(&f, |_| {
                        called.set(true);
                        Ok(())
                    }),
                    Err(Failure::RecoveryRequired)
                );
                assert!(!called.get());
                assert!(!f.path().exists());
                if directory {
                    assert_eq!(std::fs::read(path.join("sentinel")).unwrap(), b"preserve");
                } else {
                    assert_eq!(std::fs::metadata(path).unwrap().len(), 1024 * 1024 + 1);
                }
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn lifecycle_aliases_refuse_without_following_or_deleting_foreign_targets() {
        for kind in LIFECYCLE_INTENTS {
            let f = Fixture::new();
            let path = kind.path(f.session.root());
            let foreign = f._root.path().join("foreign-artifact");
            std::fs::write(&foreign, b"preserve foreign bytes").unwrap();
            std::os::unix::fs::symlink(&foreign, &path).unwrap();
            assert!(kind.read(f.session.root()).is_err());
            assert_eq!(
                lifecycle_run(&f, |owner| owner.initialize()),
                Err(Failure::RecoveryRequired)
            );
            assert!(!f.path().exists());
            assert!(std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read(foreign).unwrap(), b"preserve foreign bytes");
        }
    }
    #[test]
    fn lifecycle_intent_appearing_in_callback_blocks_every_initial_or_existing_write() {
        for kind in LIFECYCLE_INTENTS {
            for initialized in [false, true] {
                let f = Fixture::new();
                let path = kind.path(f.session.root());
                let mut before = None;
                let result = lifecycle_run(&f, |owner| {
                    let state = if initialized {
                        owner.initialize()?;
                        before = Some(std::fs::read(f.path()).unwrap());
                        owner.read()?.unwrap()
                    } else {
                        super::super::native_transaction::owner_wire::State::initial(
                            uuid::Uuid::new_v4().to_string(),
                        )
                    };
                    std::fs::write(&path, b"competing synthetic operation").unwrap();
                    assert_eq!(owner.write(&state), Err(Failure::RecoveryRequired));
                    assert!(owner.read().is_err());
                    Ok(())
                });
                assert_eq!(result, Err(Failure::RecoveryRequired));
                assert_eq!(std::fs::read(f.path()).ok(), before);
                assert_eq!(
                    std::fs::read(path).unwrap(),
                    b"competing synthetic operation"
                );
            }
        }
    }
    #[test]
    fn absent_lifecycle_readers_and_native_inspection_create_no_owner_artifacts() {
        let f = Fixture::new();
        for kind in LIFECYCLE_INTENTS {
            assert!(kind.read(f.session.root()).unwrap().is_none());
        }
        assert_eq!(
            lifecycle_run(&f, |owner| owner.disposition()).unwrap(),
            Disposition::Absent
        );
        assert!(!f.path().exists());
        for kind in LIFECYCLE_INTENTS {
            assert!(!kind.path(f.session.root()).exists());
        }
        assert_eq!(std::fs::read_dir(f.session.root()).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(f._root.path()).unwrap().count(), 3);
    }
}

/// Synthetic receipt fixtures for the original generation tests. These exercise
/// actual journal serialization; Unix uses real transaction phases, nonUnix uses typed
/// codec-only fixtures. Neither path is a handwritten wire schema or activation proof.
#[cfg(test)]
pub(crate) mod testing {
    /// Initial registered owner fixture; no native activation or credential IO.
    pub(crate) fn clear_receipt(root: &Path, storage: &StorageContext) -> Vec<u8> {
        let binding = || (storage.identity(), 0);
        let mut owner = Owner::new(root, storage, 0, &binding);
        owner.initialize().unwrap();
        std::fs::read(root.join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)).unwrap()
    }
    use super::super::native_transaction::*;
    use super::*;
    pub(crate) async fn selected(
        session: &crate::secrets::session::SecretSession,
        identity: &StorageIdentity,
        epoch: u64,
        selection: &crate::app_store::isolated_scope::SelectionSnapshot,
        operation: impl FnOnce(&mut Owner<'_>) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        super::with_selected_owner(session, identity, epoch, selection, operation).await
    }
    pub(crate) fn initialize(owner: &mut Owner<'_>) -> Result<(), Failure> {
        owner.initialize()
    }
    /// Codec/phase-only fixture; it does not invoke a native filesystem transaction.
    pub(crate) fn phase_receipts(
        root: &Path,
        storage: &StorageContext,
        client: &Path,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let binding = || (storage.identity(), 0);
        let owner = Owner::new(root, storage, 0, &binding);
        let path = root.join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
        let encode = |phase| {
            owner
                .write(&owner_wire::State::fixture(client, phase))
                .unwrap();
            std::fs::read(&path).unwrap()
        };
        (
            encode(Phase::Prepared),
            encode(Phase::Restored),
            encode(Phase::Committed),
        )
    }
    #[cfg(not(unix))]
    pub(crate) fn receipts(
        root: &Path,
        storage: &StorageContext,
        client: &Path,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        // Generation gates are platform-independent, even where activation is unsupported.
        phase_receipts(root, storage, client)
    }
    #[cfg(unix)]
    pub(crate) fn receipts(
        root: &Path,
        storage: &StorageContext,
        client: &Path,
    ) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        struct Crash(Files);
        impl FileIo for Crash {
            fn read(&mut self, p: &Path) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, Failure> {
                self.0.read(p)
            }
            fn write(&mut self, p: &Path, b: &[u8]) -> Result<(), Failure> {
                self.0.write(p, b)?;
                panic!("synthetic transaction interruption")
            }
            fn remove(&mut self, p: &Path) -> Result<(), Failure> {
                self.0.remove(p)
            }
        }
        let binding = || (storage.identity(), 0);
        let mut owner = Owner::new(root, storage, 0, &binding);
        owner.initialize().unwrap();
        let paths = Paths::at(client).unwrap();
        std::fs::write(client.join("workbuddy-desktop.info"), b"A").unwrap();
        let context = super::super::native::NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "synthetic-machine".into(),
        };
        let path = root.join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let stamp = owner.load().unwrap().0;
            let mut io = Crash(Files);
            let ticket = Admission::preview(
                &paths,
                &context,
                &stamp,
                "fixture-prepared",
                &observe(&mut io, &paths).unwrap(),
                b"B",
            )
            .unwrap();
            let _ = apply(&paths, &mut io, &mut owner, &mut || true, &context, &ticket);
        }))
        .is_err());
        let prepared = std::fs::read(&path).unwrap();
        let stamp = owner.load().unwrap().0;
        recover(
            &paths,
            &mut Files,
            &mut owner,
            &mut || true,
            &stamp,
            &context,
        )
        .unwrap();
        let restored = std::fs::read(&path).unwrap();
        let stamp = owner.load().unwrap().0;
        let ticket = Admission::preview(
            &paths,
            &context,
            &stamp,
            "fixture-committed",
            &observe(&mut Files, &paths).unwrap(),
            b"B",
        )
        .unwrap();
        apply(
            &paths,
            &mut Files,
            &mut owner,
            &mut || true,
            &context,
            &ticket,
        )
        .unwrap();
        let committed = std::fs::read(&path).unwrap();
        (prepared, restored, committed)
    }
}
