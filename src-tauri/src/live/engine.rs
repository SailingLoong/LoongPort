//! Upstream live engine: plan, stage, first-write backup and per-app write lock.
//! LoongPort keeps device persistence encrypted under the existing session read
//! guard. No runtime caller or client path is registered by this module.
//! The final hash check and rename are still optimistic against external clients.

use sha2::{Digest, Sha256};

use super::patch::{LivePatch, LiveWriteError};

use crate::config_file_io::{self, StagedWrite};
use crate::error::AppError;
use crate::secrets::owned_file::{DeviceFile, DEVICE_BACKUP_DIR, DEVICE_STATE_FILE};
use crate::secrets::VaultContext;
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock, RwLockReadGuard};
use zeroize::Zeroizing;

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
// Authenticated envelopes contain nested base64, so allow encoding overhead.
const MAX_DEVICE_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
#[path = "planner_tests.rs"]
mod planner_tests;

/// Fixed device-local upstream store, independent of the configured/synced root.
/// Construction only resolves paths. Authenticated persistence requires the
/// caller's existing session read guard and validates every path component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceStore {
    root: PathBuf,
}

impl DeviceStore {
    pub(crate) fn for_device() -> Self {
        Self::at(crate::config::get_home_dir().join(crate::config::APP_DIR_NAME))
    }

    pub(crate) fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn state_path(&self) -> PathBuf {
        self.root.join(DEVICE_STATE_FILE)
    }

    pub(crate) fn first_write_backup_dir(&self) -> PathBuf {
        self.root.join(DEVICE_BACKUP_DIR)
    }

    pub(crate) fn path_for(&self, file: &DeviceFile) -> PathBuf {
        self.root.join(file.relative_path())
    }
}

/// 一个受引擎管理的客户端文件。`private` 为真时按 0600 写（文件里有 Key）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFile {
    pub path: PathBuf,
    pub private: bool,
}

impl LiveFile {
    pub fn private(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            private: true,
        }
    }

    pub fn shared(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            private: false,
        }
    }
}

/// 文件内容的 hash；文件不存在是 `None`。
pub fn digest(bytes: Option<&[u8]>) -> Option<String> {
    bytes.map(sha256_hex)
}

/// 十六进制的 SHA-256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 在内存里算好的一次写入。
#[derive(Debug, Clone)]
pub struct Planned {
    pub file: LiveFile,
    /// 写前内容的 hash，`None` 表示文件不存在。
    pub pre: Option<String>,
    pre_bytes: Option<Vec<u8>>,
    /// 写后内容的 hash；`None` 表示删掉这个文件。
    pub planned: Option<String>,
    bytes: Option<Vec<u8>>,
}

impl Planned {
    pub fn is_noop(&self) -> bool {
        self.pre == self.planned
    }

    pub fn pre_bytes(&self) -> Option<&[u8]> {
        self.pre_bytes.as_deref()
    }
}

/// Pure upstream planning from caller-captured bytes; this never reads or writes files.
pub(crate) fn plan_from(
    file: &LiveFile,
    patch: &dyn LivePatch,
    pre_bytes: Option<Vec<u8>>,
) -> Result<Planned, LiveWriteError> {
    let bytes = patch.apply_file(&file.path, pre_bytes.as_deref())?;
    Ok(Planned {
        file: file.clone(),
        pre: digest(pre_bytes.as_deref()),
        pre_bytes,
        planned: digest(bytes.as_deref()),
        bytes,
    })
}

#[cfg(test)]
mod device_path_tests {
    use super::*;
    use crate::secrets::owned_file::DeviceFile;

    #[test]
    #[serial_test::serial]
    fn device_store_uses_fixed_home_instead_of_configured_database_root() {
        let store = DeviceStore::for_device();
        assert_eq!(
            store.state_path(),
            crate::config::get_home_dir()
                .join(crate::config::APP_DIR_NAME)
                .join("live-state.json")
        );
        assert_eq!(
            store.state_path().parent().unwrap().file_name().unwrap(),
            ".loongport"
        );
    }

