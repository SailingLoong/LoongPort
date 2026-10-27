//! Explicit configuration-root selection. The original protection/session owner
//! alone admits conversion and authenticates complete source/target inventories.
//! V2 records only immutable filesystem provenance, never protection mode/key IDs.
//!
//! Publication never hot-switches the running root or deletes original data.
//! Owner-authorized postpublication cleanup is delegated back to that same owner.
//! Unknown/pending candidates are retained and block another preparation. Legacy
//! v1 helpers remain separate and cannot create another scope after selection.
use crate::config_file_io::{read_regular_file, stage_write, write_durable_new};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(crate) const SELECTION_KEY: &str = "isolated_config";
const IDENTITY_FILE: &str = super::ISOLATED_SCOPE_IDENTITY_FILE;
const MAX_CONTROL_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct IsolatedScope {
    pub format_version: u32,
    pub scope_id: String,
    pub root: PathBuf,
    pub preserved_root: PathBuf,
    pub preserved_device_root: PathBuf,
    pub inspection_id: String,
    pub vault_id: String,
    pub key_id: String,
}

fn invalid() -> AppError {
    AppError::Config("config.isolated_scope_invalid".into())
}
fn conflict() -> AppError {
    AppError::Conflict("config.selection_changed".into())
}

/// Thin reuse of the existing pinned-identity lock owner. Ordinary writes never
/// reclaim any existing lock. A legacy UUID regular file at the same lock name
/// is unknown/ownerless and remains untouched, not converted or deleted.
pub(crate) struct SelectionLock {
    _guard: crate::zcode_file_lock::FileLock,
}
pub(crate) fn lock_selection_at(store: &Path) -> Result<SelectionLock, AppError> {
    let parent = store.parent().ok_or_else(invalid)?;
    validate_directory(parent)?;
    let guard = crate::zcode_file_lock::FileLock::acquire(
        &parent.join("app_paths.selection"),
        std::time::Duration::ZERO,
    )?;
    Ok(SelectionLock { _guard: guard })
}

/// Selection-bound private source lease; no protection/session lock is acquired here.
/// The caller already holds its storage guard. This owns the ORIGINAL selector
/// lock until scope exit, not a second native lock or a credential authority.
pub(crate) struct SelectedSourceGuard {
    snapshot: SelectionSnapshot,
    source_identity: Option<DirectoryIdentity>,
    device_identity: Option<DirectoryIdentity>,
    control_identity: Option<DirectoryIdentity>,
    lock_identity: Option<DirectoryIdentity>,
    _lock: SelectionLock,
}
impl SelectedSourceGuard {
    pub(crate) fn source_data_root(&self) -> &Path {
        self.snapshot.source_data_root()
    }
    pub(crate) fn verify_unchanged(&self) -> Result<(), AppError> {
        let parent = selected_source_control_parent(&self.snapshot)?;
        verify_snapshot(&self.snapshot)?;
        if directory_identity(&self.snapshot.root)? != self.source_identity
            || directory_identity(&self.snapshot.device_root)? != self.device_identity
            || directory_identity(parent)? != self.control_identity
            || directory_identity(&parent.join("app_paths.selection.lock"))? != self.lock_identity
        {
            return Err(conflict());
        }
        Ok(())
    }
}
/// Read-only admission shares original conversion's control-plane containment rule.
/// Unusual custom roots containing that plane are unsupported, never relocated.
fn selected_source_control_parent(expected: &SelectionSnapshot) -> Result<&Path, AppError> {
    let parent = expected.store.parent().ok_or_else(invalid)?;
    validate_directory(parent)?;
    validate_directory(&expected.root)?;
    validate_existing_ancestors(&expected.device_root)?;
    for preserved in [&expected.root, &expected.device_root] {
        if within_by_identity(parent, preserved)?
            || within_by_identity(&parent.join("isolated-configs"), preserved)?
        {
            return Err(invalid());
        }
    }
    Ok(parent)
}
pub(crate) fn lock_selected_source_at(
    expected: &SelectionSnapshot,
) -> Result<SelectedSourceGuard, AppError> {
    let parent = selected_source_control_parent(expected)?;
    verify_snapshot(expected)?;
    let source_identity = directory_identity(&expected.root)?;
    let device_identity = directory_identity(&expected.device_root)?;
    let control_identity = directory_identity(parent)?;
    let lock = lock_selection_at(&expected.store)?;
    let lock_identity = directory_identity(&parent.join("app_paths.selection.lock"))?;
    if lock_identity.is_none() {
        return Err(conflict());
    }
    let guard = SelectedSourceGuard {
        snapshot: expected.clone(),
        source_identity,
        device_identity,
        control_identity,
        lock_identity,
        _lock: lock,
    };
    guard.verify_unchanged()?;
    Ok(guard)
}

/// Recover only an original-owner-authorized, already selected v2 cleanup.
/// Ordinary preparation and legacy setters never invoke this operation.
pub(crate) fn recover_selection_lock_at(
    expected: &SelectionSnapshot,
    permit: &crate::secrets::protection::generation::GenerationRecoveryPermit<'_>,
) -> Result<SelectionLock, AppError> {
    if !matches!(&expected.selected, Some(SelectedScope::Generation(_)))
        || expected.source_root_id() != Some(permit.selected_root_id())
    {
        return Err(invalid());
    }
    permit.verify_selection(expected)?;
    verify_snapshot(expected)?;
    let parent = expected.store.parent().ok_or_else(invalid)?;
    validate_directory(parent)?;
    // Existing owner proves PID death and pins directory/file identity + bytes.
    // Unknown, live, aliased and legacy UUID-file locks remain untouched. Its
    // unsupported-platform liveness result is deliberately fail-closed.
    let guard = crate::zcode_file_lock::FileLock::acquire_recoverable(
        &parent.join("app_paths.selection"),
        std::time::Duration::from_millis(200),
    )?;
    verify_snapshot(expected)?;
    permit.verify_selection(expected)?;
    Ok(SelectionLock { _guard: guard })
}

