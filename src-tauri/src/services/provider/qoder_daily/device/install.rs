//! Exact public release pins, derived without executing the official package.
//! Evidence: REPORT-range-v2.md, official Desktop 0.4.8 mac-arm64.
use super::super::Dependency;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub(super) const ROOT: &str = "/Applications/Qoder.app";
pub(super) const HELPER: &str = "/Applications/Qoder.app/Contents/Resources/umid/runtime-info";
pub(super) const REQUIREMENT: &str = "anchor apple generic and identifier \"com.qoder.app\" and certificate leaf[subject.OU] = \"B6U242QL73\" and cdhash H\"03f94b883f04d5569777f583b345f8e03373c9e5\"";
pub(super) const HELPER_REQUIREMENT: &str = "anchor apple generic and identifier \"runtime-info\" and certificate leaf[subject.OU] = \"B6U242QL73\" and cdhash H\"80704babad49ae2dd9aec7b92cc4ef892da1d301\"";
pub(super) struct Pin {
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}
pub(super) const PINS: [Pin; 4] = [
    Pin {
        path: "Contents/Info.plist",
        size: 3916,
        sha256: "75092d5199698294377cc60765e8f6bca69f4aeb7614fd0162ed37a7bcca7d9c",
    },
    Pin {
        path: "Contents/_CodeSignature/CodeResources",
        size: 1757093,
        sha256: "32de8917d174605eda44f14fd0234e643fbcc8b5b781bbcb2d278bdcf2cc3cf6",
    },
    Pin {
        path: "Contents/MacOS/Qoder",
        size: 53248,
        sha256: "a1386a2e23a3a322fc389b4119193786ab06c523e476c4d9cbecf62c879593d7",
    },
    // Sealed installed bytes, not the discrepant umid/manifest.json declaration.
    Pin {
        path: "Contents/Resources/umid/runtime-info",
        size: 1022240,
        sha256: "298777e1a6c2dc670244437dac238474cf8654c8ba003ccb64c0ff1fa9bcab95",
    },
];
fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(PartialEq, Eq)]
struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
fn stamp(m: &fs::Metadata) -> Stamp {
    Stamp {
        dev: m.dev(),
        ino: m.ino(),
        size: m.len(),
        mode: m.mode(),
        uid: m.uid(),
        gid: m.gid(),
        links: m.nlink(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
    }
}
pub(super) struct PinnedFile {
    path: PathBuf,
    file: File,
    stamp: Stamp,
}
impl PinnedFile {
    pub(super) fn open(path: PathBuf, pin: &Pin) -> Result<Self, Dependency> {
        if fs::canonicalize(&path).map_err(|_| Dependency::OfficialDeviceUnverified)? != path {
            return Err(Dependency::OfficialDeviceUnverified);
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| Dependency::OfficialDeviceUnverified)?;
        let meta = file
            .metadata()
            .map_err(|_| Dependency::OfficialDeviceUnverified)?;
        if !meta.is_file()
            || meta.len() != pin.size
            || meta.nlink() != 1
            || meta.mode() & 0o6022 != 0
            || (meta.uid() != 0 && meta.uid() != unsafe { libc::geteuid() })
        {
            return Err(Dependency::OfficialDeviceUnverified);
        }
        if (pin.path == "Contents/MacOS/Qoder" || pin.path.ends_with("/runtime-info"))
            && meta.mode() & 0o111 == 0
        {
            return Err(Dependency::OfficialDeviceUnverified);
        }
        let mut bytes = Vec::with_capacity(pin.size as usize);
        (&mut file)
            .take(pin.size + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Dependency::OfficialDeviceUnverified)?;
        if bytes.len() as u64 != pin.size || hex_digest(&bytes) != pin.sha256 {
            return Err(Dependency::OfficialDeviceUnverified);
        }
        let result = Self {
            path,
            file,
            stamp: stamp(&meta),
        };
        result.recheck()?;
        Ok(result)
    }
    pub(super) fn recheck(&self) -> Result<(), Dependency> {
        let named =
            fs::symlink_metadata(&self.path).map_err(|_| Dependency::OfficialDeviceUnverified)?;
        let opened = self
            .file
            .metadata()
            .map_err(|_| Dependency::OfficialDeviceUnverified)?;
        if fs::canonicalize(&self.path).map_err(|_| Dependency::OfficialDeviceUnverified)?
            != self.path
            || !named.is_file()
            || named.file_type().is_symlink()
            || stamp(&named) != self.stamp
            || stamp(&opened) != self.stamp
        {
            return Err(Dependency::OfficialDeviceUnverified);
        }
        Ok(())
    }
}
pub(super) struct Installation {
    files: Vec<PinnedFile>,
}
impl Installation {
    pub(super) fn open() -> Result<Self, Dependency> {
        let root = Path::new(ROOT);
        if !root.exists() {
            return Err(Dependency::OfficialDeviceUnavailable);
        }
        if fs::canonicalize(root).map_err(|_| Dependency::OfficialDeviceUnverified)? != root {
            return Err(Dependency::OfficialDeviceUnverified);
        }
        // Parent directory replacement must not silently select another image.
        for dir in [
            "/Applications",
            ROOT,
            "/Applications/Qoder.app/Contents",
            "/Applications/Qoder.app/Contents/Resources",
            "/Applications/Qoder.app/Contents/Resources/umid",
        ] {
            let m = fs::symlink_metadata(dir).map_err(|_| Dependency::OfficialDeviceUnverified)?;
            if !m.is_dir()
                || m.file_type().is_symlink()
                || m.mode() & (if dir == "/Applications" { 0o002 } else { 0o022 }) != 0
                || (m.uid() != 0 && m.uid() != unsafe { libc::geteuid() })
            {
                return Err(Dependency::OfficialDeviceUnverified);
            }
        }
        let mut files = Vec::new();
        for (index, pin) in PINS.iter().enumerate() {
            files.push(PinnedFile::open(root.join(pin.path), pin).map_err(|e| {
                if index == 0 {
                    Dependency::OfficialDeviceUnsupported
                } else {
                    e
                }
            })?);
        }
        Ok(Self { files })
    }
    pub(super) fn recheck(&self) -> Result<(), Dependency> {
        for file in &self.files {
            file.recheck()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture() -> (Temp, PathBuf, Pin) {
        let root = std::env::temp_dir().join(format!(
            "loongport-qoder-pin-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("fixture");
        fs::write(&path, b"synthetic").unwrap();
        (
            Temp(root),
            path,
            Pin {
                path: "fixture",
                size: 9,
                sha256: "b3cc0475bb78a63c99bb03ec2b60bc53452feecbaf7d7b7bc75a600a4a9ae5c3",
            },
        )
    }
    #[test]
    fn exact_bytes_required_before_a_file_identity_is_issued() {
        let (_t, p, mut pin) = fixture();
        pin.sha256 = "wrong";
        assert!(PinnedFile::open(p, &pin).is_err());
    }
    #[test]
    fn inode_replacement_is_rejected_while_original_handle_is_held() {
        let (_t, p, mut pin) = fixture();
        let hash = hex_digest(b"synthetic");
        pin.sha256 = Box::leak(hash.into_boxed_str());
        let held = PinnedFile::open(p.clone(), &pin).unwrap();
        let other = p.with_extension("new");
        fs::write(&other, b"synthetic").unwrap();
        fs::rename(other, p).unwrap();
        assert!(held.recheck().is_err());
    }
    #[test]
    fn symlink_and_hardlink_never_inherit_a_hash_pin() {
        let (_t, p, mut pin) = fixture();
        let hash = hex_digest(b"synthetic");
        pin.sha256 = Box::leak(hash.into_boxed_str());
        let link = p.with_extension("link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(PinnedFile::open(link, &pin).is_err());
        fs::hard_link(&p, p.with_extension("hard")).unwrap();
        assert!(PinnedFile::open(p, &pin).is_err());
    }
    #[test]
    fn mutable_bytes_in_same_inode_are_rejected_on_recheck() {
        let (_t, p, mut pin) = fixture();
        let hash = hex_digest(b"synthetic");
        pin.sha256 = Box::leak(hash.into_boxed_str());
        let held = PinnedFile::open(p.clone(), &pin).unwrap();
        fs::write(p, b"different").unwrap();
        assert!(held.recheck().is_err());
    }
}