    #[test]
    fn typed_path_construction_does_not_create_any_storage() {
        let fixture = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let root = fixture.path().join("not-created");
        let store = DeviceStore::at(&root);
        let state = DeviceFile::registered("live-state.json").unwrap();
        assert_eq!(store.root(), root.as_path());
        assert_eq!(store.path_for(&state), root.join("live-state.json"));
        assert_eq!(store.state_path(), root.join("live-state.json"));
        assert_eq!(
            store.first_write_backup_dir(),
            root.join("backups/live-first-write")
        );
        assert!(!root.exists());
        assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 0);
    }
}

/// 读当前字节；文件不存在返回 `None`，其他读取错误照报。
pub fn read_current(path: &Path) -> Result<Option<Vec<u8>>, LiveWriteError> {
    validate_file_path(path).map_err(|_| LiveWriteError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(ErrorKind::InvalidInput, "live.invalid_path"),
    })?;
    match config_file_io::read_regular_file(path, MAX_FILE_BYTES) {
        Ok(bytes) => Ok(bytes),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LiveWriteError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// 读当前内容并在内存里算出新内容；什么都不写。
pub fn plan(file: &LiveFile, patch: &dyn LivePatch) -> Result<Planned, LiveWriteError> {
    let pre_bytes = read_current(&file.path)?;
    plan_from(file, patch, pre_bytes)
}

/// 把新内容写进目标旁边的临时文件（fsync 过，崩溃后可以靠它前滚）。要删文件时没有
/// 临时文件，返回 `None`。
pub fn stage(planned: &Planned) -> Result<Option<StagedWrite>, AppError> {
    let Some(bytes) = planned.bytes.as_deref() else {
        return Ok(None);
    };
    let mode = planned.file.private.then_some(0o600);
    validate_file_path(&planned.file.path)?;
    let write = config_file_io::stage_write(&planned.file.path, bytes, mode, true)?;
    if let Err(error) = sync_parent(write.tmp_path()) {
        let _ = fs::remove_file(write.tmp_path());
        return Err(error);
    }
    Ok(Some(write))
}

/// 一个应用的写锁：CC Switch 里改这个应用客户端文件的所有写入方（切换、编辑器、
/// 模式操作、托盘）都要先拿到它。不可重入。
#[derive(Debug)]
pub struct AppWriteGuard {
    app: String,
}

impl AppWriteGuard {
    pub fn app(&self) -> &str {
        &self.app
    }
}

fn lock_table() -> &'static (Mutex<HashSet<String>>, Condvar) {
    static LOCKS: OnceLock<(Mutex<HashSet<String>>, Condvar)> = OnceLock::new();
    LOCKS.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

pub fn lock_app(app: &str) -> AppWriteGuard {
    let (held, released) = lock_table();
    let mut held = held.lock().unwrap_or_else(|e| e.into_inner());
    while held.contains(app) {
        held = released.wait(held).unwrap_or_else(|e| e.into_inner());
    }
    held.insert(app.to_string());
    AppWriteGuard {
        app: app.to_string(),
    }
}

impl Drop for AppWriteGuard {
    fn drop(&mut self) {
        let (held, released) = lock_table();
        held.lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.app);
        released.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::patch::json::JsonPatch;
    use crate::live::patch::KeyPath;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn set_patch(key: &str, value: &str) -> JsonPatch {
        JsonPatch {
            set: vec![(KeyPath::new(&[key]), json!(value))],
            ..JsonPatch::default()
        }
    }

    #[test]
    fn plan_writes_nothing_and_refuses_broken_files() {
        let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{ broken").unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();

        let err = plan(&LiveFile::shared(&path), &set_patch("a", "b")).expect_err("refused");
        assert!(matches!(err, LiveWriteError::Parse { .. }));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ broken");
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp files"
        );
    }

    #[test]
    fn plan_reports_noops() {
        let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, "{\n  \"a\": \"b\"\n}").unwrap();
        let planned = plan(&LiveFile::shared(&path), &set_patch("a", "b")).unwrap();
        assert!(planned.is_noop());
        let planned = plan(&LiveFile::shared(&path), &set_patch("a", "c")).unwrap();
        assert!(!planned.is_noop());
    }

    #[cfg(unix)]
    #[test]
    fn private_files_are_staged_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = dir.path().join("settings.json");
        let planned = plan(&LiveFile::private(&path), &set_patch("a", "b")).unwrap();
        stage(&planned).unwrap().unwrap().commit().unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn app_lock_serializes_writers_of_the_same_app() {
        let guard = lock_app("lock-test-app");
        let entered = Arc::new(AtomicBool::new(false));
        let flag = entered.clone();
        let waiter = std::thread::spawn(move || {
            let _other = lock_app("lock-test-app");
            flag.store(true, Ordering::SeqCst);
        });
        let _unrelated = lock_app("lock-test-other-app");
        std::thread::sleep(Duration::from_millis(50));
        assert!(!entered.load(Ordering::SeqCst), "second writer must wait");
        drop(guard);
        waiter.join().unwrap();
        assert!(entered.load(Ordering::SeqCst));
    }
}

