//! Bounded untrusted observation -> original selected owner -> stored-only data.
//! No home discovery, source authentication, official-key access or native apply.
use super::{
    model::{Account, Credential, Domain, SavedAccount},
    native::NativeContext,
    native_transaction::Failure,
};
use crate::{
    app_store::isolated_scope::SelectionSnapshot,
    secrets::{session::SecretSession, storage::StorageIdentity},
};
use serde::Serialize;
use std::path::{Path, PathBuf};
pub(super) const AUTH_LIMIT: u64 = 4 * 1024 * 1024;
pub(super) const MARKER_LIMIT: u64 = 64 * 1024;
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreviewError {
    UnsafeSource,
    SourceChanged,
    UnreadableSource,
    InvalidDocument,
    UnsupportedContext,
    #[cfg(not(any(unix, windows)))]
    UnsupportedPlatform,
}
pub(super) fn marker_path(auth: &Path) -> PathBuf {
    let mut name = auth.as_os_str().to_os_string();
    name.push(".logged-out");
    PathBuf::from(name)
}
#[derive(PartialEq, Eq)]
struct Identity {
    volume: u64,
    index: u64,
}
fn identity(file: &std::fs::File) -> Result<Identity, PreviewError> {
    let metadata = file
        .metadata()
        .map_err(|_| PreviewError::UnreadableSource)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Identity {
            volume: metadata.dev(),
            index: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(PreviewError::UnsafeSource);
        }
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: live owned file handle and initialized writable structure for this call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(PreviewError::UnreadableSource);
        }
        Ok(Identity {
            volume: u64::from(information.dwVolumeSerialNumber),
            index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        Err(PreviewError::UnsupportedPlatform)
    }
}
fn parents(path: &Path) -> Result<Vec<Identity>, PreviewError> {
    let mut result = Vec::new();
    for parent in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if !parent.is_absolute() {
            continue;
        }
        let meta = match std::fs::symlink_metadata(parent) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(PreviewError::UnreadableSource),
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(PreviewError::UnsafeSource);
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::{fs::MetadataExt, fs::OpenOptionsExt};
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            };
            if meta.file_attributes() & 0x400 != 0 {
                return Err(PreviewError::UnsafeSource);
            }
            options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options
            .open(parent)
            .map_err(|_| PreviewError::UnsafeSource)?;
        if !file
            .metadata()
            .map_err(|_| PreviewError::UnreadableSource)?
            .is_dir()
        {
            return Err(PreviewError::UnsafeSource);
        }
        result.push(identity(&file)?);
    }
    Ok(result)
}
#[derive(PartialEq, Eq)]
pub(super) struct Sample {
    identity: Identity,
    modified: std::time::SystemTime,
    pub(super) bytes: zeroize::Zeroizing<Vec<u8>>,
}
fn read_file(path: &Path, limit: u64) -> Result<Option<Sample>, PreviewError> {
    use std::io::Read;
    let Some(mut file) = crate::config_file_io::open_regular_file(path, limit)
        .map_err(|_| PreviewError::UnsafeSource)?
    else {
        return Ok(None);
    };
    let before = identity(&file)?;
    let meta = file
        .metadata()
        .map_err(|_| PreviewError::UnreadableSource)?;
    let modified = meta
        .modified()
        .map_err(|_| PreviewError::UnreadableSource)?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PreviewError::UnreadableSource)?;
    let after = file
        .metadata()
        .map_err(|_| PreviewError::UnreadableSource)?;
    if bytes.len() as u64 > limit {
        return Err(PreviewError::UnsafeSource);
    }
    if identity(&file)? != before
        || after.len() != meta.len()
        || after
            .modified()
            .map_err(|_| PreviewError::UnreadableSource)?
            != modified
    {
        return Err(PreviewError::SourceChanged);
    }
    Ok(Some(Sample {
        identity: before,
        modified,
        bytes,
    }))
}
#[derive(PartialEq, Eq)]
pub(super) struct Pair {
    parents: Vec<Identity>,
    pub(super) auth: Option<Sample>,
    pub(super) marker: Option<Sample>,
}
pub(super) fn read_pair(auth: &Path) -> Result<Pair, PreviewError> {
    let parent = auth.parent().ok_or(PreviewError::UnsafeSource)?;
    let before = parents(parent)?;
    let auth_sample = read_file(auth, AUTH_LIMIT)?;
    let marker = read_file(&marker_path(auth), MARKER_LIMIT)?;
    if parents(parent)? != before {
        return Err(PreviewError::SourceChanged);
    }
    Ok(Pair {
        parents: before,
        auth: auth_sample,
        marker,
    })
}