/// Nonsecret selector preimage captured together with startup's old-root
/// eligibility. Fields are private so callers cannot forge a refreshed preimage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionSnapshot {
    store: PathBuf,
    bytes: Option<Vec<u8>>,
    root: PathBuf,
    device_root: PathBuf,
    selected: Option<SelectedScope>,
    digest: String,
    identity_digest: Option<String>,
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn capture_legacy_selection_at(
    store: &Path,
    default_root: &Path,
) -> Result<SelectionSnapshot, AppError> {
    let (bytes, document) = read_document(store)?;
    if document.contains_key(SELECTION_KEY) {
        return Err(AppError::Config(
            "config.isolated_scope_already_selected".into(),
        ));
    }
    validate_legacy_document(store, &document)?;
    Ok(SelectionSnapshot {
        store: store.to_path_buf(),
        digest: selection_digest(bytes.as_deref()),
        identity_digest: None,
        bytes,
        device_root: default_root.to_path_buf(),
        selected: None,
        root: document
            .get("app_config_dir_override")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(super::resolve_path)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| default_root.to_path_buf()),
    })
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct LegacyPendingScope {
    root: PathBuf,
    store: PathBuf,
    previous: Option<Vec<u8>>,
    scope_id: String,
    preserved_root: PathBuf,
    preserved_device_root: PathBuf,
    inspection_id: String,
    _lock: SelectionLock,
}
#[cfg(any(test, feature = "test-hooks"))]
impl LegacyPendingScope {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Startup must authenticate the supplied key through its vault owner before
    /// this call. IDs alone are not proof of key availability; this path owner
    /// neither generates keys nor chooses how user-supplied keys are retained.
    /// Any error retains the candidate. In particular, after an ambiguous
    /// publication callers must reread selection, never create a third vault.
    pub(crate) fn publish(self, vault_id: &str, key_id: &str) -> Result<IsolatedScope, AppError> {
        self.publish_inner(vault_id, key_id, |_| Ok(()))
    }

    fn publish_inner(
        self,
        vault_id: &str,
        key_id: &str,
        mut boundary: impl FnMut(PublishBoundary) -> Result<(), AppError>,
    ) -> Result<IsolatedScope, AppError> {
        let scope = IsolatedScope {
            format_version: 1,
            scope_id: self.scope_id.clone(),
            root: self.root.clone(),
            preserved_root: self.preserved_root.clone(),
            preserved_device_root: self.preserved_device_root.clone(),
            inspection_id: self.inspection_id.clone(),
            vault_id: vault_id.into(),
            key_id: key_id.into(),
        };
        scope.validate_shape(&self.store)?;
        validate_vault(&scope)?;
        let marker = serde_json::to_vec(&scope).map_err(|_| invalid())?;
        write_durable_new(&scope.root.join(IDENTITY_FILE), &marker)?;
        validate_selected(&self.store, &scope)?;
        boundary(PublishBoundary::IdentityDurable)?;
        if read_document(&self.store)?.0 != self.previous {
            return Err(conflict());
        }
        let mut document = raw_document(self.previous.as_deref())?;
        document.insert(
            SELECTION_KEY.into(),
            serde_json::value::RawValue::from_string(
                serde_json::to_string(&scope).map_err(|_| invalid())?,
            )
            .map_err(|_| invalid())?,
        );
        let bytes = serde_json::to_vec(&document).map_err(|_| invalid())?;
        let staged = stage_write(&self.store, &bytes, Some(0o600), true)?;
        // Check the actual durable staging file; a mismatched read never publishes.
        if read_regular_file(staged.tmp_path(), MAX_CONTROL_BYTES)
            .map_err(|e| AppError::io(staged.tmp_path(), e))?
            .as_deref()
            != Some(bytes.as_slice())
        {
            return Err(invalid());
        }
        boundary(PublishBoundary::SelectorStaged)?;
        if read_document(&self.store)?.0 != self.previous {
            return Err(conflict());
        }
        validate_selected(&self.store, &scope)?;
        staged.commit()?;
        // From this point even an error means the selected scope remains selected.
        // Do not roll back or discard its directory/key as an error cleanup.
        boundary(PublishBoundary::SelectorPublished)?;
        sync_directory(self.store.parent().ok_or_else(invalid)?)?;
        if read_scope_at(&self.store)? != Some(scope.clone()) {
            return Err(conflict());
        }
        Ok(scope)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PublishBoundary {
    IdentityDurable,
    SelectorStaged,
    SelectorPublished,
    RuntimeRetired,
}

/// None means no explicit isolated selection. A malformed selector, missing
/// selected root, identity mismatch, and all read failures are errors, NOT None.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn read_scope_at(store: &Path) -> Result<Option<IsolatedScope>, AppError> {
    let (_, document) = read_document(store)?;
    let Some(value) = document.get(SELECTION_KEY) else {
        return Ok(None);
    };
    let scope: IsolatedScope = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    validate_selected(store, &scope)?;
    Ok(Some(scope))
}

/// Only creates a fresh private candidate next to the control store, never under
/// either preserved root. Does NOT open Database, create a vault, or publish.
/// `inspection` is the original startup snapshot's stable ID, not availability.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn prepare_scope_at(
    store: &Path,
    old: &Path,
    device: &Path,
    inspection: &str,
    expected: &SelectionSnapshot,
) -> Result<LegacyPendingScope, AppError> {
    if store != expected.store || old != expected.root {
        return Err(conflict());
    }
    validate_directory(old)?;
    validate_existing_ancestors(device)?;
    if !valid_inspection(inspection) {
        return Err(invalid());
    }
    let parent = store.parent().ok_or_else(invalid)?;
    validate_existing_ancestors(parent)?;
    let container = parent.join("isolated-configs");
    if overlap(&container, old)
        || overlap(&container, device)
        || overlap(parent, old)
        || overlap(parent, device)
        || overlap_by_identity_with(parent, old, directory_identity)?
        || overlap_by_identity_with(parent, device, directory_identity)?
    {
        return Err(invalid());
    }
    // Read before locking so an existing selection is a wholly read-only denial.
    let (previous, document) = read_document(store)?;
    if previous != expected.bytes {
        return Err(conflict());
    }
    if document.contains_key(SELECTION_KEY) {
        return Err(AppError::Config(
            "config.isolated_scope_already_selected".into(),
        ));
    }
    create_private_directories(parent)?;
    let lock = lock_selection_at(store)?;
    if read_document(store)?.0 != previous {
        return Err(conflict());
    }
    if container.exists() {
        validate_directory(&container)?;
        if fs::read_dir(&container)
            .map_err(|e| AppError::io(&container, e))?
            .next()
            .transpose()
            .map_err(|e| AppError::io(&container, e))?
            .is_some()
        {
            return Err(AppError::Config("config.isolated_scope_pending".into()));
        }
    }
    crate::config_file_io::ensure_private_directory(&container)?;
    let scope_id = uuid::Uuid::new_v4().to_string();
    let root = container.join(&scope_id);
    // create_dir, not create_dir_all: refuse any pre-existing destination.
    fs::create_dir(&root).map_err(|e| AppError::io(&root, e))?;
    crate::config_file_io::ensure_private_directory(&root)?;
    sync_directory(&container)?;
    Ok(LegacyPendingScope {
        root,
        store: store.to_path_buf(),
        previous,
        scope_id,
        preserved_root: old.to_path_buf(),
        preserved_device_root: device.to_path_buf(),
        inspection_id: inspection.into(),
        _lock: lock,
    })
}