/// Reject path aliases and links before any read, stage, replay or cleanup. The
/// caller supplies an explicit file allowlist; persisted paths are never authority.
/// This is conservative path admission, not a claim of race-free external I/O.
pub(crate) fn validate_file_path(path: &Path) -> Result<(), AppError> {
    validate_path(path, false)
}

fn invalid_path() -> AppError {
    AppError::Config("live.invalid_path".into())
}

fn validate_path(path: &Path, directory: bool) -> Result<(), AppError> {
    validate_path_with_metadata(path, directory, |path| fs::symlink_metadata(path))
}

fn validate_path_with_metadata(
    path: &Path,
    directory: bool,
    mut metadata: impl FnMut(&Path) -> std::io::Result<fs::Metadata>,
) -> Result<(), AppError> {
    if !path.is_absolute()
        || path.to_str().is_none()
        || !path
            .components()
            .any(|part| matches!(part, Component::RootDir))
    {
        // Some Windows verbatim prefixes are classified as absolute even when
        // no RootDir is present. Skipping a Prefix must never admit such a path
        // without inspecting a complete root.
        return Err(invalid_path());
    }
    let mut walked = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::CurDir | Component::ParentDir) {
            return Err(invalid_path());
        }
        walked.push(part);
        // A Windows Prefix (for example \\?\C:) is not the complete root.
        // Keep it verbatim, then inspect after RootDir is joined; every root
        // and normal component still passes the same link/reparse/type checks.
        if let Component::Prefix(prefix) = part {
            // The client-file contract accepts disk/UNC paths, not arbitrary
            // Windows device namespaces. Skipping an incomplete prefix must
            // not broaden admission to those namespaces.
            if !matches!(
                prefix.kind(),
                std::path::Prefix::Disk(_)
                    | std::path::Prefix::VerbatimDisk(_)
                    | std::path::Prefix::UNC(_, _)
                    | std::path::Prefix::VerbatimUNC(_, _)
            ) {
                return Err(invalid_path());
            }
            continue;
        }
        match metadata(&walked) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(invalid_path());
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(invalid_path());
                    }
                }
                let last = walked == path;
                if !last || directory {
                    if !meta.is_dir() {
                        return Err(invalid_path());
                    }
                } else {
                    if !meta.is_file() {
                        return Err(invalid_path());
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if meta.nlink() != 1 {
                            return Err(invalid_path());
                        }
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(AppError::io(&walked, error)),
        }
    }
    if walked.as_os_str() != path.as_os_str() {
        return Err(invalid_path());
    }
    Ok(())
}

