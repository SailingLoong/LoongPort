//! One ZCode-specific implementation of the official directory-lock wire protocol.
//! Wire contract: zai-org/ZCode 29628c9a, shared/node/atomicFileLock.
//! Existing provider writes never reclaim; account recovery only reclaims a proven dead PID.
//! Unknown, malformed and ownerless locks remain untouched, regardless of age.

use crate::error::AppError;
use serde::Deserialize;
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_OWNER_BYTES: u64 = 1024;
const MAX_TOKEN_BYTES: usize = 128;

fn config_error(message: &str) -> AppError {
    AppError::Config(format!("ZCode: {message}"))
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn unique_token() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        now_ms(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OwnerMetadata {
    pid: u32,
    created_at: u64,
    token: String,
}

// Open handles pin file identities until after cleanup, preventing inode reuse.
struct LockEvidence {
    directory: PathBuf,
    directory_file: File,
    owner_name: String,
    owner_file: Option<File>,
    payload: Vec<u8>,
}

pub(crate) struct FileLock(LockEvidence);

impl FileLock {
    /// Provider compatibility: never remove an existing lock, even if its PID has exited.
    pub(crate) fn acquire(path: &Path, timeout: Duration) -> Result<Self, AppError> {
        Self::acquire_with_policy(path, timeout, false)
    }

    /// Account crash recovery. Unsupported platforms and uncertain liveness fail closed.
    pub(crate) fn acquire_recoverable(path: &Path, timeout: Duration) -> Result<Self, AppError> {
        Self::acquire_with_policy(path, timeout, true)
    }

    fn acquire_with_policy(
        path: &Path,
        timeout: Duration,
        recover: bool,
    ) -> Result<Self, AppError> {
        let parent = path
            .parent()
            .ok_or_else(|| config_error("invalid file path"))?;
        fs::create_dir_all(parent)
            .map_err(|_| config_error("cannot create configuration directory"))?;
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        let directory = PathBuf::from(name);
        let started = Instant::now();
        loop {
            match fs::create_dir(&directory) {
                Ok(()) => return Self::establish(directory),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Never reclaim after the caller's deadline, even if an owner just exited.
                    if started.elapsed() >= timeout {
                        return Err(config_error("file lock timeout; configuration preserved"));
                    }
                    if recover && reclaim_dead_owner(&directory) {
                        continue;
                    }
                    std::thread::sleep(
                        Duration::from_millis(25).min(timeout - started.elapsed().min(timeout)),
                    );
                }
                Err(_) => return Err(config_error("cannot acquire file lock")),
            }
        }
    }

    fn establish(directory: PathBuf) -> Result<Self, AppError> {
        let directory_file =
            open_directory(&directory).map_err(|_| config_error("cannot verify file lock"))?;
        let token = unique_token();
        let owner_name = format!("owner-{token}.json");
        let payload = json!({"pid":std::process::id(),"createdAt":now_ms(),"token":token})
            .to_string()
            .into_bytes();
        let mut owner_file = open_owner(&directory_file, &directory, &owner_name, true)
            .map_err(|_| config_error("cannot establish file lock"))?;
        owner_file
            .write_all(&payload)
            .map_err(|_| config_error("cannot establish file lock"))?;
        let guard = Self(LockEvidence {
            directory,
            directory_file,
            owner_name,
            owner_file: Some(owner_file),
            payload,
        });
        if !guard.0.unchanged() {
            return Err(config_error("file lock ownership changed"));
        }
        Ok(guard)
    }
}

impl LockEvidence {
    fn unchanged(&self) -> bool {
        (|| -> std::io::Result<bool> {
            let directory = open_directory(&self.directory)?;
            if !same_identity(&directory, &self.directory_file)?
                || only_owner_name(&self.directory)?.as_deref() != Some(&self.owner_name)
            {
                return Ok(false);
            }
            let owner = open_owner(
                &self.directory_file,
                &self.directory,
                &self.owner_name,
                false,
            )?;
            if !same_identity(&owner, self.owner_file.as_ref().ok_or_else(invalid_lock)?)?
                || read_owner(&owner)? != self.payload
            {
                return Ok(false);
            }
            let current_directory = open_directory(&self.directory)?;
            same_identity(&current_directory, &self.directory_file)
        })()
        .unwrap_or(false)
    }

    fn remove_unchanged(&mut self) -> bool {
        if !self.unchanged() {
            return false;
        }
        // Unix unlink is relative to the pinned directory, never a replaced directory path.
        if remove_owner(&self.directory_file, &self.directory, &self.owner_name).is_err() {
            return false;
        }
        // Windows completes a pending owner deletion only when its last handle closes.
        drop(self.owner_file.take());
        if !open_directory(&self.directory)
            .and_then(|file| same_identity(&file, &self.directory_file))
            .unwrap_or(false)
        {
            return false;
        }
        // rmdir, never recursive deletion: a new owner or unknown child makes this fail.
        fs::remove_dir(&self.directory).is_ok()
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        self.0.remove_unchanged();
    }
}

fn reclaim_dead_owner(directory: &Path) -> bool {
    (|| -> std::io::Result<bool> {
        let directory_file = open_directory(directory)?;
        let Some(owner_name) = only_owner_name(directory)? else {
            return Ok(false);
        };
        let owner_file = open_owner(&directory_file, directory, &owner_name, false)?;
        let payload = read_owner(&owner_file)?;
        let owner: OwnerMetadata = serde_json::from_slice(&payload).map_err(|_| invalid_lock())?;
        if owner.token.is_empty()
            || owner.token.len() > MAX_TOKEN_BYTES
            || !owner
                .token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || owner_name != format!("owner-{}.json", owner.token)
            || owner.created_at as u128 > now_ms().saturating_add(5 * 60_000)
            || !pid_definitely_exited(owner.pid)
        {
            return Ok(false);
        }
        let mut evidence = LockEvidence {
            directory: directory.to_path_buf(),
            directory_file,
            owner_name,
            owner_file: Some(owner_file),
            payload,
        };
        Ok(evidence.remove_unchanged())
    })()
    .unwrap_or(false)
}

fn pid_definitely_exited(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        // Signal zero only checks existence/permission. EPERM and every unknown error mean
        // possibly alive; PID reuse therefore remains locked rather than being reclaimed.
        (unsafe { libc::kill(pid as libc::pid_t, 0) == -1 })
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn invalid_lock() -> std::io::Error {
    std::io::Error::other("invalid ZCode lock")
}

/// At most two directory entries are read; unknown children are not inspected or removed.
fn only_owner_name(directory: &Path) -> std::io::Result<Option<String>> {
    let mut entries = fs::read_dir(directory)?;
    let Some(first) = entries.next() else {
        return Ok(None);
    };
    let entry = first?;
    if entries.next().is_some() {
        return Ok(None);
    }
    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
        return Ok(None);
    };
    if name.len() > MAX_TOKEN_BYTES + 11 || !name.starts_with("owner-") || !name.ends_with(".json")
    {
        return Ok(None);
    }
    Ok(Some(name))
}

fn read_owner(file: &File) -> std::io::Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_OWNER_BYTES || !single_link(file)? {
        return Err(invalid_lock());
    }
    // Every read uses a newly opened descriptor, so no shared seek cursor is changed.
    let mut bytes = Vec::new();
    file.take(MAX_OWNER_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_OWNER_BYTES {
        return Err(invalid_lock());
    }
    Ok(bytes)
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() || is_reparse(&file)? {
        return Err(invalid_lock());
    }
    Ok(file)
}

fn open_owner(
    directory_file: &File,
    directory: &Path,
    name: &str,
    create: bool,
) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        let _ = directory;
        let name = std::ffi::CString::new(name).map_err(|_| invalid_lock())?;
        let flags = libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if create {
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
            } else {
                libc::O_RDONLY
            };
        let fd = unsafe {
            libc::openat(
                directory_file.as_raw_fd(),
                name.as_ptr(),
                flags,
                // C variadic arguments require integer promotion; macOS mode_t is u16.
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(not(unix))]
    {
        let _ = directory_file;
        let mut options = OpenOptions::new();
        options.read(true).write(create).create_new(create);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let file = options.open(directory.join(name))?;
        if is_reparse(&file)? {
            return Err(invalid_lock());
        }
        Ok(file)
    }
}

fn remove_owner(directory_file: &File, directory: &Path, name: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let _ = directory;
        let name = std::ffi::CString::new(name).map_err(|_| invalid_lock())?;
        if unsafe { libc::unlinkat(directory_file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = directory_file;
        fs::remove_file(directory.join(name))
    }
}

fn same_identity(left: &File, right: &File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = left.metadata()?;
        let right = right.metadata()?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
    #[cfg(windows)]
    {
        let left = windows_identity(left)?;
        let right = windows_identity(right)?;
        Ok(left.dwVolumeSerialNumber == right.dwVolumeSerialNumber
            && left.nFileIndexHigh == right.nFileIndexHigh
            && left.nFileIndexLow == right.nFileIndexLow)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        Err(invalid_lock())
    }
}

fn single_link(file: &File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink() == 1)
    }
    #[cfg(windows)]
    {
        Ok(windows_identity(file)?.nNumberOfLinks == 1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Ok(false)
    }
}

fn is_reparse(file: &File) -> std::io::Result<bool> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(file.metadata()?.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = file;
        Ok(false)
    }
}