pub(crate) fn write_legacy_override_at(store: &Path, path: Option<&str>) -> Result<(), AppError> {
    if let Some(path) = path.map(str::trim).filter(|p| !p.is_empty()) {
        validate_legacy_root_at(store, &super::resolve_path(path))?;
    }
    let (previous, _) = read_document(store)?;
    let mut document = raw_document(previous.as_deref())?;
    if document.contains_key(SELECTION_KEY) {
        return Err(AppError::Config(
            "config.isolated_scope_already_selected".into(),
        ));
    }
    create_private_directories(store.parent().ok_or_else(invalid)?)?;
    let _lock = lock_selection_at(store)?;
    if read_document(store)?.0 != previous {
        return Err(conflict());
    }
    match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => {
            document.insert(
                "app_config_dir_override".into(),
                serde_json::value::RawValue::from_string(
                    serde_json::to_string(path).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?,
            );
        }
        None => {
            document.remove("app_config_dir_override");
        }
    }
    let bytes = serde_json::to_vec(&document).map_err(|_| invalid())?;
    let staged = stage_write(store, &bytes, Some(0o600), true)?;
    if read_document(store)?.0 != previous {
        return Err(conflict());
    }
    staged.commit()?;
    sync_directory(store.parent().ok_or_else(invalid)?)
}

fn validate_legacy_document(store: &Path, document: &Map<String, Value>) -> Result<(), AppError> {
    if let Some(raw) = document
        .get("app_config_dir_override")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        validate_legacy_root_at(store, &super::resolve_path(raw))?;
    }
    Ok(())
}
/// The managed namespace can be admitted only by an explicit committed selector,
/// never by the old directory override (including its symlink/case aliases).
/// Ordinary legacy file paths and aliases outside this namespace retain their
/// previous behavior; this is not a new directory-validity policy for legacy.
pub(crate) fn validate_legacy_root_at(store: &Path, path: &Path) -> Result<(), AppError> {
    let container =
        canonical_existing_prefix(&store.parent().ok_or_else(invalid)?.join("isolated-configs"))?;
    let target = canonical_existing_prefix(path)?;
    if target.starts_with(&container) {
        return Err(invalid());
    }
    if let Some(identity) = directory_identity(&container)? {
        for ancestor in target.ancestors() {
            // The old GUI accepts existing files. Compare directory ancestors,
            // not that leaf, and retain its original later initialization error.
            if ancestor.is_dir() && directory_identity(ancestor)? == Some(identity) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
fn canonical_existing_prefix(path: &Path) -> Result<PathBuf, AppError> {
    let mut current = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| AppError::io(path, e))?
            .join(path)
    };
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut prefix) => {
                for component in suffix.iter().rev() {
                    prefix.push(component);
                }
                return Ok(prefix);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                let component = current
                    .components()
                    .next_back()
                    .ok_or_else(invalid)?
                    .as_os_str()
                    .to_os_string();
                suffix.push(component);
                if !current.pop() {
                    return Err(invalid());
                }
            }
            Err(error) => return Err(AppError::io(&current, error)),
        }
    }
}

type ControlDocument = (Option<Vec<u8>>, Map<String, Value>);

fn read_document(store: &Path) -> Result<ControlDocument, AppError> {
    // Missing control parent is normal before first use; an existing ancestor
    // alias or non-directory is not. Never create anything on this read path.
    validate_existing_ancestors(store.parent().ok_or_else(invalid)?)?;
    let raw = read_regular_file(store, MAX_CONTROL_BYTES).map_err(|e| AppError::io(store, e))?;
    let Some(bytes) = &raw else {
        return Ok((None, Map::new()));
    };
    // Reject duplicate keys rather than guessing which spelling is authoritative.
    raw_document(Some(bytes))?;
    let value: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let document = value.as_object().cloned().ok_or_else(invalid)?;
    Ok((raw, document))
}

// Raw JSON values preserve unknown numbers/objects exactly; serde_json::Value
// would round large integers and precise decimals through f64 on reserialization.
fn raw_document(
    raw: Option<&[u8]>,
) -> Result<std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>, AppError> {
    struct RawObject(std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>);
    impl<'de> Deserialize<'de> for RawObject {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = RawObject;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("an object with unique keys")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut values = std::collections::BTreeMap::new();
                    while let Some((key, value)) =
                        map.next_entry::<String, Box<serde_json::value::RawValue>>()?
                    {
                        if values.insert(key, value).is_some() {
                            return Err(serde::de::Error::custom("duplicate control key"));
                        }
                    }
                    Ok(RawObject(values))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }
    match raw {
        Some(raw) => serde_json::from_slice::<RawObject>(raw)
            .map(|r| r.0)
            .map_err(|_| invalid()),
        None => Ok(std::collections::BTreeMap::new()),
    }
}