impl DeviceStore {
    /// Reads do not create a directory, initialize a vault, or repair bad state.
    pub(crate) fn read_device(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        file: &DeviceFile,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, AppError> {
        validate_path(&self.root, true)?;
        let path = self.path_for(file);
        validate_file_path(&path)?;
        config_file_io::read_regular_file(&path, MAX_DEVICE_BYTES)
            .map_err(|e| AppError::io(&path, e))?
            .map(|bytes| file.decode(vault, &bytes))
            .transpose()
    }

    fn prepare_device_parent(&self, file: &DeviceFile) -> Result<(), AppError> {
        validate_path(&self.root, true)?;
        config_file_io::ensure_private_directory(&self.root)?;
        let mut directory = self.root.clone();
        if let Some(parent) = file.relative_path().parent() {
            for part in parent.components() {
                directory.push(part);
                validate_path(&directory, true)?;
                config_file_io::ensure_private_directory(&directory)?;
            }
        }
        validate_file_path(&self.path_for(file))
    }

    pub(crate) fn write_device(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        file: &DeviceFile,
        plaintext: &[u8],
    ) -> Result<(), AppError> {
        self.prepare_device_parent(file)?;
        let bytes = file.encode(vault, plaintext)?;
        let path = self.path_for(file);
        config_file_io::stage_write(&path, &bytes, Some(0o600), true)?.commit()?;
        sync_parent(&path)
    }

    fn create_device(
        &self,
        vault: &RwLockReadGuard<'_, VaultContext>,
        file: &DeviceFile,
        plaintext: &[u8],
    ) -> Result<(), AppError> {
        self.prepare_device_parent(file)?;
        let bytes = file.encode(vault, plaintext)?;
        config_file_io::write_durable_new(&self.path_for(file), &bytes)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupSource {
    version: u32,
    path: PathBuf,
    digest: Option<String>,
}

/// Upstream first-write backup, with full path digest identity and authenticated
/// DeviceFile envelopes. The marker is durable only after the original bytes.
/// An orphaned backup is verified and reused, never replaced with later content.
pub(crate) fn ensure_first_write_backup(
    store: &DeviceStore,
    vault: &RwLockReadGuard<'_, VaultContext>,
    path: &Path,
    current: Option<&[u8]>,
) -> Result<(), AppError> {
    static BACKUP_LOCK: Mutex<()> = Mutex::new(());
    let _guard = BACKUP_LOCK
        .lock()
        .map_err(|_| AppError::Lock("live.backup_lock".into()))?;
    validate_file_path(path)?;
    let key = sha256_hex(path.to_str().ok_or_else(invalid_path)?.as_bytes());
    let backup = DeviceFile::registered(format!("{DEVICE_BACKUP_DIR}/{key}.backup"))?;
    let marker = DeviceFile::registered(format!("{DEVICE_BACKUP_DIR}/{key}.source"))?;
    let original = store.read_device(vault, &backup)?;
    if let Some(bytes) = store.read_device(vault, &marker)? {
        let source: BackupSource = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::Config("live.invalid_backup".into()))?;
        if source.version != 1
            || source.path != path
            || source.digest != digest(original.as_ref().map(|v| v.as_slice()))
        {
            return Err(AppError::Config("live.invalid_backup".into()));
        }
        return Ok(());
    }
    if let Some(original) = original {
        if Some(original.as_slice()) != current {
            return Err(AppError::Config("live.incomplete_backup".into()));
        }
    } else if let Some(bytes) = current {
        store.create_device(vault, &backup, bytes)?;
    }
    let source = BackupSource {
        version: 1,
        path: path.to_path_buf(),
        digest: digest(current),
    };
    let bytes = serde_json::to_vec(&source).map_err(|source| AppError::JsonSerialize { source })?;
    store.create_device(vault, &marker, &bytes)
}

#[cfg(test)]
#[path = "engine_persistence_tests.rs"]
mod engine_persistence_tests;

/// Make directory entries durable after staging, publication and deletion.
pub(crate) fn sync_parent(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| AppError::io(parent, e))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
