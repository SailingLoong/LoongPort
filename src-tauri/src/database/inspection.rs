//! Passive SQLite inspection. SQLite only opens a private captured DB/WAL set.
//! This is an observed-stable image, not an online-backup or writer-lock guarantee.

use crate::{config_file_io, error::AppError};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub(crate) struct InspectedDatabase {
    pub image: Connection,
    pub revision: SourceRevision,
}

/// Deliberately not serializable or Debug: paths/digests are private evidence.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SourceRevision {
    path: PathBuf,
    directories: Vec<(PathBuf, FileIdentity)>,
    files: Vec<Option<FileRevision>>,
}

#[derive(Clone, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u64,
    #[cfg(windows)]
    id: [u8; 16],
}

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    identity: FileIdentity,
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(unix)]
    mode: u32,
    #[cfg(windows)]
    attributes: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct FileRevision {
    stamp: FileStamp,
    digest: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapturePhase {
    Copied,
    Observed,
    Validated,
}

fn error(code: &str) -> AppError {
    AppError::Config(format!("upgrade.{code}"))
}

fn changed() -> AppError {
    error("source_changed")
}

fn open_read(path: &Path, directory: bool) -> Result<File, AppError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK
                | if directory { libc::O_DIRECTORY } else { 0 },
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        options.custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
    }
    options.open(path).map_err(|_| error("storage_unavailable"))
}

fn identity(file: &File, metadata: &Metadata) -> Result<FileIdentity, AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        Ok(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
        };
        let _ = metadata;
        let mut info = FILE_ID_INFO::default();
        // Same handle-identity API as the existing session-log reader. The owned
        // handle and correctly sized writable buffer remain live for this call.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                std::ptr::addr_of_mut!(info).cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } == 0
        {
            return Err(error("storage_unavailable"));
        }
        Ok(FileIdentity {
            volume: info.VolumeSerialNumber,
            id: info.FileId.Identifier,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, metadata);
        Err(error("storage_unavailable"))
    }
}

fn stamp(file: &File) -> Result<FileStamp, AppError> {
    let metadata = file.metadata().map_err(|_| error("storage_unavailable"))?;
    if !metadata.is_file() {
        return Err(error("invalid_storage_path"));
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(windows)]
    use std::os::windows::fs::MetadataExt;
    Ok(FileStamp {
        identity: identity(file, &metadata)?,
        len: metadata.len(),
        modified: metadata
            .modified()
            .map_err(|_| error("storage_unavailable"))?,
        #[cfg(unix)]
        changed: (metadata.ctime(), metadata.ctime_nsec()),
        #[cfg(unix)]
        mode: metadata.mode(),
        #[cfg(windows)]
        attributes: metadata.file_attributes(),
    })
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn observe(path: &Path, destination: Option<&Path>) -> Result<SourceRevision, AppError> {
    let path = std::path::absolute(path).map_err(|_| error("invalid_storage_path"))?;
    let parent = path.parent().ok_or_else(|| error("invalid_storage_path"))?;
    crate::secrets::files::device_directory_exists(parent)?;
    let mut directories = Vec::new();
    for ancestor in parent.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(ancestor) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err(error("storage_unavailable")),
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(error("invalid_storage_path"))
            }
            Ok(_) => {
                let file = open_read(ancestor, true)?;
                let metadata = file.metadata().map_err(|_| error("storage_unavailable"))?;
                directories.push((ancestor.to_owned(), identity(&file, &metadata)?));
            }
        }
    }
    // Distinguish inaccessible membership from a genuinely absent database.
    match fs::read_dir(parent) {
        Ok(entries) => {
            for entry in entries {
                entry.map_err(|_| error("storage_unavailable"))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(error("storage_unavailable")),
    }
    let mut files = Vec::new();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let source = sidecar(&path, suffix);
        let metadata = match fs::symlink_metadata(&source) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                files.push(None);
                continue;
            }
            Err(_) => return Err(error("storage_unavailable")),
            Ok(metadata) => metadata,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(error("invalid_storage_path"));
        }
        if suffix == "-journal" && metadata.len() > 0 {
            return Err(error("source_recovery_required"));
        }
        let mut input = open_read(&source, false)?;
        let before = stamp(&input)?;
        let mut output = if suffix.is_empty() || suffix == "-wal" {
            destination
                .map(|root| {
                    let target = root.join(
                        source
                            .file_name()
                            .ok_or_else(|| error("invalid_storage_path"))?,
                    );
                    config_file_io::ensure_private_file(&target)?;
                    OpenOptions::new()
                        .write(true)
                        .open(target)
                        .map_err(|_| error("temporary_storage_unavailable"))
                })
                .transpose()?
        } else {
            None
        };
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut read = 0u64;
        // Bound reads even if another process continually appends.
        while read <= before.len {
            let limit = (before.len - read)
                .saturating_add(1)
                .min(buffer.len() as u64) as usize;
            let count = input.read(&mut buffer[..limit]).map_err(|_| changed())?;
            if count == 0 {
                break;
            }
            read += count as u64;
            if read > before.len {
                return Err(changed());
            }
            hash.update(&buffer[..count]);
            if let Some(output) = &mut output {
                output
                    .write_all(&buffer[..count])
                    .map_err(|_| error("temporary_storage_unavailable"))?;
            }
        }
        if read != before.len
            || stamp(&input)? != before
            || stamp(&open_read(&source, false)?)? != before
        {
            return Err(changed());
        }
        files.push(Some(FileRevision {
            stamp: before,
            digest: hash.finalize().into(),
        }));
    }
    if files[0].is_none() && files.iter().skip(1).any(Option::is_some) {
        return Err(error("source_recovery_required"));
    }
    Ok(SourceRevision {
        path,
        directories,
        files,
    })
}