impl IsolatedScope {
    fn validate_shape(&self, store: &Path) -> Result<(), AppError> {
        if self.format_version != 1
            || !valid_uuid(&self.scope_id)
            || !valid_uuid(&self.vault_id)
            || !valid_uuid(&self.key_id)
            || !valid_inspection(&self.inspection_id)
        {
            return Err(invalid());
        }
        for path in [
            &self.root,
            &self.preserved_root,
            &self.preserved_device_root,
        ] {
            validate_absolute(path)?;
        }
        let parent = store.parent().ok_or_else(invalid)?;
        if self.root != parent.join("isolated-configs").join(&self.scope_id)
            || overlap(&self.root, &self.preserved_root)
            || overlap(&self.root, &self.preserved_device_root)
            || overlap_by_identity_with(&self.root, &self.preserved_root, directory_identity)?
            || overlap_by_identity_with(
                &self.root,
                &self.preserved_device_root,
                directory_identity,
            )?
        {
            return Err(invalid());
        }
        Ok(())
    }
}
fn validate_selected(store: &Path, scope: &IsolatedScope) -> Result<(), AppError> {
    scope.validate_shape(store)?;
    validate_directory(&scope.root)?;
    let marker = read_regular_file(&scope.root.join(IDENTITY_FILE), MAX_CONTROL_BYTES)
        .map_err(|e| AppError::io(scope.root.join(IDENTITY_FILE), e))?
        .ok_or_else(invalid)?;
    let identity: IsolatedScope = serde_json::from_slice(&marker).map_err(|_| invalid())?;
    if identity != *scope {
        return Err(invalid());
    }
    validate_vault(scope)
}
fn validate_vault(scope: &IsolatedScope) -> Result<(), AppError> {
    validate_directory(&scope.root)?;
    let path = scope.root.join("vault.json");
    let bytes = read_regular_file(&path, MAX_CONTROL_BYTES)
        .map_err(|e| AppError::io(&path, e))?
        .ok_or_else(invalid)?;
    let vault: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if vault.pointer("/metadata/vaultId").and_then(Value::as_str) != Some(&scope.vault_id)
        || vault.pointer("/metadata/keyId").and_then(Value::as_str) != Some(&scope.key_id)
    {
        return Err(invalid());
    }
    // The original session validates/authenticates complete vault metadata. This
    // owner only binds paths and identities; it does not implement cryptography.
    Ok(())
}
fn valid_uuid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| !u.is_nil() && u.to_string() == id)
}
fn valid_inspection(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryIdentity {
    volume: u64,
    index: u64,
}
fn overlap_by_identity_with(
    a: &Path,
    b: &Path,
    identity: impl Fn(&Path) -> Result<Option<DirectoryIdentity>, AppError>,
) -> Result<bool, AppError> {
    if overlap(a, b) {
        return Ok(true);
    }
    for (target, other) in [(a, b), (b, a)] {
        if let Some(target_id) = identity(target)? {
            for ancestor in other.ancestors() {
                if identity(ancestor)? == Some(target_id) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn directory_identity(path: &Path) -> Result<Option<DirectoryIdentity>, AppError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::io(path, e)),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(DirectoryIdentity {
            volume: metadata.dev(),
            index: metadata.ino(),
        }))
    }
    #[cfg(windows)]
    {
        use std::os::windows::{
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawHandle,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT,
        };
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| AppError::io(path, e))?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the valid directory handle and initialized output live through
        // the call; identities are compared only within this filesystem check.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(AppError::io(path, std::io::Error::last_os_error()));
        }
        Ok(Some(DirectoryIdentity {
            volume: u64::from(info.dwVolumeSerialNumber),
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        }))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(invalid())
    }
}

fn overlap(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
fn validate_absolute(path: &Path) -> Result<(), AppError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid());
    }
    Ok(())
}
fn validate_existing_ancestors(path: &Path) -> Result<(), AppError> {
    validate_absolute(path)?;
    let mut current = PathBuf::new();
    for c in path.components() {
        current.push(c);
        // A Windows drive/verbatim prefix is not yet a filesystem root (for
        // example `\\?\C:`). Probe only after the following RootDir is present.
        if matches!(c, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err(invalid());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(invalid());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(AppError::io(&current, e)),
        }
    }
    Ok(())
}
fn validate_directory(path: &Path) -> Result<(), AppError> {
    validate_existing_ancestors(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| AppError::io(path, e))?;
    if !meta.is_dir() {
        return Err(invalid());
    }
    Ok(())
}
fn create_private_directories(path: &Path) -> Result<(), AppError> {
    validate_existing_ancestors(path)?;
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        if matches!(part, Component::Prefix(_)) {
            continue;
        }
        if !current.exists() {
            crate::config_file_io::ensure_private_directory(&current)?;
        }
    }
    validate_directory(path)
}
fn sync_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| AppError::io(path, e))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Nonsecret path binding, constructible only by the selector owner. It conveys
/// no authority to decrypt, choose protection, or replace source credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CandidateBinding {
    scope: GenerationScope,
}
impl CandidateBinding {
    pub(crate) fn root_id(&self) -> &str {
        &self.scope.root_id
    }
    pub(crate) fn root(&self) -> &Path {
        &self.scope.root
    }
    pub(crate) fn source_data_root(&self) -> &Path {
        &self.scope.source.data_root
    }
    pub(crate) fn source_device_root(&self) -> &Path {
        &self.scope.source.device_root
    }
    pub(crate) fn source_root_id(&self) -> Option<&str> {
        self.scope.source.root_id.as_deref()
    }
    pub(crate) fn source_identity_digest(&self) -> Option<&str> {
        self.scope.source.identity_digest.as_deref()
    }
    pub(crate) fn source_selection_digest(&self) -> &str {
        &self.scope.source.selection_digest
    }
    pub(crate) fn inspection_token(&self) -> &str {
        &self.scope.source.inspection_token
    }
}
impl SelectionSnapshot {
    /// Immutable v2 binding captured with this selector snapshot. This does not
    /// re-read storage or authenticate a cleanup intent, target key, or inventory.
    /// Ordinary legacy and v1 selections have no generation binding.
    pub(crate) fn selected_generation_binding(&self) -> Option<CandidateBinding> {
        match self.selected.as_ref() {
            Some(SelectedScope::Generation(scope)) => Some(CandidateBinding {
                scope: scope.clone(),
            }),
            _ => None,
        }
    }
    pub(crate) fn source_data_root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn source_device_root(&self) -> &Path {
        &self.device_root
    }
    pub(crate) fn source_root_id(&self) -> Option<&str> {
        self.selected.as_ref().map(SelectedScope::root_id)
    }
    pub(crate) fn selection_digest(&self) -> &str {
        &self.digest
    }
}
fn selection_digest(bytes: Option<&[u8]>) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"loongport.selector-preimage.v2\0");
    digest.update([u8::from(bytes.is_some())]);
    if let Some(bytes) = bytes {
        digest.update(bytes);
    }
    digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn identity_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn capture_selection_at(
    store: &Path,
    default_data_root: &Path,
    legacy_device_root: &Path,
) -> Result<SelectionSnapshot, AppError> {
    let (bytes, document) = read_document(store)?;
    let selected = parse_active_scope(store, &document)?;
    let (root, device_root, marker_digest) = match &selected {
        Some(scope) => {
            let path = scope.root().join(IDENTITY_FILE);
            let marker = read_regular_file(&path, MAX_CONTROL_BYTES)
                .map_err(|e| AppError::io(&path, e))?
                .ok_or_else(invalid)?;
            (
                scope.root().to_path_buf(),
                scope.root().to_path_buf(),
                Some(identity_digest(&marker)),
            )
        }
        None => {
            let root = document
                .get("app_config_dir_override")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(super::resolve_path)
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| default_data_root.to_path_buf());
            validate_legacy_root_at(store, &root)?;
            validate_legacy_root_at(store, legacy_device_root)?;
            (root, legacy_device_root.to_path_buf(), None)
        }
    };
    validate_directory(&root)?;
    validate_existing_ancestors(&device_root)?;
    Ok(SelectionSnapshot {
        store: store.to_path_buf(),
        digest: selection_digest(bytes.as_deref()),
        bytes,
        root,
        device_root,
        selected,
        identity_digest: marker_digest,
    })
}

