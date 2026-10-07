//! Read-only native installation, settings and writer observations for one operation.
//! The renderer chooses a source; only the backend supplies OS identity and evidence.
use super::admission::{BlockedReason, ContextObservation, ContextProbe, ContractEntry};
#[cfg(any(target_os = "macos", all(test, unix)))]
use super::admission::{BuildFingerprint, Platform, WriterState};
use serde::Deserialize;
#[cfg(any(target_os = "macos", all(test, unix)))]
use sha2::{Digest, Sha256};
use std::path::PathBuf;
#[cfg(any(target_os = "macos", all(test, unix)))]
use std::{
    fs::{self, File},
    io::Read,
    path::Path,
    sync::Mutex,
};
#[cfg(any(target_os = "macos", all(test, unix)))]
use zeroize::Zeroizing;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ContextSelection {
    pub install_path: PathBuf,
    pub data_root: PathBuf,
    pub key_mode: KeyMode,
}
pub(crate) use super::admission::KeyMode;
#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone, PartialEq, Eq)]
struct OsUser {
    home: String,
    username: String,
    uid: u32,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(Clone)]
struct SystemFacts {
    user: OsUser,
    writers: WriterState,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
struct ArtifactContract {
    relative_path: &'static str,
    sha256: [u8; 32],
    exact_size: Option<u64>,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
struct InstalledContract {
    artifacts: [ArtifactContract; 3],
}
#[cfg(any(target_os = "macos", all(test, unix)))]
impl InstalledContract {
    fn fingerprint(&self) -> BuildFingerprint {
        // Fixed-order length framing binds each path to its complete file hash.
        let mut hash = Sha256::new();
        hash.update(b"zcode-installed-artifacts-v1\0");
        for artifact in &self.artifacts {
            hash.update((artifact.relative_path.len() as u64).to_be_bytes());
            hash.update(artifact.relative_path.as_bytes());
            hash.update(32u64.to_be_bytes());
            hash.update(artifact.sha256);
        }
        BuildFingerprint {
            platform: Platform::MacOs,
            version: "3.14.4".into(),
            build: "3.14.4.7912".into(),
            artifact_sha256: hash.finalize().into(),
        }
    }
}
#[cfg(any(target_os = "macos", all(test, unix)))]
fn installed_contract() -> InstalledContract {
    // Exact installed public artifacts reported 2026-10-03. Matching Info.plist
    // bytes bind version/build; no permissive plist or ASAR parser is involved.
    InstalledContract {
        artifacts: [
            ArtifactContract {
                relative_path: "Contents/Resources/app.asar",
                sha256: [
                    0x23, 0x2e, 0x91, 0x3e, 0xa1, 0x3d, 0x60, 0xbd, 0x0e, 0xcc, 0x86, 0xbf, 0x9f,
                    0x2f, 0x14, 0x53, 0x28, 0x80, 0x96, 0x08, 0xfe, 0x4d, 0x48, 0xe7, 0x66, 0x85,
                    0xd6, 0x1a, 0x80, 0x76, 0xae, 0xf0,
                ],
                exact_size: Some(326_914_570),
            },
            ArtifactContract {
                relative_path: "Contents/MacOS/ZCode",
                sha256: [
                    0x2c, 0x17, 0xd1, 0x38, 0x64, 0xc9, 0xec, 0x0b, 0xbd, 0x57, 0x44, 0x30, 0xd9,
                    0xe9, 0xc3, 0xde, 0xc9, 0x0b, 0x70, 0xd6, 0x80, 0xb6, 0xe5, 0x28, 0x92, 0x3c,
                    0xba, 0xb2, 0x2d, 0x86, 0xe6, 0x1a,
                ],
                exact_size: None,
            },
            ArtifactContract {
                relative_path: "Contents/Info.plist",
                sha256: [
                    0x4e, 0xf6, 0xbc, 0x5c, 0xce, 0x05, 0xae, 0xa1, 0xfe, 0x0c, 0x13, 0x29, 0xda,
                    0xca, 0xcc, 0x46, 0xaf, 0x7e, 0xaa, 0x97, 0xa5, 0xc2, 0xa7, 0x95, 0x69, 0x4c,
                    0xa6, 0xa2, 0x1b, 0x3d, 0x03, 0xf8,
                ],
                exact_size: None,
            },
        ],
    }
}
#[cfg(any(target_os = "macos", all(test, unix)))]
pub(super) fn supported_contracts() -> Vec<ContractEntry> {
    vec![ContractEntry {
        fingerprint: installed_contract().fingerprint(),
        // Exact macOS contract verified by the native/main/UI gate on 2026-10-04.
        // Other platforms have no accepted native writer/probe implementation.
        native_gate_passed: cfg!(target_os = "macos"),
    }]
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn verified_artifacts(
    install_path: &Path,
    manifest: &InstalledContract,
) -> Result<Vec<ObservedFile>, BlockedReason> {
    require_canonical(install_path).map_err(|_| BlockedReason::UnsupportedBuild)?;
    let mut artifacts = Vec::with_capacity(3);
    for (index, expected) in manifest.artifacts.iter().enumerate() {
        let max_size = [512 * 1024 * 1024, 128 * 1024 * 1024, 1024 * 1024][index];
        let mut observed = ObservedFile::open(install_path.join(expected.relative_path), max_size)
            .map_err(|_| BlockedReason::UnsupportedBuild)?;
        if expected
            .exact_size
            .is_some_and(|size| size != observed.stamp.size)
        {
            return Err(BlockedReason::UnsupportedBuild);
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut read = 0u64;
        loop {
            let count = observed
                .file
                .read(&mut buffer)
                .map_err(|_| BlockedReason::UnsupportedBuild)?;
            if count == 0 {
                break;
            }
            read += count as u64;
            if read > observed.stamp.size {
                return Err(BlockedReason::ContextChanged);
            }
            hash.update(&buffer[..count]);
        }
        observed
            .recheck()
            .map_err(|_| BlockedReason::ContextChanged)?;
        let actual: [u8; 32] = hash.finalize().into();
        if read != observed.stamp.size || actual != expected.sha256 {
            return Err(BlockedReason::UnsupportedBuild);
        }
        artifacts.push(observed);
    }
    Ok(artifacts)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataCandidate {
    install_path: PathBuf,
    version: Option<String>,
    build: Option<String>,
    verified_build: bool,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MetadataDiscovery {
    data_root: PathBuf,
    source_basis: &'static str,
    candidates: Vec<MetadataCandidate>,
    latest_status: &'static str,
}
/// Informational version of one actually observed matching installation. An
/// absent/ambiguous observation never substitutes a fabricated API version.
pub(super) fn library_app_version(data_root: &std::path::Path) -> Option<String> {
    let metadata = discover_metadata(None).ok()?;
    if metadata.data_root != data_root || metadata.candidates.len() != 1 {
        return None;
    }
    let candidate = metadata.candidates.into_iter().next()?;
    if candidate.verified_build {
        candidate.version
    } else {
        None
    }
}
/// OS identity and a directory label only; no installation, process, native
/// credentials or current provider is inspected for vault-only account work.
#[cfg(target_os = "macos")]
pub(super) fn library_context(
    selected: Option<&Path>,
) -> Result<super::library_context::LibraryContext, BlockedReason> {
    library_from_user(&native_user()?, selected)
}
#[cfg(not(target_os = "macos"))]
pub(super) fn library_context(
    _selected: Option<&std::path::Path>,
) -> Result<super::library_context::LibraryContext, BlockedReason> {
    Err(BlockedReason::UnsupportedPlatform)
}
#[cfg(any(target_os = "macos", all(test, unix)))]
fn library_from_user(
    user: &OsUser,
    selected: Option<&Path>,
) -> Result<super::library_context::LibraryContext, BlockedReason> {
    let data_root = match selected {
        Some(path) => path.to_owned(),
        None => discover_from_candidates(user, &[], &installed_contract())?.data_root,
    };
    super::library_context::LibraryContext::from_os_identity(&user.home, &user.username, &data_root)
}

#[cfg(target_os = "macos")]
pub(super) fn discover_metadata(
    selected: Option<&Path>,
) -> Result<MetadataDiscovery, BlockedReason> {
    let user = native_user()?;
    let candidates =
        super::discovery_paths::installation_candidates(Path::new(&user.home), selected)
            .map_err(|_| BlockedReason::RootUnverified)?;
    discover_from_candidates(&user, &candidates, &installed_contract())
}
#[cfg(not(target_os = "macos"))]
pub(super) fn discover_metadata(
    _selected: Option<&std::path::Path>,
) -> Result<MetadataDiscovery, BlockedReason> {
    Err(BlockedReason::UnsupportedPlatform)
}
#[cfg(any(target_os = "macos", all(test, unix)))]
fn discover_from_candidates(
    user: &OsUser,
    candidates: &[PathBuf],
    manifest: &InstalledContract,
) -> Result<MetadataDiscovery, BlockedReason> {
    require_canonical(Path::new(&user.home)).map_err(|_| BlockedReason::RootUnverified)?;
    fn present_base<'de, D: serde::Deserializer<'de>>(
        input: D,
    ) -> Result<Option<String>, D::Error> {
        String::deserialize(input).map(Some)
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Bootstrap {
        #[serde(default, deserialize_with = "present_base")]
        data_base_dir: Option<String>,
    }
    let path = Path::new(&user.home).join(".zcode/v2/setting.json");
    let bootstrap = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(BlockedReason::SettingsInvalid),
        Ok(_) => {
            let mut opened = ObservedFile::open(path, 1024 * 1024)
                .map_err(|_| BlockedReason::SettingsInvalid)?;
            let mut bytes = Zeroizing::new(Vec::new());
            (&mut opened.file)
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| BlockedReason::SettingsInvalid)?;
            if bytes.len() as u64 != opened.stamp.size || bytes.len() > 1024 * 1024 {
                return Err(BlockedReason::SettingsInvalid);
            }
            opened
                .recheck()
                .map_err(|_| BlockedReason::ContextChanged)?;
            Some(
                serde_json::from_slice::<Bootstrap>(&bytes)
                    .map_err(|_| BlockedReason::SettingsInvalid)?,
            )
        }
    };
    let base = bootstrap
        .as_ref()
        .and_then(|value| value.data_base_dir.as_deref());
    let data_root = super::discovery_paths::account_directory(Path::new(&user.home), base)
        .map_err(|_| BlockedReason::SettingsInvalid)?;
    let mut result = Vec::with_capacity(candidates.len());
    for path in candidates {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(BlockedReason::UnsupportedBuild),
            Ok(_) => {}
        }
        let verified_build = verified_artifacts(path, manifest).is_ok();
        let fingerprint = manifest.fingerprint();
        let (version, build) = if verified_build {
            (Some(fingerprint.version), Some(fingerprint.build))
        } else {
            public_version_hint(path).unwrap_or_default()
        };
        result.push(MetadataCandidate {
            install_path: path.clone(),
            version,
            build,
            verified_build,
        });
    }
    Ok(MetadataDiscovery {
        data_root,
        source_basis: if base.is_some() {
            "bootstrapDataBaseDir"
        } else {
            "osAccountHome"
        },
        candidates: result,
        latest_status: "notQueried",
    })
}

// XML labels are informational only. They never select a compatible contract.
#[cfg(any(target_os = "macos", all(test, unix)))]
fn public_version_hint(install: &Path) -> Option<(Option<String>, Option<String>)> {
    let mut opened = ObservedFile::open(install.join("Contents/Info.plist"), 1024 * 1024).ok()?;
    let mut bytes = Vec::new();
    (&mut opened.file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 != opened.stamp.size {
        return None;
    }
    opened.recheck().ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let scalar = |key: &str| -> Option<String> {
        let marker = format!("<key>{key}</key>");
        if text.matches(&marker).count() != 1 {
            return None;
        }
        let (_, rest) = text.split_once(&marker)?;
        let value = rest
            .trim_start()
            .strip_prefix("<string>")?
            .split_once("</string>")?
            .0;
        if value.is_empty()
            || value.len() > 32
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        {
            return None;
        }
        Some(value.to_owned())
    };
    Some((
        scalar("CFBundleShortVersionString"),
        scalar("CFBundleVersion"),
    ))
}

#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(PartialEq, Eq)]
struct FileStamp {
    identity: [u64; 2],
    size: u64,
    modified: [i64; 2],
    changed: [i64; 2],
    mode: u32,
    owner: u32,
    links: u64,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
fn file_stamp(meta: &fs::Metadata) -> FileStamp {
    use std::os::unix::fs::MetadataExt;
    FileStamp {
        identity: [meta.dev(), meta.ino()],
        size: meta.len(),
        modified: [meta.mtime(), meta.mtime_nsec()],
        changed: [meta.ctime(), meta.ctime_nsec()],
        mode: meta.mode(),
        owner: meta.uid(),
        links: meta.nlink(),
    }
}
#[cfg(any(target_os = "macos", all(test, unix)))]
struct ObservedFile {
    path: PathBuf,
    file: File,
    stamp: FileStamp,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
impl ObservedFile {
    #[cfg(unix)]
    fn open(path: PathBuf, max_size: u64) -> Result<Self, ()> {
        use std::os::unix::fs::OpenOptionsExt;
        require_canonical(&path)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| ())?;
        let meta = file.metadata().map_err(|_| ())?;
        if !meta.is_file() || meta.len() > max_size || file_stamp(&meta).links != 1 {
            return Err(());
        }
        let observed = Self {
            path,
            file,
            stamp: file_stamp(&meta),
        };
        observed.recheck()?;
        Ok(observed)
    }
    #[cfg(unix)]
    fn recheck(&self) -> Result<(), ()> {
        require_canonical(&self.path)?;
        let opened = self.file.metadata().map_err(|_| ())?;
        let named = fs::symlink_metadata(&self.path).map_err(|_| ())?;
        if !named.is_file()
            || named.file_type().is_symlink()
            || file_stamp(&opened) != self.stamp
            || file_stamp(&named) != self.stamp
        {
            return Err(());
        }
        Ok(())
    }
}
#[cfg(any(target_os = "macos", all(test, unix)))]
fn is_local_mount(flags: u32) -> bool {
    // Darwin MNT_LOCAL; remote PID ownership cannot satisfy the native lock contract.
    flags & 0x0000_1000 != 0
}
#[cfg(target_os = "macos")]
fn native_local_root(root: &File) -> Result<(), BlockedReason> {
    use std::os::fd::AsRawFd;
    let mut filesystem: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(root.as_raw_fd(), &mut filesystem) } != 0
        || !is_local_mount(filesystem.f_flags)
    {
        return Err(BlockedReason::RootUnverified);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn require_canonical(path: &Path) -> Result<(), ()> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || fs::canonicalize(path).map_err(|_| ())? != path
    {
        return Err(());
    }
    Ok(())
}

#[cfg(any(target_os = "macos", all(test, unix)))]
pub(super) struct NativeContextProbe {
    selection: ContextSelection,
    user: OsUser,
    fingerprint: BuildFingerprint,
    artifacts: Vec<ObservedFile>,
    root_identity: [u64; 2],
    // Keep the root inode alive while later checks compare dev/ino.
    _root: File,
    // Keep the previous settings inode alive until the next observation is read.
    settings: Mutex<Option<ObservedFile>>,
    #[cfg(test)]
    test_facts: Mutex<Option<Result<SystemFacts, BlockedReason>>>,
}
#[cfg(any(target_os = "macos", all(test, unix)))]
impl NativeContextProbe {
    #[cfg(target_os = "macos")]
    pub(super) fn new(selection: ContextSelection) -> Result<Self, BlockedReason> {
        Self::from_parts(selection, native_user()?, &installed_contract())
    }
    #[cfg(not(target_os = "macos"))]
    pub(super) fn new(_selection: ContextSelection) -> Result<Self, BlockedReason> {
        Err(BlockedReason::UnsupportedPlatform)
    }
    fn from_parts(
        selection: ContextSelection,
        user: OsUser,
        manifest: &InstalledContract,
    ) -> Result<Self, BlockedReason> {
        match selection.key_mode {
            KeyMode::Custom => return Err(BlockedReason::CustomKeyContext),
            KeyMode::Unknown => return Err(BlockedReason::KeyContextUnknown),
            KeyMode::Standard => {}
        }
        if user.username.is_empty() || user.username.contains('\0') || user.home.contains('\0') {
            return Err(BlockedReason::KeyContextUnknown);
        }
        require_canonical(Path::new(&user.home)).map_err(|_| BlockedReason::RootUnverified)?;
        let root_identity = super::transaction::native_root_identity(&selection.data_root)
            .map_err(|_| BlockedReason::RootUnverified)?;
        let root = File::open(&selection.data_root).map_err(|_| BlockedReason::RootUnverified)?;
        #[cfg(unix)]
        if file_stamp(&root.metadata().map_err(|_| BlockedReason::RootUnverified)?).identity
            != root_identity
        {
            return Err(BlockedReason::ContextChanged);
        }
        #[cfg(target_os = "macos")]
        native_local_root(&root)?;
        let artifacts = verified_artifacts(&selection.install_path, manifest)?;
        let probe = Self {
            selection,
            user,
            fingerprint: manifest.fingerprint(),
            artifacts,
            root_identity,
            _root: root,
            settings: Mutex::new(None),
            #[cfg(test)]
            test_facts: Mutex::new(None),
        };
        probe.recheck_files()?;
        Ok(probe)
    }
    fn recheck_files(&self) -> Result<(), BlockedReason> {
        #[cfg(target_os = "macos")]
        native_local_root(&self._root)?;
        for artifact in &self.artifacts {
            artifact
                .recheck()
                .map_err(|_| BlockedReason::ContextChanged)?;
        }
        if super::transaction::native_root_identity(&self.selection.data_root)
            .map_err(|_| BlockedReason::ContextChanged)?
            != self.root_identity
        {
            return Err(BlockedReason::ContextChanged);
        }
        Ok(())
    }
    fn system_facts(&self) -> Result<SystemFacts, BlockedReason> {
        #[cfg(test)]
        if let Some(facts) = self
            .test_facts
            .lock()
            .map_err(|_| BlockedReason::ContextChanged)?
            .as_ref()
        {
            return facts.clone();
        }
        #[cfg(target_os = "macos")]
        {
            let user = native_user()?;
            let writers = native_writers(&self.selection.install_path, user.uid);
            Ok(SystemFacts { user, writers })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(BlockedReason::UnsupportedPlatform)
        }
    }
}
#[cfg(any(target_os = "macos", all(test, unix)))]
impl ContextProbe for NativeContextProbe {
    #[cfg(all(feature = "gui", target_os = "macos"))]
    fn open_for_login(&self) -> Result<(), BlockedReason> {
        super::native_process_control::open(self, &self.selection)
    }
    #[cfg(all(feature = "gui", target_os = "macos"))]
    fn prepare_switch(&self) -> Result<bool, BlockedReason> {
        super::native_process_control::stop(self, &self.selection)
    }
    #[cfg(all(feature = "gui", target_os = "macos"))]
    fn restart_after_switch(&self) -> Result<(), BlockedReason> {
        super::native_process_control::restart(self, &self.selection)
    }
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        let facts = self.system_facts()?;
        if facts.user != self.user {
            return Err(BlockedReason::ContextChanged);
        }
        self.recheck_files()?;
        let settings_file = Path::new(&self.user.home).join(".zcode/v2/setting.json");
        let mut held_settings = self
            .settings
            .lock()
            .map_err(|_| BlockedReason::ContextChanged)?;
        let mut settings = ObservedFile::open(settings_file.clone(), 1024 * 1024)
            .map_err(|_| BlockedReason::SettingsInvalid)?;
        let mut bytes = Zeroizing::new(Vec::new());
        (&mut settings.file)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| BlockedReason::SettingsInvalid)?;
        if bytes.len() > 1024 * 1024 || bytes.len() as u64 != settings.stamp.size {
            return Err(BlockedReason::SettingsInvalid);
        }
        settings
            .recheck()
            .map_err(|_| BlockedReason::ContextChanged)?;
        self.recheck_files()?;
        let settings_identity = settings.stamp.identity;
        *held_settings = Some(settings);
        Ok(ContextObservation {
            install: self.fingerprint.clone(),
            credential_root: self.selection.data_root.clone(),
            settings_file,
            root_identity: self.root_identity,
            settings_identity,
            home: self.user.home.clone(),
            settings_home: self.user.home.clone(),
            bootstrap_home: self.user.home.clone(),
            username: self.user.username.clone(),
            key_choice: KeyMode::Standard,
            writers: facts.writers,
            settings: std::mem::take(&mut *bytes),
        })
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn classify_executable(install: &Path, executable: &Path) -> WriterState {
    let Some(name) = executable.file_name().and_then(|name| name.to_str()) else {
        return WriterState::Unknown;
    };
    let name = name.to_ascii_lowercase();
    if !executable.is_absolute() {
        return WriterState::Unknown;
    }
    if executable.starts_with(install)
        || name.starts_with("zcode")
        || executable.components().any(|part| {
            part.as_os_str()
                .to_str()
                .is_some_and(|part| part.eq_ignore_ascii_case("ZCode.app"))
        })
    {
        WriterState::Running
    } else if matches!(
        name.as_str(),
        "node" | "nodejs" | "bun" | "deno" | "electron"
    ) {
        // An executable path cannot prove which script a shared runtime is running.
        WriterState::Unknown
    } else {
        WriterState::Stopped
    }
}

#[cfg(target_os = "macos")]
fn native_user() -> Result<OsUser, BlockedReason> {
    use std::ffi::CStr;
    let uid = unsafe { libc::geteuid() };
    if uid != unsafe { libc::getuid() } {
        return Err(BlockedReason::KeyContextUnknown);
    }
    let mut size = 16 * 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if status != 0
            || result.is_null()
            || entry.pw_uid != uid
            || entry.pw_dir.is_null()
            || entry.pw_name.is_null()
        {
            return Err(BlockedReason::KeyContextUnknown);
        }
        let home = unsafe { CStr::from_ptr(entry.pw_dir) }
            .to_str()
            .map_err(|_| BlockedReason::KeyContextUnknown)?
            .to_owned();
        let username = unsafe { CStr::from_ptr(entry.pw_name) }
            .to_str()
            .map_err(|_| BlockedReason::KeyContextUnknown)?
            .to_owned();
        return Ok(OsUser {
            home,
            username,
            uid,
        });
    }
}

#[cfg(target_os = "macos")]
pub(super) fn native_pids(uid: u32) -> Result<Vec<i32>, ()> {
    // Apple xnu bsd/sys/proc_info.h: PROC_UID_ONLY = 4 (2 is PROC_PGRP_ONLY).
    // https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/proc_info.h
    // A full buffer cannot prove completeness.
    const PROC_UID_ONLY: u32 = 4;
    let mut pids = vec![0i32; 32_768];
    let capacity = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let bytes =
        unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, pids.as_mut_ptr().cast(), capacity) };
    if bytes <= 0 || bytes >= capacity || bytes % 4 != 0 {
        return Err(());
    }
    pids.truncate(bytes as usize / 4);
    pids.retain(|pid| *pid > 0);
    pids.sort_unstable();
    if pids.is_empty() || pids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(());
    }
    Ok(pids)
}
#[cfg(target_os = "macos")]
fn native_process_path(pid: i32, uid: u32) -> Result<PathBuf, ()> {
    native_process_identity(pid, uid).map(|(path, _)| path)
}
#[cfg(target_os = "macos")]
pub(super) fn native_process_identity(pid: i32, uid: u32) -> Result<(PathBuf, [u64; 2]), ()> {
    use std::os::unix::ffi::OsStrExt;
    fn identity(pid: i32, uid: u32) -> Result<(u64, u64), ()> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if read != size || info.pbi_pid != pid as u32 || info.pbi_uid != uid || info.pbi_ruid != uid
        {
            return Err(());
        }
        Ok((info.pbi_start_tvsec, info.pbi_start_tvusec))
    }
    let before = identity(pid, uid)?;
    let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let read = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if read <= 0 || read as usize >= buffer.len() {
        return Err(());
    }
    let end = buffer.iter().position(|byte| *byte == 0).ok_or(())?;
    if end == 0 || before != identity(pid, uid)? {
        return Err(());
    }
    Ok((
        PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..end])),
        [before.0, before.1],
    ))
}
#[cfg(target_os = "macos")]
fn native_writers(install: &Path, uid: u32) -> WriterState {
    fn snapshot(install: &Path, uid: u32) -> Result<WriterState, ()> {
        let before = native_pids(uid)?;
        let mut state = WriterState::Stopped;
        for pid in &before {
            match classify_executable(install, &native_process_path(*pid, uid)?) {
                WriterState::Running => return Ok(WriterState::Running),
                WriterState::Unknown => state = WriterState::Unknown,
                WriterState::Stopped => {}
            }
        }
        if before != native_pids(uid)? {
            return Err(());
        }
        Ok(state)
    }
    // One bounded retry for churn; incomplete/error evidence never means stopped.
    for _ in 0..2 {
        if let Ok(state) = snapshot(install, uid) {
            return state;
        }
    }
    WriterState::Unknown
}