fn inspection_temp_base(root: &Path, device: &Path, requested: &Path) -> Result<PathBuf, AppError> {
    let root_exists = crate::secrets::files::device_directory_exists(root)?;
    // The device root is a denied destination, not a source we are admitting.
    // Canonicalizing it cannot authorize reading device members. Its aliases
    // must not impose unrelated path admission on an otherwise valid DB source.
    let device = match device.canonicalize() {
        Ok(path) => Some(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(error("storage_unavailable")),
    };
    let temp = requested
        .canonicalize()
        .map_err(|_| error("temporary_storage_unavailable"))?;
    if root_exists {
        let canonical = root
            .canonicalize()
            .map_err(|_| error("storage_unavailable"))?;
        if temp.starts_with(canonical) {
            return Err(error("temporary_storage_unavailable"));
        }
    }
    if device.is_some_and(|device| temp.starts_with(device)) {
        return Err(error("temporary_storage_unavailable"));
    }
    Ok(temp)
}

pub(crate) fn capture(path: &Path) -> Result<Option<InspectedDatabase>, AppError> {
    capture_with_hook(path, &mut |_| Ok(()))
}

fn capture_with_hook(
    path: &Path,
    hook: &mut dyn FnMut(CapturePhase) -> Result<(), AppError>,
) -> Result<Option<InspectedDatabase>, AppError> {
    let source = std::path::absolute(path).map_err(|_| error("invalid_storage_path"))?;
    let root = source
        .parent()
        .ok_or_else(|| error("invalid_storage_path"))?;
    let device = crate::live::engine::DeviceStore::for_device();
    let base = inspection_temp_base(root, device.root(), &std::env::temp_dir())?;
    let temporary = tempfile::Builder::new()
        .prefix("loongport-inspection-")
        .tempdir_in(base)
        .map_err(|_| error("temporary_storage_unavailable"))?;
    let result = (|| {
        config_file_io::ensure_private_directory(temporary.path())
            .map_err(|_| error("temporary_storage_unavailable"))?;
        let revision = observe(path, Some(temporary.path()))?;
        hook(CapturePhase::Copied)?;
        verify_unchanged(path, &revision)?;
        hook(CapturePhase::Observed)?;
        if revision.files[0].is_none() {
            verify_unchanged(path, &revision)?;
            return Ok(None);
        }
        // An existing empty file is not evidence of a fresh installation.
        if revision.files[0]
            .as_ref()
            .is_some_and(|file| file.stamp.len == 0)
        {
            return Err(error("invalid_database"));
        }
        let copy_path = temporary.path().join(
            path.file_name()
                .ok_or_else(|| error("invalid_storage_path"))?,
        );
        let copy =
            Connection::open_with_flags(copy_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|_| error("invalid_database"))?;
        let integrity: String = copy
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|_| error("invalid_database"))?;
        if integrity != "ok" {
            return Err(error("invalid_database"));
        }
        let mut image =
            Connection::open_in_memory().map_err(|_| error("temporary_storage_unavailable"))?;
        super::vault::copy(&copy, &mut image).map_err(|_| error("invalid_database"))?;
        copy.close()
            .map_err(|_| error("temporary_storage_unavailable"))?;
        hook(CapturePhase::Validated)?;
        verify_unchanged(path, &revision)?;
        Ok(Some(InspectedDatabase { image, revision }))
    })();
    // All copy connections/handles are out of scope before removing sidecars.
    if temporary.close().is_err() {
        log::warn!("upgrade.temporary_cleanup_failed");
        return Err(error("temporary_cleanup_failed"));
    }
    result
}

pub(crate) fn verify_unchanged(path: &Path, revision: &SourceRevision) -> Result<(), AppError> {
    if observe(path, None).map_err(|_| changed())? != *revision {
        return Err(changed());
    }
    Ok(())
}

#[cfg(test)]
#[path = "inspection_tests.rs"]
mod tests;
