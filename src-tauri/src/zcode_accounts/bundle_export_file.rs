//! Private create-new publication in one checked directory. Never replaces a path.
use super::{valid_destination_shape, ExportFailure};
use crate::zcode_accounts::{bundle_limits::MAX_BUNDLE_BYTES, transaction::native_root_identity};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{self, File},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};

pub(super) struct Destination {
    path: PathBuf,
    directory: File,
    identity: [u64; 2],
    name: CString,
}

impl Destination {
    pub(super) fn open(path: &Path, protected: &[&Path]) -> Result<Self, ExportFailure> {
        if !valid_destination_shape(path) {
            return Err(ExportFailure::UnsafeDestination);
        }
        let parent = path.parent().ok_or(ExportFailure::UnsafeDestination)?;
        let identity =
            native_root_identity(parent).map_err(|_| ExportFailure::UnsafeDestination)?;
        for root in protected {
            if path.starts_with(normalize_protected(root)?) {
                return Err(ExportFailure::UnsafeDestination);
            }
        }
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)
            .map_err(|_| ExportFailure::UnsafeDestination)?;
        let metadata = directory.metadata().map_err(|_| ExportFailure::Storage)?;
        if [metadata.dev(), metadata.ino()] != identity {
            return Err(ExportFailure::UnsafeDestination);
        }
        let name = CString::new(
            path.file_name()
                .ok_or(ExportFailure::UnsafeDestination)?
                .as_bytes(),
        )
        .map_err(|_| ExportFailure::UnsafeDestination)?;
        Ok(Self {
            path: path.to_owned(),
            directory,
            identity,
            name,
        })
    }
    fn check_parent(&self) -> Result<(), ExportFailure> {
        if native_root_identity(self.path.parent().ok_or(ExportFailure::UnsafeDestination)?)
            .map_err(|_| ExportFailure::UnsafeDestination)?
            != self.identity
        {
            return Err(ExportFailure::UnsafeDestination);
        }
        Ok(())
    }
    pub(super) fn require_absent(&self) -> Result<(), ExportFailure> {
        self.check_parent()?;
        match fs::symlink_metadata(&self.path) {
            Ok(_) => Err(ExportFailure::DestinationExists),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ExportFailure::UnsafeDestination),
        }
    }
    pub(super) fn create_new(&self) -> Result<File, ExportFailure> {
        self.check_parent()?;
        // The held directory descriptor prevents a parent-path race from redirecting
        // this write. O_EXCL atomically refuses every existing entry, even symlinks.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::AlreadyExists {
                    ExportFailure::DestinationExists
                } else {
                    ExportFailure::Storage
                },
            );
        }
        // SAFETY: openat just returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(super) fn sync_directory(&self) -> Result<(), ExportFailure> {
        self.check_parent()?;
        self.directory
            .sync_all()
            .map_err(|_| ExportFailure::Storage)
    }
    fn open_read(&self) -> Result<File, ExportFailure> {
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(ExportFailure::Storage);
        }
        // SAFETY: openat just returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub(super) fn verified_bytes(
        &self,
        expected_bytes: usize,
        expected_digest: &[u8; 32],
        created_file: Option<&File>,
    ) -> Result<Vec<u8>, ExportFailure> {
        self.check_parent()?;
        let mut file = self.open_read()?;
        let before = file.metadata().map_err(|_| ExportFailure::Storage)?;
        validate_file(&before, expected_bytes)?;
        if let Some(created) = created_file {
            let expected = created.metadata().map_err(|_| ExportFailure::Storage)?;
            if [before.dev(), before.ino()] != [expected.dev(), expected.ino()] {
                return Err(ExportFailure::SavedDataInvalid);
            }
        }
        let mut bytes = Vec::with_capacity(expected_bytes);
        (&mut file)
            .take(MAX_BUNDLE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ExportFailure::Storage)?;
        let after = file.metadata().map_err(|_| ExportFailure::Storage)?;
        validate_file(&after, expected_bytes)?;
        if stamp(&before) != stamp(&after)
            || bytes.len() != expected_bytes
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != *expected_digest
        {
            return Err(ExportFailure::SavedDataInvalid);
        }
        // Reading a formerly-correct unlinked inode cannot prove the destination.
        let linked = self
            .open_read()?
            .metadata()
            .map_err(|_| ExportFailure::Storage)?;
        validate_file(&linked, expected_bytes)?;
        if stamp(&after) != stamp(&linked) {
            return Err(ExportFailure::SavedDataInvalid);
        }
        self.check_parent()?;
        Ok(bytes)
    }
}

fn validate_file(meta: &fs::Metadata, expected_bytes: usize) -> Result<(), ExportFailure> {
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() != expected_bytes as u64
        || expected_bytes == 0
        || expected_bytes > MAX_BUNDLE_BYTES
    {
        return Err(ExportFailure::SavedDataInvalid);
    }
    Ok(())
}
fn stamp(meta: &fs::Metadata) -> [u64; 7] {
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

/// Account-library backup works before a native data directory exists. Resolve
/// only its existing ancestor; do not create or read a native credential file.
fn normalize_protected(path: &Path) -> Result<PathBuf, ExportFailure> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(ExportFailure::UnsafeDestination);
    }
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A dangling link is not an absent account directory.
                if fs::symlink_metadata(ancestor).is_ok() {
                    return Err(ExportFailure::UnsafeDestination);
                }
                suffix.push(
                    ancestor
                        .file_name()
                        .ok_or(ExportFailure::UnsafeDestination)?
                        .to_owned(),
                );
                ancestor = ancestor.parent().ok_or(ExportFailure::UnsafeDestination)?;
            }
            Err(_) => return Err(ExportFailure::UnsafeDestination),
        }
    }
}