// A closed, private adapter: only these concrete original-owner capabilities
// can reach shared path/CAS code. No public generic trait, bool or ID constructor
// can turn separate configuration into an authenticated source conversion.
enum PreparationAuthority<'borrow, 'owner> {
    Conversion(
        &'borrow crate::secrets::protection::generation::GenerationPreparationPermit<'owner>,
    ),
    Separate(&'borrow crate::secrets::protection::generation::SeparateConfigurationPermit<'owner>),
}
impl PreparationAuthority<'_, '_> {
    fn verify_selection(&self, snapshot: &SelectionSnapshot) -> Result<(), AppError> {
        match self {
            Self::Conversion(p) => p.verify_selection(snapshot),
            Self::Separate(p) => p.verify_selection(snapshot),
        }
    }
    fn source_data_root(&self) -> &Path {
        match self {
            Self::Conversion(p) => p.source_data_root(),
            Self::Separate(p) => p.source_data_root(),
        }
    }
    fn source_device_root(&self) -> &Path {
        match self {
            Self::Conversion(p) => p.source_device_root(),
            Self::Separate(p) => p.source_device_root(),
        }
    }
    fn inspection_token(&self) -> &str {
        match self {
            Self::Conversion(p) => p.inspection_token(),
            Self::Separate(p) => p.inspection_token(),
        }
    }
}
enum PublicationAuthority<'a> {
    Conversion(Box<crate::secrets::protection::generation::VerifiedGeneration<'a>>),
    Separate(Box<crate::secrets::protection::generation::VerifiedSeparateConfiguration<'a>>),
}
impl PublicationAuthority<'_> {
    fn binding(&self) -> &CandidateBinding {
        match self {
            Self::Conversion(p) => p.binding(),
            Self::Separate(p) => p.binding(),
        }
    }
    fn begin_publication(&mut self, binding: &CandidateBinding) -> Result<(), AppError> {
        match self {
            Self::Conversion(p) => p.begin_publication(binding),
            Self::Separate(p) => p.begin_publication(binding),
        }
    }
    fn complete_publication(&mut self, binding: &CandidateBinding) -> Result<(), AppError> {
        match self {
            Self::Conversion(p) => p.complete_publication(binding),
            Self::Separate(p) => p.complete_publication(binding),
        }
    }
}

pub(crate) struct PendingScope {
    binding: CandidateBinding,
    store: PathBuf,
    previous: Option<Vec<u8>>,
    _lock: SelectionLock,
}
/// NotPublished means begin_publication was never entered. Once it is entered,
/// errors are conservative PossiblyPublished, even if the last CAS did not run:
/// the source runtime may already be retired and must never be automatically
/// unblocked. Only RestartRequired confirms the owner's completion callback also
/// succeeded. Separate configuration never authorizes cleanup of original data.
#[derive(Debug)]
pub(crate) enum PublicationOutcome {
    NotPublished(AppError),
    RestartRequired(GenerationScope),
    PossiblyPublished(AppError),
}
impl PendingScope {
    pub(crate) fn binding(&self) -> &CandidateBinding {
        &self.binding
    }
    pub(crate) fn root(&self) -> &Path {
        self.binding.root()
    }
    pub(crate) fn publish(
        self,
        proof: crate::secrets::protection::generation::VerifiedGeneration<'_>,
    ) -> PublicationOutcome {
        #[cfg(test)]
        {
            self.publish_at_boundary(proof, |_| Ok(()))
        }
        #[cfg(not(test))]
        {
            self.publish_inner(
                PublicationAuthority::Conversion(Box::new(proof)),
                |_| Ok(()),
            )
        }
    }
    pub(crate) fn publish_separate(
        self,
        proof: crate::secrets::protection::generation::VerifiedSeparateConfiguration<'_>,
    ) -> PublicationOutcome {
        self.publish_inner(PublicationAuthority::Separate(Box::new(proof)), |_| Ok(()))
    }
    #[cfg(test)]
    pub(crate) fn publish_at_boundary(
        self,
        proof: crate::secrets::protection::generation::VerifiedGeneration<'_>,
        boundary: impl FnMut(PublishBoundary) -> Result<(), AppError>,
    ) -> PublicationOutcome {
        self.publish_inner(PublicationAuthority::Conversion(Box::new(proof)), boundary)
    }
    fn publish_inner(
        self,
        mut proof: PublicationAuthority<'_>,
        mut boundary: impl FnMut(PublishBoundary) -> Result<(), AppError>,
    ) -> PublicationOutcome {
        let mut publication_begun = false;
        let result = (|| {
            if proof.binding() != &self.binding {
                return Err(conflict());
            }
            let scope = &self.binding.scope;
            validate_generation_selected(&self.store, scope)?;
            verify_source_identity(&self.binding)?;
            boundary(PublishBoundary::IdentityDurable)?;
            if read_document(&self.store)?.0 != self.previous {
                return Err(conflict());
            }
            let mut document = raw_document(self.previous.as_deref())?;
            document.insert(
                SELECTION_KEY.into(),
                serde_json::value::RawValue::from_string(
                    serde_json::to_string(scope).map_err(|_| invalid())?,
                )
                .map_err(|_| invalid())?,
            );
            let bytes = serde_json::to_vec(&document).map_err(|_| invalid())?;
            let staged = stage_write(&self.store, &bytes, Some(0o600), true)?;
            if read_regular_file(staged.tmp_path(), MAX_CONTROL_BYTES)
                .map_err(|e| AppError::io(staged.tmp_path(), e))?
                .as_deref()
                != Some(bytes.as_slice())
            {
                return Err(invalid());
            }
            boundary(PublishBoundary::SelectorStaged)?;
            if read_document(&self.store)?.0 != self.previous {
                return Err(conflict());
            }
            validate_generation_selected(&self.store, scope)?;
            verify_source_identity(&self.binding)?;
            // The original owner revalidates complete source+target inventories
            // under its session and database guards, then retires source runtime.
            // We do not unblock it on any outcome, including a precommit failure.
            publication_begun = true;
            proof.begin_publication(&self.binding)?;
            boundary(PublishBoundary::RuntimeRetired)?;
            if read_document(&self.store)?.0 != self.previous {
                return Err(conflict());
            }
            validate_generation_selected(&self.store, scope)?;
            verify_source_identity(&self.binding)?;
            staged.commit()?;
            boundary(PublishBoundary::SelectorPublished)?;
            sync_directory(self.store.parent().ok_or_else(invalid)?)?;
            if read_document(&self.store)?.0.as_deref() != Some(bytes.as_slice())
                || read_active_scope_at(&self.store)?
                    != Some(SelectedScope::Generation(scope.clone()))
            {
                return Err(conflict());
            }
            proof.complete_publication(&self.binding)?;
            if read_document(&self.store)?.0.as_deref() != Some(bytes.as_slice()) {
                return Err(conflict());
            }
            Ok(scope.clone())
        })();
        match result {
            Ok(scope) => PublicationOutcome::RestartRequired(scope),
            Err(error) if publication_begun => PublicationOutcome::PossiblyPublished(error),
            Err(error) => PublicationOutcome::NotPublished(error),
        }
    }
}
/// A distinct original-owner authorization for explicit separate configuration;
/// it is never an authenticated conversion of unread original accounts.
pub(crate) fn prepare_separate_configuration_at(
    expected: &SelectionSnapshot,
    permit: &crate::secrets::protection::generation::SeparateConfigurationPermit<'_>,
) -> Result<PendingScope, AppError> {
    prepare_candidate_at(expected, PreparationAuthority::Separate(permit))
}