#[cfg(windows)]
fn windows_identity(
    file: &File,
) -> std::io::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_dir(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        PathBuf::from(name)
    }

    fn create_owner(path: &Path, pid: u32, token: &str) -> PathBuf {
        let directory = lock_dir(path);
        fs::create_dir(&directory).unwrap();
        let owner = directory.join(format!("owner-{token}.json"));
        fs::write(
            &owner,
            json!({"pid":pid,"createdAt":0,"token":token}).to_string(),
        )
        .unwrap();
        owner
    }

    #[test]
    fn acquisition_uses_the_official_owner_wire_format_and_releases() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        let entries = fs::read_dir(lock_dir(&path))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(entries[0].path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(entries[0].path()).unwrap()).unwrap();
        assert_eq!(value["pid"], std::process::id());
        assert!(value["createdAt"].as_u64().unwrap() > 0);
        assert_eq!(
            entries[0].file_name(),
            format!("owner-{}.json", value["token"].as_str().unwrap()).as_str()
        );
        drop(guard);
        assert!(!lock_dir(&path).exists());
    }

    #[test]
    fn release_preserves_a_replaced_owner_inode_even_with_identical_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        let owner = fs::read_dir(lock_dir(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(&owner).unwrap();
        fs::rename(&owner, root.path().join("previous-owner.json")).unwrap();
        fs::write(&owner, &bytes).unwrap();
        drop(guard);
        assert_eq!(fs::read(owner).unwrap(), bytes);
    }

    #[test]
    fn release_preserves_rewritten_owner_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        let owner = fs::read_dir(lock_dir(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let foreign =
            json!({"pid":std::process::id(),"createdAt":0,"token":"different"}).to_string();
        fs::write(&owner, &foreign).unwrap();
        drop(guard);
        assert_eq!(fs::read_to_string(owner).unwrap(), foreign);
    }

    #[test]
    fn release_preserves_a_replaced_lock_directory() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        let owner = fs::read_dir(lock_dir(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(&owner).unwrap();
        fs::rename(lock_dir(&path), root.path().join("old.lock")).unwrap();
        fs::create_dir(lock_dir(&path)).unwrap();
        fs::write(&owner, &bytes).unwrap();
        drop(guard);
        assert_eq!(fs::read(owner).unwrap(), bytes);
    }

    #[test]
    fn recoverable_acquire_never_reclaims_a_live_pid_by_age() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let owner = create_owner(&path, std::process::id(), "old-but-live");
        let bytes = fs::read(&owner).unwrap();
        assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(30)).is_err());
        assert_eq!(fs::read(owner).unwrap(), bytes);
    }

    #[test]
    fn recoverable_acquire_preserves_fresh_ownerless_lock() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        fs::create_dir(lock_dir(&path)).unwrap();
        assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(30)).is_err());
        assert!(lock_dir(&path).is_dir());
    }

    #[test]
    #[ignore = "Only the parent test launches this synthetic crash fixture"]
    fn crash_fixture_child() {
        let root = std::env::var_os("LOONGPORT_ZCODE_LOCK_FIXTURE").expect("explicit fixture root");
        let path = PathBuf::from(root).join("credentials.json");
        let _guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        std::process::exit(0);
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_acquire_reclaims_a_real_exited_subprocess_lock() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "zcode_file_lock::tests::crash_fixture_child",
                "--ignored",
                "--nocapture",
            ])
            .env("LOONGPORT_ZCODE_LOCK_FIXTURE", root.path())
            .status()
            .unwrap();
        assert!(status.success());
        let owner = fs::read_dir(lock_dir(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(&owner).unwrap();
        // Existing provider API deliberately continues to fail closed.
        assert!(FileLock::acquire(&path, Duration::from_millis(30)).is_err());
        assert_eq!(fs::read(&owner).unwrap(), bytes);
        let guard = FileLock::acquire_recoverable(&path, Duration::from_millis(200)).unwrap();
        assert!(!owner.exists());
        drop(guard);
        assert!(!lock_dir(&path).exists());
    }

    #[cfg(unix)]
    fn exited_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "no_matching_test"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(child.wait().unwrap().success());
        assert!(pid_definitely_exited(pid));
        pid
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_acquire_preserves_malformed_or_unbounded_dead_owner_metadata() {
        let pid = exited_pid();
        let cases = [
            json!({"pid":pid,"createdAt":0,"token":"different"}).to_string(),
            json!({"pid":0,"createdAt":0,"token":"test"}).to_string(),
            json!({"pid":u32::MAX,"createdAt":0,"token":"test"}).to_string(),
            json!({"pid":pid,"createdAt":-1,"token":"test"}).to_string(),
            json!({"pid":pid,"createdAt":u64::MAX,"token":"test"}).to_string(),
            json!({"pid":pid,"createdAt":0,"token":"test","extra":"untrusted"}).to_string(),
            format!("{{\"pid\":{pid},\"pid\":{pid},\"createdAt\":0,\"token\":\"test\"}}"),
            "invalid-metadata".to_owned(),
            "x".repeat(MAX_OWNER_BYTES as usize + 1),
        ];
        for payload in cases {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("credentials.json");
            let owner = create_owner(&path, pid, "test");
            fs::write(&owner, &payload).unwrap();
            let error = FileLock::acquire_recoverable(&path, Duration::from_millis(1))
                .err()
                .expect("invalid lock must remain");
            assert_eq!(
                error.to_string(),
                "配置错误: ZCode: file lock timeout; configuration preserved"
            );
            assert_eq!(fs::read_to_string(owner).unwrap(), payload);
        }
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_acquire_preserves_unknown_children_and_multiple_owners() {
        let pid = exited_pid();
        for extra in ["foreign.txt", "owner-other.json", "nested"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("credentials.json");
            let owner = create_owner(&path, pid, "test");
            let extra_path = lock_dir(&path).join(extra);
            if extra == "nested" {
                fs::create_dir(&extra_path).unwrap();
            } else {
                fs::write(&extra_path, "preserve-me").unwrap();
            }
            let bytes = fs::read(&owner).unwrap();
            assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(1)).is_err());
            assert_eq!(fs::read(&owner).unwrap(), bytes);
            assert!(extra_path.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_acquire_preserves_symlink_and_hardlinked_owners() {
        use std::os::unix::fs::symlink;
        let pid = exited_pid();
        for is_symlink in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("credentials.json");
            let owner = create_owner(&path, pid, "test");
            let target = root.path().join("foreign.json");
            fs::rename(&owner, &target).unwrap();
            if is_symlink {
                symlink(&target, &owner).unwrap();
            } else {
                fs::hard_link(&target, &owner).unwrap();
            }
            let bytes = fs::read(&target).unwrap();
            assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(1)).is_err());
            assert_eq!(fs::read(&owner).unwrap(), bytes);
            assert_eq!(fs::read(&target).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_acquire_preserves_symlinked_lock_directory_and_legacy_file() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let target = root.path().join("foreign.lock");
        fs::create_dir(&target).unwrap();
        symlink(&target, lock_dir(&path)).unwrap();
        assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(1)).is_err());
        assert!(fs::symlink_metadata(lock_dir(&path))
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_file(lock_dir(&path)).unwrap();
        fs::write(lock_dir(&path), "legacy-owner").unwrap();
        assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(1)).is_err());
        assert_eq!(fs::read(lock_dir(&path)).unwrap(), b"legacy-owner");
    }

    #[test]
    fn release_leaves_unknown_children_and_its_owner_untouched() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
        let owner = fs::read_dir(lock_dir(&path))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(lock_dir(&path).join("foreign"), "preserve-me").unwrap();
        drop(guard);
        assert!(owner.exists());
        assert_eq!(
            fs::read(lock_dir(&path).join("foreign")).unwrap(),
            b"preserve-me"
        );
    }

    #[cfg(unix)]
    #[test]
    fn recoverable_acquire_does_not_reclaim_after_deadline() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.json");
        let owner = create_owner(&path, exited_pid(), "test");
        assert!(FileLock::acquire_recoverable(&path, Duration::ZERO).is_err());
        assert!(owner.exists());
    }

    #[cfg(unix)]
    #[test]
    fn acquisition_preserves_non_utf8_path_identity() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join(std::ffi::OsString::from_vec(b"native-\xff.json".to_vec()));
        let raw_lock = root.path().join(std::ffi::OsString::from_vec(
            b"native-\xff.json.lock".to_vec(),
        ));
        let entries = || {
            fs::read_dir(root.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>()
        };
        // Probe this filesystem with the exact bytes independently of FileLock.
        // Darwin filesystems can reject them; Linux filesystems generally retain them.
        match fs::create_dir(&raw_lock) {
            Ok(()) => {
                assert_eq!(entries().len(), 1);
                assert_eq!(entries()[0].as_bytes(), b"native-\xff.json.lock");
                let owner = raw_lock.join("owner-native.json");
                let payload =
                    json!({"pid":std::process::id(),"createdAt":now_ms(),"token":"native"})
                        .to_string();
                fs::write(&owner, &payload).unwrap();
                assert!(FileLock::acquire(&path, Duration::ZERO).is_err());
                assert!(FileLock::acquire_recoverable(&path, Duration::from_millis(1)).is_err());
                assert_eq!(fs::read_to_string(&owner).unwrap(), payload);
                assert_eq!(entries().len(), 1);
                assert_eq!(entries()[0].as_bytes(), b"native-\xff.json.lock");
                fs::remove_file(&owner).unwrap();
                fs::remove_dir(&raw_lock).unwrap();

                let guard = FileLock::acquire(&path, Duration::ZERO).unwrap();
                assert_eq!(entries().len(), 1);
                assert_eq!(entries()[0].as_bytes(), b"native-\xff.json.lock");
                assert!(FileLock::acquire(&path, Duration::ZERO).is_err());
                drop(guard);
                assert!(entries().is_empty());
                let guard = FileLock::acquire_recoverable(&path, Duration::ZERO).unwrap();
                assert_eq!(entries().len(), 1);
                assert_eq!(entries()[0].as_bytes(), b"native-\xff.json.lock");
                drop(guard);
                assert!(entries().is_empty());
            }
            Err(error) if error.raw_os_error() == Some(libc::EILSEQ) => {
                // Rejection is a checked result, not a skip: neither acquisition
                // policy may create any entry, including a lossy UTF-8 substitute.
                assert!(entries().is_empty());
                assert!(matches!(FileLock::acquire(&path, Duration::ZERO),
                    Err(AppError::Config(message)) if message == "ZCode: cannot acquire file lock"));
                assert!(entries().is_empty());
                assert!(
                    matches!(FileLock::acquire_recoverable(&path, Duration::ZERO),
                    Err(AppError::Config(message)) if message == "ZCode: cannot acquire file lock")
                );
                assert!(entries().is_empty());
                assert_eq!(
                    fs::create_dir(&raw_lock).unwrap_err().raw_os_error(),
                    Some(libc::EILSEQ)
                );
                assert!(entries().is_empty());
            }
            Err(error) => panic!("unexpected raw-path creation error: {error}"),
        }
    }
}
