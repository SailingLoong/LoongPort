//! Stateless file permissions and writes, shared by configuration and vault callers.
//! Paths are supplied by the caller; this module never resolves a user home.

use crate::error::AppError;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(windows)]
#[path = "windows_private_file.rs"]
mod windows_private_file;

/// Create or tighten one application-owned directory without changing its parent.
/// Callers create nested application directories one level at a time.
pub fn ensure_private_directory(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(AppError::io(path, error)),
        }
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| AppError::io(path, error))?;
        directory
            .set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|error| AppError::io(path, error))?;
        Ok(())
    }

    #[cfg(windows)]
    {
        let created = match fs::create_dir(path) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(AppError::io(path, error)),
        };
        if let Err(error) = windows_private_file::restrict_existing(path, true) {
            if created {
                let _ = fs::remove_dir(path);
            }
            return Err(AppError::io(path, error));
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    Err(AppError::Config(
        "private directory permissions are unsupported on this platform".into(),
    ))
}

/// Create or tighten one application-owned database/container file without truncating it.
pub fn ensure_private_file(path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let file = loop {
            match fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
            {
                Ok(file) => break file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                        .open(path)
                    {
                        Ok(file) => break file,
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(AppError::io(path, error)),
                    }
                }
                Err(error) => return Err(AppError::io(path, error)),
            }
        };
        if !file
            .metadata()
            .map_err(|error| AppError::io(path, error))?
            .is_file()
        {
            return Err(AppError::Config(format!(
                "private file path is not a regular file: {}",
                path.display()
            )));
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| AppError::io(path, error))?;
        Ok(())
    }

    #[cfg(windows)]
    {
        match windows_private_file::create_new(path) {
            Ok(file) => {
                drop(file);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(AppError::io(path, error)),
        }
        windows_private_file::restrict_existing(path, false)
            .map_err(|error| AppError::io(path, error))?;
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    Err(AppError::Config(
        "private file permissions are unsupported on this platform".into(),
    ))
}

/// fsync 一个已存在的私有文件。Windows 上 FlushFileBuffers 要求句柄带写权限，
/// 平台层以读写方式打开；unix 侧只读打开即可。
pub fn sync_private_file(path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        windows_private_file::sync_file(path).map_err(|error| AppError::io(path, error))
    }
    #[cfg(not(windows))]
    {
        std::fs::File::open(path)
            .and_then(|file| file.sync_all())
            .map_err(|error| AppError::io(path, error))
    }
}

/// 原子写入：写入临时文件后 rename 替换，避免半写状态
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    atomic_write_with_unix_mode(path, data, None)
}

/// 原子写入包含凭据的文件。Unix 上新文件和替换文件始终使用 0600。
pub fn atomic_write_private(path: &Path, data: &[u8]) -> Result<(), AppError> {
    atomic_write_with_unix_mode(path, data, Some(0o600))
}