pub(crate) fn prepare_generation_at(
    expected: &SelectionSnapshot,
    permit: &crate::secrets::protection::generation::GenerationPreparationPermit<'_>,
) -> Result<PendingScope, AppError> {
    prepare_candidate_at(expected, PreparationAuthority::Conversion(permit))
}
fn prepare_candidate_at(
    expected: &SelectionSnapshot,
    permit: PreparationAuthority<'_, '_>,
) -> Result<PendingScope, AppError> {
    permit.verify_selection(expected)?;
    if permit.source_data_root() != expected.source_data_root()
        || permit.source_device_root() != expected.source_device_root()
    {
        return Err(conflict());
    }
    if !valid_inspection(permit.inspection_token()) {
        return Err(invalid());
    }
    let store = &expected.store;
    let parent = store.parent().ok_or_else(invalid)?;
    validate_existing_ancestors(parent)?;
    validate_directory(&expected.root)?;
    validate_existing_ancestors(&expected.device_root)?;
    let container = parent.join("isolated-configs");
    // Check actual write targets, not a symmetric parent/child overlap: a
    // legitimate next generation is a sibling of the selected source root.
    for target in [parent, container.as_path()] {
        for preserved in [&expected.root, &expected.device_root] {
            if within_by_identity(target, preserved)? {
                return Err(invalid());
            }
        }
    }
    verify_snapshot(expected)?;
    validate_container_history(expected, &container)?;
    create_private_directories(parent)?;
    let lock = lock_selection_at(store)?;
    verify_snapshot(expected)?;
    validate_container_history(expected, &container)?;
    // Recheck original owner authority under the same selector lock retained by
    // PendingScope, before creating any container/candidate or durable marker.
    permit.verify_selection(expected)?;
    if !container.exists() {
        crate::config_file_io::ensure_private_directory(&container)?;
    }
    let root_id = uuid::Uuid::new_v4().to_string();
    let root = container.join(&root_id);
    let scope = GenerationScope {
        format_version: 2,
        root_id,
        root: root.clone(),
        source: SourceProvenance {
            data_root: expected.root.clone(),
            device_root: expected.device_root.clone(),
            root_id: expected.source_root_id().map(str::to_owned),
            identity_digest: expected.identity_digest.clone(),
            selection_digest: expected.digest.clone(),
            inspection_token: permit.inspection_token().to_owned(),
        },
    };
    scope.validate_shape(store)?;
    #[cfg(unix)]
    let mut builder = fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root).map_err(|e| AppError::io(&root, e))?;
    crate::config_file_io::ensure_private_directory(&root)?;
    // Nonsecret marker is durable before inventory verification. Its presence
    // alone never means committed: only reachability from the selected head does.
    write_durable_new(
        &root.join(IDENTITY_FILE),
        &serde_json::to_vec(&scope).map_err(|_| invalid())?,
    )?;
    sync_directory(&root)?;
    sync_directory(&container)?;
    Ok(PendingScope {
        binding: CandidateBinding { scope },
        store: store.clone(),
        previous: expected.bytes.clone(),
        _lock: lock,
    })
}
/// Read only: a marker identifies a candidate, never permission to resume it.
pub(crate) fn inspect_pending_generation_at(
    expected: &SelectionSnapshot,
) -> Result<Option<CandidateBinding>, AppError> {
    verify_snapshot(expected)?;
    let container = expected
        .store
        .parent()
        .ok_or_else(invalid)?
        .join("isolated-configs");
    let committed = committed_generation_ids(expected, &container)?;
    if !container.exists() {
        return if committed.is_empty() {
            Ok(None)
        } else {
            Err(invalid())
        };
    }
    validate_directory(&container)?;
    let mut seen = std::collections::BTreeSet::new();
    let mut candidate = None;
    for entry in fs::read_dir(&container).map_err(|e| AppError::io(&container, e))? {
        let entry = entry.map_err(|e| AppError::io(&container, e))?;
        validate_directory(&entry.path())?;
        let name = entry.file_name().into_string().map_err(|_| invalid())?;
        if committed.contains(&name) {
            seen.insert(name);
            continue;
        }
        // Anything other than exactly one fully source-bound v2 marker remains
        // unknown pending. Never guess, remove it or mint another generation.
        if candidate.is_some() {
            return Err(invalid());
        }
        let marker = entry.path().join(IDENTITY_FILE);
        let bytes = read_regular_file(&marker, MAX_CONTROL_BYTES)
            .map_err(|e| AppError::io(&marker, e))?
            .ok_or_else(invalid)?;
        let scope: GenerationScope = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if scope.root != entry.path()
            || scope.root_id != name
            || scope.source.data_root != expected.root
            || scope.source.device_root != expected.device_root
            || scope.source.root_id.as_deref() != expected.source_root_id()
            || scope.source.identity_digest != expected.identity_digest
            || scope.source.selection_digest != expected.digest
        {
            return Err(conflict());
        }
        validate_generation_selected(&expected.store, &scope)?;
        candidate = Some(CandidateBinding { scope });
    }
    if seen != committed {
        return Err(invalid());
    }
    verify_snapshot(expected)?;
    Ok(candidate)
}