/// Sealed observation, not verified provenance or a NativeSnapshot capability.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Observation {
    path: PathBuf,
    pair: Pair,
    context: NativeContext,
    uid: String,
}
#[cfg_attr(not(test), allow(dead_code))]
impl Observation {
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) fn realm_at(path: &Path) -> Result<Domain, Failure> {
        let pair = read_pair(path).map_err(|_| Failure::SourceChanged)?;
        if pair.marker.is_some() {
            return Err(Failure::RequestConflict);
        }
        let auth = pair.auth.as_ref().ok_or(Failure::RequestConflict)?;
        let realm = super::native::observe(&auth.bytes).map_err(|_| Failure::RequestConflict)?;
        Domain::parse(realm).map_err(|_| Failure::RequestConflict)
    }
    pub(super) fn read(path: &Path, context: &NativeContext) -> Result<Self, Failure> {
        if !path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(Failure::UnsafePath);
        }
        let pair = read_pair(path).map_err(|_| Failure::SourceChanged)?;
        if pair.marker.is_some() {
            return Err(Failure::RequestConflict);
        }
        let auth = pair.auth.as_ref().ok_or(Failure::RequestConflict)?;
        let uid = super::native::validate_stored_identity(&auth.bytes, context)
            .map_err(|_| Failure::RequestConflict)?;
        let result = Self {
            path: path.into(),
            pair,
            context: context.clone(),
            uid,
        };
        result.verify()?;
        Ok(result)
    }
    fn verify(&self) -> Result<(), Failure> {
        if read_pair(&self.path).map_err(|_| Failure::SourceChanged)? != self.pair {
            return Err(Failure::SourceChanged);
        }
        Ok(())
    }
}
/// Explicit original owner binding, never inferred from an observed path or UID.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Catalog<'a> {
    pub session: &'a SecretSession,
    pub identity: &'a StorageIdentity,
    pub epoch: u64,
    pub selection: &'a SelectionSnapshot,
}
/// Data lookup fence reuses original generation + credential equality, including
/// the persisted v2 row lifetime. Legacy v1 data never issues a lifetime receipt.
/// Private/zeroizing credential retention; no serialization or native execution admission.
/// Non-secret persistent choice of one existing row lifetime. The original
/// incarnation is required on reselect; ID/generation alone cannot renew it.
/// This data is neither verified source provenance nor an Admission ticket.
#[derive(Clone, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectionKey {
    pub(super) id: String,
    pub(super) generation: u64,
    pub(super) incarnation: uuid::Uuid,
}
#[derive(Serialize)]
pub(crate) struct CapturedAccount {
    selection: SelectionKey,
    label: String,
    realm: String,
}
#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Receipt {
    pub id: String,
    pub generation: u64,
    pub realm: String,
    credential: Credential,
}
impl Receipt {
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub(super) fn view(&self, label: &str) -> Result<CapturedAccount, Failure> {
        Ok(CapturedAccount {
            selection: self.selection_key()?,
            label: label.into(),
            realm: self.realm.clone(),
        })
    }
    #[cfg_attr(not(test), allow(dead_code))]
    #[cfg(unix)]
    pub(super) fn selection_key(&self) -> Result<SelectionKey, Failure> {
        Ok(SelectionKey {
            id: self.id.clone(),
            generation: self.generation,
            incarnation: self
                .credential
                .native_incarnation()
                .ok_or(Failure::RequestConflict)?,
        })
    }
    fn matches(&self, row: &SavedAccount, owner: &str) -> bool {
        row.account.has_native_lifetime()
            && self.id == row.account.id(owner)
            && self.realm == row.account.domain.host()
            && self.generation == row.generation
            && self.credential == row.account.credential
    }
    #[cfg(unix)]
    pub(super) fn read_under_owner(
        &self,
        owner: &super::native_owner::Owner<'_>,
        store_id: &str,
    ) -> Result<SavedAccount, Failure> {
        let rows = owner.load_accounts()?;
        let mut matches = rows
            .into_iter()
            .filter(|row| row.account.id(store_id) == self.id);
        let row = matches.next().ok_or(Failure::RequestConflict)?;
        if matches.next().is_some() || !self.matches(&row, store_id) {
            return Err(Failure::RequestConflict);
        }
        Ok(row)
    }
}
#[cfg_attr(not(test), allow(dead_code))]
impl Catalog<'_> {
    #[cfg(unix)]
    pub(super) async fn list_captures(&self) -> Result<Vec<CapturedAccount>, Failure> {
        super::native_owner::with_selected_owner(
            self.session,
            self.identity,
            self.epoch,
            self.selection,
            |owner| {
                let rows = owner.load_accounts()?;
                Ok(rows
                    .into_iter()
                    .filter_map(|row| {
                        let incarnation = row.account.credential.native_incarnation()?;
                        Some(CapturedAccount {
                            selection: SelectionKey {
                                id: row.account.id(self.identity.store_id()),
                                generation: row.generation,
                                incarnation,
                            },
                            label: row.account.label,
                            realm: row.account.domain.host().into(),
                        })
                    })
                    .collect())
            },
        )
        .await
    }
    #[cfg(unix)]
    pub(super) async fn select(&self, key: &SelectionKey) -> Result<Receipt, Failure> {
        super::native_owner::with_selected_owner(
            self.session,
            self.identity,
            self.epoch,
            self.selection,
            |owner| {
                let rows = owner.load_accounts()?;
                let mut matches = rows
                    .into_iter()
                    .filter(|row| row.account.id(self.identity.store_id()) == key.id);
                let row = matches.next().ok_or(Failure::RequestConflict)?;
                if matches.next().is_some()
                    || row.generation != key.generation
                    || row.account.credential.native_incarnation() != Some(key.incarnation)
                {
                    return Err(Failure::RequestConflict);
                }
                Ok(Receipt {
                    id: key.id.clone(),
                    generation: key.generation,
                    realm: row.account.domain.host().into(),
                    credential: row.account.credential.clone(),
                })
            },
        )
        .await
    }
    pub(super) async fn save(
        &self,
        observation: &Observation,
        expected: Option<&Receipt>,
        label: &str,
    ) -> Result<Receipt, Failure> {
        self.save_checked(observation, expected, label, || Ok(()))
            .await
    }
    pub(super) async fn save_checked(
        &self,
        observation: &Observation,
        expected: Option<&Receipt>,
        label: &str,
        mut verify_source: impl FnMut() -> Result<(), Failure>,
    ) -> Result<Receipt, Failure> {
        super::native_owner::with_selected_owner(
            self.session,
            self.identity,
            self.epoch,
            self.selection,
            |owner| {
                verify_source()?;
                observation.verify()?;
                let mut rows = owner.load_accounts()?;
                let domain = Domain::parse(&observation.context.realm)
                    .map_err(|_| Failure::RequestConflict)?;
                let matching: Vec<_> = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.account.domain == domain && r.account.uid == observation.uid)
                    .map(|(i, _)| i)
                    .collect();
                let bytes = &observation
                    .pair
                    .auth
                    .as_ref()
                    .ok_or(Failure::RequestConflict)?
                    .bytes;
                let index = match matching.as_slice() {
                    [] if expected.is_none() => {
                        let account = Account::from_observation(bytes, &observation.context, label)
                            .map_err(|_| Failure::RequestConflict)?;
                        rows.push(SavedAccount::new(account));
                        rows.len() - 1
                    }
                    [i] if expected.is_some_and(|receipt| {
                        receipt.matches(&rows[*i], self.identity.store_id())
                    }) =>
                    {
                        if !rows[*i].account.same_native_scope(&observation.context) {
                            return Err(Failure::RequestConflict);
                        }
                        let account = rows[*i]
                            .account
                            .recapture_observation(bytes, &observation.context, label)
                            .map_err(|_| Failure::RequestConflict)?;
                        rows[*i].generation = rows[*i]
                            .generation
                            .checked_add(1)
                            .ok_or(Failure::RequestConflict)?;
                        rows[*i].account = account;
                        *i
                    }
                    _ => return Err(Failure::RequestConflict),
                };
                let row = &rows[index];
                let receipt = Receipt {
                    id: row.account.id(self.identity.store_id()),
                    generation: row.generation,
                    realm: domain.host().into(),
                    credential: row.account.credential.clone(),
                };
                let mut verification = Ok(());
                let saved = owner.save_accounts(rows, || {
                    verification = verify_source().and_then(|_| observation.verify());
                    verification
                        .as_ref()
                        .map(|_| ())
                        .map_err(|_| Failure::SourceChanged)
                });
                verification?;
                saved?;
                Ok(receipt)
            },
        )
        .await
    }
    pub(super) async fn load(&self, receipt: &Receipt) -> Result<SavedAccount, Failure> {
        super::native_owner::with_selected_owner(
            self.session,
            self.identity,
            self.epoch,
            self.selection,
            |owner| {
                let rows = owner.load_accounts()?;
                let mut matches = rows
                    .into_iter()
                    .filter(|r| r.account.id(self.identity.store_id()) == receipt.id);
                let row = matches.next().ok_or(Failure::RequestConflict)?;
                if matches.next().is_some()
                    || !receipt.matches(&row, self.identity.store_id())
                    || !matches!(&row.account.credential, Credential::NativeOnly(_))
                {
                    return Err(Failure::RequestConflict);
                }
                Ok(row)
            },
        )
        .await
    }
}