fn atomic_write_with_unix_mode(
    path: &Path,
    data: &[u8],
    unix_mode: Option<u32>,
) -> Result<(), AppError> {
    #[cfg(windows)]
    let private = unix_mode.is_some();
    #[cfg(not(any(unix, windows)))]
    let _ = unix_mode;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("无效的文件名".to_string()))?
        .to_string_lossy()
        .to_string();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (tmp, mut file) = (|| -> Result<(PathBuf, fs::File), AppError> {
        let mut last_collision = None;
        for _ in 0..16 {
            let counter = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = parent.join(format!(
                "{file_name}.tmp.{}.{ts}.{counter}",
                std::process::id()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            #[cfg(windows)]
            let opened = if private {
                windows_private_file::create_new(&candidate)
            } else {
                options.open(&candidate)
            };
            #[cfg(not(windows))]
            let opened = options.open(&candidate);
            match opened {
                Ok(file) => return Ok((candidate, file)),
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_collision = Some((candidate, source));
                }
                Err(source) => return Err(AppError::io(&candidate, source)),
            }
        }

        let (candidate, source) = last_collision.expect("temporary filename loop must run");
        Err(AppError::io(&candidate, source))
    })()?;

    if let Err(source) = file.write_all(data).and_then(|_| file.flush()) {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(AppError::io(&tmp, source));
    }
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = unix_mode {
            if let Err(source) = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)) {
                let _ = fs::remove_file(&tmp);
                return Err(AppError::io(&tmp, source));
            }
        } else if let Ok(meta) = fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::ERROR_NOT_SUPPORTED, Storage::FileSystem::ReplaceFileW,
        };

        if private {
            let mut last_error = None;
            for _ in 0..3 {
                match fs::rename(&tmp, path) {
                    Ok(()) => return Ok(()),
                    Err(source)
                        if matches!(
                            source.kind(),
                            std::io::ErrorKind::AlreadyExists
                                | std::io::ErrorKind::PermissionDenied
                        ) =>
                    {
                        last_error = Some(source);
                    }
                    Err(source) => {
                        last_error = Some(source);
                        break;
                    }
                }
            }
            let source = last_error.unwrap_or_else(std::io::Error::last_os_error);
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }

        let replaced: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let replacement: Vec<u16> = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut completed = false;
        let mut last_error = None;

        for _ in 0..3 {
            // SAFETY: both path buffers are NUL-terminated UTF-16 and remain alive for the
            // duration of the call. Backup, exclusion, and reserved pointers are intentionally null.
            let replaced_ok = unsafe {
                ReplaceFileW(
                    replaced.as_ptr(),
                    replacement.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if replaced_ok != 0 {
                completed = true;
                break;
            }

            let replace_error = std::io::Error::last_os_error();
            // WSL UNC paths reject ReplaceFileW with ERROR_NOT_SUPPORTED (50).
            // std::fs::rename uses a different replace-existing API on Windows.
            let replace_not_supported =
                replace_error.raw_os_error() == Some(ERROR_NOT_SUPPORTED as i32);
            if replace_error.kind() != std::io::ErrorKind::NotFound && !replace_not_supported {
                last_error = Some(replace_error);
                break;
            }

            match fs::rename(&tmp, path) {
                Ok(()) => {
                    completed = true;
                    break;
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    last_error = Some(source);
                }
                Err(source) => {
                    last_error = Some(source);
                    break;
                }
            }
        }

        if !completed {
            let source = last_error.unwrap_or_else(std::io::Error::last_os_error);
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }

    #[cfg(not(windows))]
    {
        if let Err(source) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }
    Ok(())
}

pub(crate) fn write_durable(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    atomic_write_private(path, bytes)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| AppError::io(path, e))?;
    file.sync_all().map_err(|e| AppError::io(path, e))?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| AppError::io(parent, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_in(directory: &Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_creation_and_repair_leave_parent_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let directory = root.path().join("private");
        ensure_private_directory(&directory).unwrap();
        assert_eq!(mode(&directory), 0o700);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_directory(&directory).unwrap();
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(root.path()), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_rejects_symlink_without_changing_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(ensure_private_directory(&link).is_err());
        assert_eq!(mode(&target), 0o755);
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn private_file_creation_and_repair_preserve_bytes() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.db");
        ensure_private_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"");
        assert_eq!(mode(&path), 0o600);
        fs::write(&path, b"existing contents").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        ensure_private_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"existing contents");
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn private_file_rejects_symlink_without_changing_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        fs::write(&target, b"untouched").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(ensure_private_file(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert_eq!(mode(&target), 0o644);
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn private_file_rejects_directory_without_changing_mode() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_file(root.path()).is_err());
        assert_eq!(mode(root.path()), 0o755);
    }

    #[test]
    fn atomic_writes_replace_existing_bytes_without_leaving_temporary_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture");
        fs::write(&path, b"longer existing contents").unwrap();
        atomic_write(&path, b"replacement").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        atomic_write_private(&path, b"private").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"private");
        assert_eq!(names_in(root.path()), ["fixture"]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_mode_and_private_write_tightens_it() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture");
        fs::write(&path, b"before").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&path, b"ordinary").unwrap();
        assert_eq!(mode(&path), 0o640);
        atomic_write_private(&path, b"private").unwrap();
        assert_eq!(mode(&path), 0o600);
        let new_path = root.path().join("new");
        atomic_write_private(&new_path, b"new private").unwrap();
        assert_eq!(mode(&new_path), 0o600);
    }

    #[test]
    fn failed_replacement_preserves_destination_and_unowned_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("contents"), b"keep destination").unwrap();
        let unowned = root.path().join("destination.tmp.unowned");
        fs::write(&unowned, b"keep unowned file").unwrap();
        for write in [atomic_write, atomic_write_private, write_durable] {
            assert!(write(&destination, b"replacement").is_err());
            assert_eq!(
                fs::read(destination.join("contents")).unwrap(),
                b"keep destination"
            );
            assert_eq!(fs::read(&unowned).unwrap(), b"keep unowned file");
            assert_eq!(
                names_in(root.path()),
                ["destination", "destination.tmp.unowned"]
            );
        }
    }

    #[test]
    fn failed_parent_creation_does_not_truncate_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("file");
        fs::write(&parent, b"keep existing bytes").unwrap();
        for write in [atomic_write, atomic_write_private, write_durable] {
            assert!(write(&parent.join("child"), b"replacement").is_err());
            assert_eq!(fs::read(&parent).unwrap(), b"keep existing bytes");
            assert_eq!(names_in(root.path()), ["file"]);
        }
    }

    #[test]
    fn sync_existing_private_file_preserves_bytes_and_reports_missing_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture");
        atomic_write_private(&path, b"durable contents").unwrap();
        sync_private_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"durable contents");
        assert!(sync_private_file(&root.path().join("missing")).is_err());
        assert_eq!(names_in(root.path()), ["fixture"]);
    }

    #[test]
    fn durable_write_creates_then_replaces_file_with_no_temporary_leftovers() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture");
        write_durable(&path, b"first contents").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first contents");
        write_durable(&path, b"next").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"next");
        assert_eq!(names_in(root.path()), ["fixture"]);
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
    }
}
