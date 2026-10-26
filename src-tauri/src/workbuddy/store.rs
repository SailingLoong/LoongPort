//! Persistence uses LoongPort's registered encrypted files and lifecycle owner.
use super::model::{Failure, SavedAccount};
use crate::secrets::{owned_file::OwnedFile, session::SecretSession, storage::StorageIdentity};
use serde::{Deserialize, Serialize};
use std::io::Read;
pub(crate) const FILE: &str = crate::secrets::owned_file::WORKBUDDY_FILE;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Document {
    accounts: Vec<SavedAccount>,
}
pub(crate) fn binding(session: &SecretSession) -> Result<StorageIdentity, Failure> {
    session
        .read()
        .map(|v| v.identity())
        .map_err(|_| Failure::StorageUnavailable)
}
pub(crate) fn load(
    session: &SecretSession,
    expected: &StorageIdentity,
) -> Result<Vec<SavedAccount>, Failure> {
    let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
    if &vault.identity() != expected {
        return Err(Failure::StorageUnavailable);
    }
    let owned = OwnedFile::registered(FILE).map_err(|_| Failure::StorageUnavailable)?;
    let path = owned.path(session);
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(Failure::StorageUnavailable);
        }
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(Failure::StorageUnavailable),
    };
    if !file
        .metadata()
        .map_err(|_| Failure::StorageUnavailable)?
        .is_file()
    {
        return Err(Failure::StorageUnavailable);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::StorageUnavailable)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Failure::StorageUnavailable);
    }
    let plaintext = owned
        .decode_storage(&vault, &bytes)
        .map_err(|_| Failure::StorageUnavailable)?;
    let doc: Document =
        serde_json::from_slice(&plaintext).map_err(|_| Failure::StorageUnavailable)?;
    if doc.accounts.len() > 200 {
        return Err(Failure::StorageUnavailable);
    }
    Ok(doc.accounts)
}
pub(crate) fn save(
    session: &SecretSession,
    expected: &StorageIdentity,
    accounts: Vec<SavedAccount>,
) -> Result<(), Failure> {
    let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
    if &vault.identity() != expected || accounts.len() > 200 {
        return Err(Failure::StorageUnavailable);
    }
    let plaintext = zeroize::Zeroizing::new(
        serde_json::to_vec(&Document { accounts }).map_err(|_| Failure::StorageUnavailable)?,
    );
    let owned = OwnedFile::registered(FILE).map_err(|_| Failure::StorageUnavailable)?;
    let ciphertext = owned
        .encode_storage(&vault, &plaintext)
        .map_err(|_| Failure::StorageUnavailable)?;
    if ciphertext.len() as u64 > MAX_BYTES {
        return Err(Failure::StorageUnavailable);
    }
    crate::secrets::session::ensure_owned_directory(session.root(), session.root())
        .map_err(|_| Failure::StorageUnavailable)?;
    crate::secrets::files::ensure_plaintext_destination(&vault, &owned.path(session))
        .map_err(|_| Failure::StorageUnavailable)?;
    crate::secrets::session::write_durable(&owned.path(session), &ciphertext)
        .map_err(|_| Failure::StorageUnavailable)
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) mod plaintext_tests {
    use super::*;
    use crate::secrets::storage::{PlaintextIdentity, StorageContext};

    #[cfg_attr(test, test)]
    fn plaintext_store_round_trips_without_key_and_preserves_binding_checks() {
        let root = tempfile::tempdir().unwrap();
        let session = SecretSession::from_storage_context(
            root.path().into(),
            StorageContext::Plaintext(PlaintextIdentity {
                format_version: 1,
                store_id: "11111111-1111-4111-8111-111111111111".into(),
                generation_id: "22222222-2222-4222-8222-222222222222".into(),
                revision: 1,
            }),
        );
        let original = binding(&session).unwrap();
        let account = SavedAccount::new(super::super::model::Account {
            uid: "synthetic-user".into(),
            domain: super::super::model::Domain::WorkBuddy,
            label: "Synthetic".into(),
            token: "synthetic-plaintext-token".into(),
        });
        save(&session, &original, vec![account]).unwrap();
        let bytes = std::fs::read(root.path().join(FILE)).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            document["accounts"][0]["account"]["token"],
            "synthetic-plaintext-token"
        );
        assert_eq!(
            load(&session, &original).unwrap()[0].account.token,
            "synthetic-plaintext-token"
        );
        assert!(!root.path().join("vault.json").exists());
        let mut changed = original.clone();
        if let StorageIdentity::Plaintext(identity) = &mut changed {
            identity.revision += 1;
        }
        assert!(load(&session, &changed).is_err());
        assert!(save(&session, &changed, vec![]).is_err());
        assert_eq!(std::fs::read(root.path().join(FILE)).unwrap(), bytes);
        let opaque = b"lpenc:synthetic-unavailable-key";
        std::fs::write(root.path().join(FILE), opaque).unwrap();
        assert!(save(&session, &original, vec![]).is_err());
        assert_eq!(std::fs::read(root.path().join(FILE)).unwrap(), opaque);
        std::fs::write(root.path().join(FILE), &bytes).unwrap();
        session.set_blocked(true);
        assert!(load(&session, &original).is_err());
    }

    #[cfg(feature = "test-hooks")]
    pub(crate) fn run() {
        plaintext_store_round_trips_without_key_and_preserves_binding_checks();
    }
}