// Closed adapter: each public entry requires its own original-owner authority.
// In particular, explicit separate resumption does not authenticate old secrets
// or grant permission to convert or clean up the preserved source.
enum ResumeAuthority<'borrow, 'owner> {
    Conversion(&'borrow crate::secrets::protection::generation::GenerationResumePermit<'owner>),
    Separate(
        &'borrow crate::secrets::protection::generation::SeparateConfigurationResumePermit<'owner>,
    ),
}
impl ResumeAuthority<'_, '_> {
    fn binding(&self) -> &CandidateBinding {
        match self {
            Self::Conversion(permit) => permit.binding(),
            Self::Separate(permit) => permit.binding(),
        }
    }
    fn verify_selection(&self, expected: &SelectionSnapshot) -> Result<(), AppError> {
        match self {
            Self::Conversion(permit) => permit.verify_selection(expected),
            Self::Separate(permit) => permit.verify_selection(expected),
        }
    }
}

/// Resume only the same explicitly authorized separate candidate. Old-source
/// authentication/conversion is not implied; publication still needs its own
/// original-owner VerifiedSeparateConfiguration proof.
pub(crate) fn resume_separate_configuration_at(
    expected: &SelectionSnapshot,
    permit: &crate::secrets::protection::generation::SeparateConfigurationResumePermit<'_>,
) -> Result<PendingScope, AppError> {
    resume_candidate_at(expected, ResumeAuthority::Separate(permit))
}

/// Resume only the same candidate authenticated by the original owner. No new
/// root or marker, source mutation, cleanup or selector write occurs here.
pub(crate) fn resume_generation_at(
    expected: &SelectionSnapshot,
    permit: &crate::secrets::protection::generation::GenerationResumePermit<'_>,
) -> Result<PendingScope, AppError> {
    resume_candidate_at(expected, ResumeAuthority::Conversion(permit))
}

fn resume_candidate_at(
    expected: &SelectionSnapshot,
    permit: ResumeAuthority<'_, '_>,
) -> Result<PendingScope, AppError> {
    permit.verify_selection(expected)?;
    if inspect_pending_generation_at(expected)?.as_ref() != Some(permit.binding()) {
        return Err(conflict());
    }
    let parent = expected.store.parent().ok_or_else(invalid)?;
    validate_directory(parent)?;
    let guard = crate::zcode_file_lock::FileLock::acquire_recoverable(
        &parent.join("app_paths.selection"),
        std::time::Duration::from_millis(200),
    )?;
    if inspect_pending_generation_at(expected)?.as_ref() != Some(permit.binding()) {
        return Err(conflict());
    }
    permit.verify_selection(expected)?;
    Ok(PendingScope {
        binding: permit.binding().clone(),
        store: expected.store.clone(),
        previous: expected.bytes.clone(),
        _lock: SelectionLock { _guard: guard },
    })
}

