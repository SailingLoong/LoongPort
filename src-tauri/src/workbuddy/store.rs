//! Persistence uses LoongPort's registered encrypted files and lifecycle owner.
use super::model::{Failure, SavedAccount};
use crate::secrets::{owned_file::OwnedFile, session::SecretSession, VaultMetadata};
use serde::{Deserialize, Serialize};
use std::io::Read;
pub(crate) const FILE: &str = crate::secrets::owned_file::WORKBUDDY_FILE;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Document {
    accounts: Vec<SavedAccount>,
}
pub(crate) fn binding(session: &SecretSession) -> Result<VaultMetadata, Failure> {
    session
        .read()
        .map(|v| v.metadata().clone())
        .map_err(|_| Failure::StorageUnavailable)
}
pub(crate) fn load(
    session: &SecretSession,
    expected: &VaultMetadata,
) -> Result<Vec<SavedAccount>, Failure> {
    let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
    if vault.metadata() != expected {
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
        .decode(&vault, &bytes)
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
    expected: &VaultMetadata,
    accounts: Vec<SavedAccount>,
) -> Result<(), Failure> {
    let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
    if vault.metadata() != expected || accounts.len() > 200 {
        return Err(Failure::StorageUnavailable);
    }
    let plaintext = zeroize::Zeroizing::new(
        serde_json::to_vec(&Document { accounts }).map_err(|_| Failure::StorageUnavailable)?,
    );
    let owned = OwnedFile::registered(FILE).map_err(|_| Failure::StorageUnavailable)?;
    let ciphertext = owned
        .encode(&vault, &plaintext)
        .map_err(|_| Failure::StorageUnavailable)?;
    if ciphertext.len() as u64 > MAX_BYTES {
        return Err(Failure::StorageUnavailable);
    }
    crate::secrets::session::ensure_owned_directory(session.root(), session.root())
        .map_err(|_| Failure::StorageUnavailable)?;
    crate::secrets::session::write_durable(&owned.path(session), &ciphertext)
        .map_err(|_| Failure::StorageUnavailable)
}
