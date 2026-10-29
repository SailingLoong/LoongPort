//! Exact public build membership and local user/install scope, never native-key
//! validation or a globally unique machine identity. No client is executed.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::process;
use super::{ControlledSource, Failure, NativeContext};
use crate::workbuddy::model::Domain;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[cfg(test)]
type ReadHook = Box<dyn FnMut(&Path)>;
#[cfg(test)]
std::thread_local! {
    static CONTENT_READS: std::cell::RefCell<std::collections::BTreeMap<PathBuf, u64>> = const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
    static CONTENT_HOOK: std::cell::RefCell<Option<ReadHook>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(super) fn take_content_reads() -> std::collections::BTreeMap<PathBuf, u64> {
    CONTENT_READS.with(|reads| std::mem::take(&mut *reads.borrow_mut()))
}
#[cfg(test)]
pub(super) struct ContentHook;
#[cfg(test)]
pub(super) fn on_content_read(hook: impl FnMut(&Path) + 'static) -> ContentHook {
    CONTENT_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    ContentHook
}
#[cfg(test)]
impl Drop for ContentHook {
    fn drop(&mut self) {
        CONTENT_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

pub(super) struct Artifact {
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}
// Official CN saas macOS ARM64 5.9.3.42398134-d4692421 DMG, SHA256
// 694046f11eb1de1adf2bca4f096e8950f871014766aa1894d2eda94e7519afba.
// Public layout/hash receipts do not attest account validity or decryptability.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ARTIFACTS: &[Artifact] = &[
    Artifact {
        path: "Contents/Info.plist",
        size: 18886,
        sha256: "9b44d180ae2a40a561da66faf9eac8fef8152aedba51a2e1a4853d441e508cb3",
    },
    Artifact {
        path: "Contents/Resources/app.asar",
        size: 181953296,
        sha256: "12a2be53ec7db2943a9958f6986d7807adcf083bd79b307d26706d83750effa7",
    },
    Artifact {
        path: "Contents/MacOS/Electron",
        size: 206384,
        sha256: "61a272736dfac172749d9ec581a31b5f23a5012f083075a4848f856bebfba0db",
    },
    Artifact {
        path: "Contents/Frameworks/Electron Framework.framework/Versions/A/Electron Framework",
        size: 171218704,
        sha256: "379cac5b10e3c83c05d445be17cbc6fad800bc4e5d6f3ed4e15add44945130bf",
    },
    Artifact {
        path: "Contents/Frameworks/Electron Framework.framework/Versions/A/Resources/Info.plist",
        size: 923,
        sha256: "448a49f27601706cf4527cc4ce77045a31f55586e090c4008a9b16385aab6187",
    },
    Artifact {
        path: "Contents/Resources/app.asar.unpacked/cli/product.json",
        size: 385139,
        sha256: "2495b11cc30f1abd786e79eb2ae34bce85f9bd49ce83503ec79c8f4b23526359",
    },
];
const LINKS: &[(&str, &str)] = &[
    ("Versions/Current", "A"),
    ("Electron Framework", "Versions/Current/Electron Framework"),
    ("Libraries", "Versions/Current/Libraries"),
    ("Helpers", "Versions/Current/Helpers"),
    ("Resources", "Versions/Current/Resources"),
];
fn identity(file: &File) -> Result<(u64, u64, u32), Failure> {
    let m = file.metadata().map_err(|_| Failure::SourceChanged)?;
    Ok((m.dev(), m.ino(), m.uid()))
}
fn directory(path: &Path) -> Result<File, Failure> {
    if !path.is_absolute()
        || !crate::config_file_io::directory_exists_no_alias(path)
            .map_err(|_| Failure::UnsafePath)?
        || std::fs::canonicalize(path).map_err(|_| Failure::UnsafePath)? != path
    {
        return Err(Failure::UnsafePath);
    }
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| Failure::UnsafePath)
}
struct PinnedArtifact {
    path: PathBuf,
    file: File,
    expected: &'static Artifact,
}
fn same_revision(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.uid() == b.uid()
        && a.len() == b.len()
        && (a.mtime(), a.mtime_nsec(), a.ctime(), a.ctime_nsec())
            == (b.mtime(), b.mtime_nsec(), b.ctime(), b.ctime_nsec())
}
impl PinnedArtifact {
    fn open(bundle: &Path, expected: &'static Artifact) -> Result<Self, Failure> {
        let path = bundle.join(expected.path);
        directory(path.parent().ok_or(Failure::UnsafePath)?)?;
        let file = crate::config_file_io::open_regular_file(&path, expected.size)
            .map_err(|_| Failure::SourceChanged)?
            .ok_or(Failure::SourceChanged)?;
        Ok(Self {
            path,
            file,
            expected,
        })
    }
    fn verify(&self) -> Result<std::fs::Metadata, Failure> {
        directory(self.path.parent().ok_or(Failure::UnsafePath)?)?;
        let mut file = crate::config_file_io::open_regular_file(&self.path, self.expected.size)
            .map_err(|_| Failure::SourceChanged)?
            .ok_or(Failure::SourceChanged)?;
        let before = file.metadata().map_err(|_| Failure::SourceChanged)?;
        if identity(&file)? != identity(&self.file)? || before.len() != self.expected.size {
            return Err(Failure::SourceChanged);
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut read = 0u64;
        loop {
            let n = file.read(&mut buffer).map_err(|_| Failure::SourceChanged)?;
            if n == 0 {
                break;
            }
            #[cfg(test)]
            CONTENT_READS.with(|reads| {
                *reads.borrow_mut().entry(self.path.clone()).or_default() += n as u64;
            });
            #[cfg(test)]
            CONTENT_HOOK.with(|slot| {
                if let Some(hook) = slot.borrow_mut().as_mut() {
                    hook(&self.path);
                }
            });
            read += n as u64;
            if read > self.expected.size {
                return Err(Failure::SourceChanged);
            }
            hash.update(&buffer[..n]);
        }
        let after = file.metadata().map_err(|_| Failure::SourceChanged)?;
        if read != self.expected.size
            || hex::encode(hash.finalize()) != self.expected.sha256
            || before.len() != after.len()
            || before.uid() != after.uid()
            || (
                before.mtime(),
                before.mtime_nsec(),
                before.ctime(),
                before.ctime_nsec(),
            ) != (
                after.mtime(),
                after.mtime_nsec(),
                after.ctime(),
                after.ctime_nsec(),
            )
        {
            return Err(Failure::SourceChanged);
        }
        self.verify_revision(&after)?;
        Ok(after)
    }
    fn verify_revision(&self, expected: &std::fs::Metadata) -> Result<(), Failure> {
        directory(self.path.parent().ok_or(Failure::UnsafePath)?)?;
        let named = crate::config_file_io::open_regular_file(&self.path, self.expected.size)
            .map_err(|_| Failure::SourceChanged)?
            .ok_or(Failure::SourceChanged)?;
        if identity(&named)? != identity(&self.file)?
            || !same_revision(
                &self.file.metadata().map_err(|_| Failure::SourceChanged)?,
                expected,
            )
            || !same_revision(
                &named.metadata().map_err(|_| Failure::SourceChanged)?,
                expected,
            )
        {
            return Err(Failure::SourceChanged);
        }
        Ok(())
    }
}
pub(super) struct Origin {
    bundle: PathBuf,
    root: PathBuf,
    uid: u32,
    bundle_pin: File,
    root_pin: File,
    artifacts: Vec<PinnedArtifact>,
    revisions: Vec<std::fs::Metadata>,
    first_operation: bool,
    context: NativeContext,
}
impl Origin {
    fn at(
        bundle: &Path,
        home: &Path,
        uid: u32,
        realm: Domain,
        artifacts: &'static [Artifact],
    ) -> Result<Self, Failure> {
        directory(home)?;
        let root = home.join("Library/Application Support/CodeBuddyExtension/Data/Public/auth");
        let bundle_pin = directory(bundle)?;
        let root_pin = directory(&root)?;
        let mut hash = Sha256::new();
        // Stable local scope only: no auth image, inode, account UID or secret.
        for bytes in [
            b"workbuddy-cn-macos-local-v1".as_slice(),
            &uid.to_be_bytes(),
            bundle.as_os_str().as_bytes(),
            root.as_os_str().as_bytes(),
        ] {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
        let mut pins = Vec::new();
        for artifact in artifacts {
            hash.update(artifact.path.as_bytes());
            hash.update(artifact.size.to_be_bytes());
            hash.update(artifact.sha256.as_bytes());
            pins.push(PinnedArtifact::open(bundle, artifact)?);
        }
        let mut origin = Self {
            bundle: bundle.into(),
            root,
            uid,
            bundle_pin,
            root_pin,
            artifacts: pins,
            revisions: Vec::new(),
            first_operation: true,
            context: NativeContext {
                product: "workbuddy".into(),
                region: "cn".into(),
                realm: realm.host().into(),
                source: hex::encode(hash.finalize()),
            },
        };
        // This complete read belongs to the first operation. Later operations
        // explicitly refresh once; no checkpoint may refresh its own baseline.
        origin.verify_with(&mut |_| {})?;
        Ok(origin)
    }
    fn verify_local(&self) -> Result<(), Failure> {
        // SAFETY: UID queries have no pointers or side effects.
        if unsafe { libc::geteuid() } != self.uid
            || unsafe { libc::getuid() } != self.uid
            || identity(&directory(&self.bundle)?)? != identity(&self.bundle_pin)?
            || identity(&directory(&self.root)?)? != identity(&self.root_pin)?
            || identity(&self.root_pin)?.2 != self.uid
        {
            return Err(Failure::SourceChanged);
        }
        let framework = self
            .bundle
            .join("Contents/Frameworks/Electron Framework.framework");
        for (path, expected) in LINKS {
            if std::fs::read_link(framework.join(path)).map_err(|_| Failure::SourceChanged)?
                != Path::new(expected)
            {
                return Err(Failure::SourceChanged);
            }
        }
        // Recovery admission checks metadata only; malformed/missing auth and a
        // logout marker remain the original journal/preimage owner's decision.
        for name in [
            "workbuddy-desktop.info",
            "workbuddy-desktop.info.logged-out",
        ] {
            match std::fs::symlink_metadata(self.root.join(name)) {
                Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.uid() == self.uid => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(Failure::SourceChanged),
            }
        }
        Ok(())
    }
}
impl super::sealed::Sealed for Origin {}
impl Origin {
    fn verify_with(&mut self, after_artifact: &mut dyn FnMut(usize)) -> Result<(), Failure> {
        self.revisions.clear();
        self.verify_local()?;
        let mut revisions = Vec::with_capacity(self.artifacts.len());
        for (index, artifact) in self.artifacts.iter().enumerate() {
            revisions.push(artifact.verify()?);
            after_artifact(index);
        }
        self.revisions = revisions;
        let checked = self.verify_unchanged();
        if checked.is_err() {
            self.revisions.clear();
        }
        checked
    }
}
impl ControlledSource for Origin {
    fn begin_operation(&mut self) -> Result<(), Failure> {
        if std::mem::take(&mut self.first_operation) {
            self.verify_unchanged()
        } else {
            self.verify_with(&mut |_| {})
        }
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn context(&self) -> &NativeContext {
        &self.context
    }
    fn verify_stopped(&mut self) -> Result<(), Failure> {
        self.verify_unchanged()
    }
    fn verify_unchanged(&mut self) -> Result<(), Failure> {
        self.verify_local()?;
        if self.revisions.len() != self.artifacts.len() {
            return Err(Failure::SourceChanged);
        }
        for (artifact, revision) in self.artifacts.iter().zip(&self.revisions) {
            artifact.verify_revision(revision)?;
        }
        self.verify_local()
    }
}
#[cfg(test)]
pub(super) fn verify_during(
    origin: &mut Origin,
    after_artifact: &mut dyn FnMut(usize),
) -> Result<(), Failure> {
    origin.verify_with(after_artifact)
}
#[cfg(test)]
pub(super) fn open_at(
    bundle: &Path,
    home: &Path,
    uid: u32,
    realm: Domain,
    artifacts: &'static [Artifact],
) -> Result<Origin, Failure> {
    Origin::at(bundle, home, uid, realm, artifacts)
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) type Source = process::ProcessBound<Origin, process::MacProcesses>;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn open(bundle: &Path, realm: Domain) -> Result<Source, Failure> {
    let (uid, home) = crate::zcode_accounts::native_context::native_user_home()
        .map_err(|_| Failure::SourceChanged)?;
    let origin = Origin::at(bundle, &home, uid, realm, ARTIFACTS)?;
    let scope = process::Scope::at(bundle, &bundle.join("Contents/MacOS/Electron"))?;
    let mut source = process::ProcessBound::new(origin, scope, process::MacProcesses);
    source.verify_stopped()?;
    Ok(source)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
pub(super) fn open_capture(bundle: &Path) -> Result<Source, Failure> {
    let (uid, home) = crate::zcode_accounts::native_context::native_user_home()
        .map_err(|_| Failure::SourceChanged)?;
    let mut origin = Origin::at(bundle, &home, uid, Domain::WorkBuddy, ARTIFACTS)?;
    let realm = crate::workbuddy::native_source::Observation::realm_at(
        &origin.root.join("workbuddy-desktop.info"),
    )?;
    origin.context.realm = realm.host().into();
    origin.verify_unchanged()?;
    let scope = process::Scope::at(bundle, &bundle.join("Contents/MacOS/Electron"))?;
    let mut source = process::ProcessBound::new(origin, scope, process::MacProcesses);
    source.verify_stopped()?;
    Ok(source)
}