fn within_by_identity(target: &Path, preserved: &Path) -> Result<bool, AppError> {
    if target.starts_with(preserved) {
        return Ok(true);
    }
    if let Some(identity) = directory_identity(preserved)? {
        for ancestor in target.ancestors() {
            if directory_identity(ancestor)? == Some(identity) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn verify_snapshot(expected: &SelectionSnapshot) -> Result<(), AppError> {
    let actual = capture_selection_at(&expected.store, &expected.root, &expected.device_root)?;
    if actual != *expected {
        return Err(conflict());
    }
    Ok(())
}
fn verify_source_identity(binding: &CandidateBinding) -> Result<(), AppError> {
    validate_directory(binding.source_data_root())?;
    validate_existing_ancestors(binding.source_device_root())?;
    if let Some(expected) = binding.source_identity_digest() {
        let path = binding.source_data_root().join(IDENTITY_FILE);
        let bytes = read_regular_file(&path, MAX_CONTROL_BYTES)
            .map_err(|e| AppError::io(&path, e))?
            .ok_or_else(invalid)?;
        if identity_digest(&bytes) != expected {
            return Err(conflict());
        }
    }
    Ok(())
}
fn committed_generation_ids(
    expected: &SelectionSnapshot,
    container: &Path,
) -> Result<std::collections::BTreeSet<String>, AppError> {
    use std::collections::BTreeSet;
    let mut committed = BTreeSet::new();
    let mut current = expected.selected.clone();
    while let Some(scope) = current {
        if !committed.insert(scope.root_id().to_owned()) {
            return Err(invalid());
        }
        match scope {
            SelectedScope::Legacy(scope) => {
                // v1 can only have been selected from a legacy root. Its vault
                // is not read here: retained archives may be sanitized by owner.
                scope.validate_shape(&expected.store)?;
                current = None;
            }
            SelectedScope::Generation(scope) => {
                scope.validate_shape(&expected.store)?;
                if let Some(id) = &scope.source.root_id {
                    let root = container.join(id);
                    validate_directory(&root)?;
                    let path = root.join(IDENTITY_FILE);
                    let bytes = read_regular_file(&path, MAX_CONTROL_BYTES)
                        .map_err(|e| AppError::io(&path, e))?
                        .ok_or_else(invalid)?;
                    if Some(identity_digest(&bytes)).as_deref()
                        != scope.source.identity_digest.as_deref()
                    {
                        return Err(conflict());
                    }
                    let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                    let predecessor = parse_scope_identity(&value)?;
                    if predecessor.root() != root || predecessor.root_id() != id {
                        return Err(invalid());
                    }
                    current = Some(predecessor);
                } else {
                    current = None;
                }
            }
        }
    }
    Ok(committed)
}
fn validate_container_history(
    expected: &SelectionSnapshot,
    container: &Path,
) -> Result<(), AppError> {
    use std::collections::BTreeSet;
    let committed = committed_generation_ids(expected, container)?;
    if container.exists() {
        validate_directory(container)?;
        let mut seen = BTreeSet::new();
        for entry in fs::read_dir(container).map_err(|e| AppError::io(container, e))? {
            let entry = entry.map_err(|e| AppError::io(container, e))?;
            let name = entry.file_name().into_string().map_err(|_| invalid())?;
            validate_directory(&entry.path())?;
            if !committed.contains(&name) {
                return Err(AppError::Config("config.isolated_scope_pending".into()));
            }
            seen.insert(name);
        }
        if seen != committed {
            return Err(invalid());
        }
    } else if !committed.is_empty() {
        return Err(invalid());
    }
    Ok(())
}

/// Path identity only. Credential protection has exactly one owner: protection.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenerationScope {
    format_version: u32,
    root_id: String,
    root: PathBuf,
    source: SourceProvenance,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceProvenance {
    data_root: PathBuf,
    device_root: PathBuf,
    root_id: Option<String>,
    identity_digest: Option<String>,
    selection_digest: String,
    inspection_token: String,
}
impl GenerationScope {
    pub(crate) fn root_id(&self) -> &str {
        &self.root_id
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    fn validate_shape(&self, store: &Path) -> Result<(), AppError> {
        if self.format_version != 2
            || !valid_uuid(&self.root_id)
            || !valid_inspection(&self.source.selection_digest)
            || !valid_inspection(&self.source.inspection_token)
            || self
                .source
                .root_id
                .as_deref()
                .is_some_and(|s| !valid_uuid(s))
            || self
                .source
                .identity_digest
                .as_deref()
                .is_some_and(|s| !valid_inspection(s))
            || self.source.root_id.is_some() != self.source.identity_digest.is_some()
        {
            return Err(invalid());
        }
        for path in [&self.root, &self.source.data_root, &self.source.device_root] {
            validate_absolute(path)?;
        }
        let container = store.parent().ok_or_else(invalid)?.join("isolated-configs");
        if self.root != container.join(&self.root_id) {
            return Err(invalid());
        }
        if let Some(source_id) = &self.source.root_id {
            if self.source.data_root != container.join(source_id)
                || self.source.device_root != self.source.data_root
            {
                return Err(invalid());
            }
        }
        for preserved in [&self.source.data_root, &self.source.device_root] {
            if overlap_by_identity_with(&self.root, preserved, directory_identity)? {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectedScope {
    Legacy(IsolatedScope),
    Generation(GenerationScope),
}
impl SelectedScope {
    fn root(&self) -> &Path {
        match self {
            Self::Legacy(s) => &s.root,
            Self::Generation(s) => &s.root,
        }
    }
    fn root_id(&self) -> &str {
        match self {
            Self::Legacy(s) => &s.scope_id,
            Self::Generation(s) => &s.root_id,
        }
    }
}
fn validate_generation_selected(store: &Path, scope: &GenerationScope) -> Result<(), AppError> {
    scope.validate_shape(store)?;
    validate_directory(&scope.root)?;
    let path = scope.root.join(IDENTITY_FILE);
    let bytes = read_regular_file(&path, MAX_CONTROL_BYTES)
        .map_err(|e| AppError::io(&path, e))?
        .ok_or_else(invalid)?;
    let marker: GenerationScope = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if marker != *scope {
        return Err(invalid());
    }
    Ok(())
}
fn parse_scope_identity(value: &Value) -> Result<SelectedScope, AppError> {
    match value.get("formatVersion").and_then(Value::as_u64) {
        Some(1) => serde_json::from_value(value.clone())
            .map(SelectedScope::Legacy)
            .map_err(|_| invalid()),
        Some(2) => serde_json::from_value(value.clone())
            .map(SelectedScope::Generation)
            .map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}
fn parse_active_scope(
    store: &Path,
    document: &Map<String, Value>,
) -> Result<Option<SelectedScope>, AppError> {
    let Some(value) = document.get(SELECTION_KEY) else {
        validate_legacy_document(store, document)?;
        return Ok(None);
    };
    let selected = parse_scope_identity(value)?;
    match &selected {
        SelectedScope::Legacy(scope) => validate_selected(store, scope)?,
        SelectedScope::Generation(scope) => validate_generation_selected(store, scope)?,
    }
    Ok(Some(selected))
}
fn read_active_scope_at(store: &Path) -> Result<Option<SelectedScope>, AppError> {
    let (_, document) = read_document(store)?;
    parse_active_scope(store, &document)
}

/// Frozen process selection: publication takes effect only after restart.
#[derive(Default)]
pub(crate) struct ScopeCache(std::sync::OnceLock<Result<Option<SelectedScope>, String>>);
impl ScopeCache {
    pub(crate) const fn new() -> Self {
        Self(std::sync::OnceLock::new())
    }
    pub(crate) fn initialize_optional_path(&self, store: Option<&Path>) -> Result<(), AppError> {
        self.0
            .get_or_init(|| {
                store
                    .ok_or_else(|| AppError::Config("config.store_path_unavailable".into()))
                    .and_then(read_active_scope_at)
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map(|_| ())
            .map_err(|e| AppError::Config(e.clone()))
    }
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn initialize_at(&self, store: &Path) -> Result<(), AppError> {
        self.initialize_optional_path(Some(store))
    }
    pub(crate) fn root(&self) -> Result<Option<PathBuf>, AppError> {
        self.0
            .get()
            .ok_or_else(|| AppError::Config("config.scope_not_initialized".into()))?
            .as_ref()
            .map(|s| s.as_ref().map(|s| s.root().to_path_buf()))
            .map_err(|e| AppError::Config(e.clone()))
    }
}

#[cfg(test)]
#[path = "isolated_scope_tests.rs"]
mod tests;