#[cfg(not(any(target_os = "macos", all(test, unix))))]
pub(super) fn supported_contracts() -> Vec<ContractEntry> {
    Vec::new()
}
#[cfg(not(any(target_os = "macos", all(test, unix))))]
pub(super) struct NativeContextProbe;
#[cfg(not(any(target_os = "macos", all(test, unix))))]
impl NativeContextProbe {
    pub(super) fn new(selection: ContextSelection) -> Result<Self, BlockedReason> {
        // Consume the untrusted DTO without touching the selected filesystem.
        drop((
            selection.install_path,
            selection.data_root,
            selection.key_mode,
        ));
        Err(BlockedReason::UnsupportedPlatform)
    }
}
#[cfg(not(any(target_os = "macos", all(test, unix))))]
impl ContextProbe for NativeContextProbe {
    fn observe(&self) -> Result<ContextObservation, BlockedReason> {
        Err(BlockedReason::UnsupportedPlatform)
    }
}

#[cfg(all(test, unix))]
#[path = "native_context_tests.rs"]
mod tests;

#[cfg(all(test, not(unix)))]
#[test]
fn unsupported_platform_facade_rejects_without_reading_any_selected_path() {
    assert!(supported_contracts().is_empty());
    let selected = ContextSelection {
        install_path: PathBuf::from("this-path-must-not-be-opened"),
        data_root: PathBuf::new(),
        key_mode: KeyMode::Custom,
    };
    assert!(matches!(
        NativeContextProbe::new(selected),
        Err(BlockedReason::UnsupportedPlatform)
    ));
    assert!(matches!(
        NativeContextProbe.observe(),
        Err(BlockedReason::UnsupportedPlatform)
    ));
}
