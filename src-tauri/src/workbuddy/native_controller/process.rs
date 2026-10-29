//! Read-only process admission, composed with an independently verified source.
//! Scope is a process locator, never source authentication. This module cannot
//! construct an origin, NativeContext, ReadPermit, snapshot, or Admission.
use super::{ControlledSource, Failure, NativeContext};
use std::{
    collections::BTreeSet,
    fs::File,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) mod sealed {
    pub trait Sealed {}
}
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Process {
    pub id: u32,
    /// Native BSD start seconds/microseconds; ephemeral, never a stored epoch.
    pub start_identity: [u64; 2],
    pub executable: PathBuf,
}
/// Ok means complete observation; partial/denied/truncated queries are errors.
/// The only system implementation is the bounded, read-only macOS kernel probe.
pub(super) trait Processes: sealed::Sealed {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure>;
}
const MAX_PROCESSES: usize = 4096;
const OBSERVATION_BUDGET: Duration = Duration::from_millis(500);

fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
fn physical_directory(path: &Path) -> Result<(), Failure> {
    if !absolute(path)
        || !crate::config_file_io::directory_exists_no_alias(path)
            .map_err(|_| Failure::UnsafePath)?
        || std::fs::canonicalize(path).map_err(|_| Failure::UnsafePath)? != path
    {
        return Err(Failure::UnsafePath);
    }
    Ok(())
}
#[derive(PartialEq, Eq)]
struct Physical {
    device: u64,
    inode: u64,
    uid: u32,
}
fn physical(file: &File) -> Result<Physical, Failure> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(|_| Failure::SourceChanged)?;
    Ok(Physical {
        device: m.dev(),
        inode: m.ino(),
        uid: m.uid(),
    })
}
/// Supplied by the independent source adapter; neither a bundle path nor its
/// main executable's name establishes official origin. Pin only the process
/// targets here, and retain them across prepare/apply/recover checks.
pub(super) struct Scope {
    bundle: PathBuf,
    executable: PathBuf,
    bundle_pin: File,
    executable_pin: File,
}
impl Scope {
    pub(super) fn at(bundle: &Path, executable: &Path) -> Result<Self, Failure> {
        use std::os::unix::fs::OpenOptionsExt;
        physical_directory(bundle)?;
        if !absolute(executable) || !executable.starts_with(bundle) {
            return Err(Failure::UnsafePath);
        }
        physical_directory(executable.parent().ok_or(Failure::UnsafePath)?)?;
        let bundle_pin = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(bundle)
            .map_err(|_| Failure::UnsafePath)?;
        let executable_pin = crate::config_file_io::open_regular_file(executable, u64::MAX)
            .map_err(|_| Failure::UnsafePath)?
            .ok_or(Failure::UnsafePath)?;
        let scope = Self {
            bundle: bundle.into(),
            executable: executable.into(),
            bundle_pin,
            executable_pin,
        };
        scope.verify()?;
        Ok(scope)
    }
    fn verify(&self) -> Result<(), Failure> {
        physical_directory(&self.bundle)?;
        physical_directory(self.executable.parent().ok_or(Failure::UnsafePath)?)?;
        let current = File::open(&self.bundle).map_err(|_| Failure::SourceChanged)?;
        let executable = crate::config_file_io::open_regular_file(&self.executable, u64::MAX)
            .map_err(|_| Failure::SourceChanged)?
            .ok_or(Failure::SourceChanged)?;
        if physical(&current)? != physical(&self.bundle_pin)?
            || physical(&executable)? != physical(&self.executable_pin)?
        {
            return Err(Failure::SourceChanged);
        }
        Ok(())
    }
    fn running(&self, path: &Path) -> Result<bool, Failure> {
        if !absolute(path) {
            return Err(Failure::SourceChanged);
        }
        // Main/helper executables in the selected bundle, and a copy of that
        // bundle/main in another location, must all be stopped. Resolve aliases
        // before comparing so a differently named symlink cannot evade the scope.
        let resolved = std::fs::canonicalize(path).map_err(|_| Failure::SourceChanged)?;
        // Metadata only: no executable contents or credential bytes are read.
        let file = crate::config_file_io::open_regular_file(&resolved, u64::MAX)
            .map_err(|_| Failure::SourceChanged)?
            .ok_or(Failure::SourceChanged)?;
        Ok(physical(&file)? == physical(&self.executable_pin)?
            || resolved.starts_with(&self.bundle)
            || resolved.file_name() == self.executable.file_name()
            || resolved.components().any(
                |c| matches!(c, Component::Normal(name) if Some(name) == self.bundle.file_name()),
            ))
    }
}
pub(super) struct ProcessBound<S, P> {
    source: S,
    scope: Scope,
    processes: P,
}
impl<S, P> ProcessBound<S, P> {
    pub(super) fn new(source: S, scope: Scope, processes: P) -> Self {
        Self {
            source,
            scope,
            processes,
        }
    }
}
impl<S: ControlledSource, P: Processes> super::sealed::Sealed for ProcessBound<S, P> {}
impl<S: ControlledSource, P: Processes> ControlledSource for ProcessBound<S, P> {
    fn begin_operation(&mut self) -> Result<(), Failure> {
        // Bulk content reads finish before the next native process observation.
        self.source.begin_operation()
    }
    fn root(&self) -> &Path {
        self.source.root()
    }
    fn context(&self) -> &NativeContext {
        self.source.context()
    }
    fn verify_stopped(&mut self) -> Result<(), Failure> {
        self.source.verify_stopped()?;
        self.scope.verify()?;
        // Retain both source checks. begin_operation finished bulk content IO;
        // checkpoints compare the retained revisions/pins without refreshing.
        self.source.verify_stopped()?;
        let rows = self.processes.snapshot()?;
        if rows.is_empty() || rows.len() >= MAX_PROCESSES {
            // A live current-user caller itself must be present. Empty is not
            // evidence of quiescence, even if a platform query returned success.
            return Err(Failure::SourceChanged);
        }
        let mut pids = BTreeSet::new();
        for row in rows {
            if row.id == 0 || !pids.insert(row.id) || self.scope.running(&row.executable)? {
                return Err(Failure::SourceChanged);
            }
        }
        self.scope.verify()?;
        self.source.verify_unchanged()
    }
    fn verify_unchanged(&mut self) -> Result<(), Failure> {
        self.scope.verify()?;
        self.source.verify_unchanged()
    }
}

trait Kernel {
    fn pids(&mut self) -> Result<Vec<u32>, Failure>;
    fn process(&mut self, pid: u32) -> Result<Process, Failure>;
}
fn pid_set(pids: Vec<u32>) -> Result<BTreeSet<u32>, Failure> {
    if pids.is_empty() || pids.len() >= MAX_PROCESSES {
        return Err(Failure::SourceChanged);
    }
    let mut result = BTreeSet::new();
    for pid in pids {
        if pid == 0 || !result.insert(pid) {
            return Err(Failure::SourceChanged);
        }
    }
    Ok(result)
}
fn snapshot(kernel: &mut impl Kernel, deadline: Instant) -> Result<Vec<Process>, Failure> {
    let pids = pid_set(kernel.pids()?)?;
    let mut result = Vec::with_capacity(pids.len());
    for &pid in &pids {
        if Instant::now() >= deadline {
            return Err(Failure::SourceChanged);
        }
        let process = kernel.process(pid)?;
        if process.id != pid || !absolute(&process.executable) {
            return Err(Failure::SourceChanged);
        }
        result.push(process);
    }
    // Stable membership alone cannot rule out reuse of an already sampled PID.
    // Re-observe every native start identity and executable path, under the same
    // total budget. A denied second query is unknown, never stopped.
    if pid_set(kernel.pids()?)? != pids {
        return Err(Failure::SourceChanged);
    }
    for before in &result {
        if Instant::now() >= deadline || kernel.process(before.id)? != *before {
            return Err(Failure::SourceChanged);
        }
    }
    // Preserve complete membership verification after both detail sweeps. This
    // is no OS exclusion lock: starts after observation still rely on the
    // original repeated write admission and pending journal recovery.
    if pid_set(kernel.pids()?)? != pids || Instant::now() >= deadline {
        return Err(Failure::SourceChanged);
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
pub(super) struct MacProcesses;
#[cfg(target_os = "macos")]
impl sealed::Sealed for MacProcesses {}
#[cfg(target_os = "macos")]
impl Processes for MacProcesses {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure> {
        snapshot(&mut MacKernel, Instant::now() + OBSERVATION_BUDGET)
    }
}
#[cfg(target_os = "macos")]
struct MacKernel;
#[cfg(target_os = "macos")]
impl Kernel for MacKernel {
    fn pids(&mut self) -> Result<Vec<u32>, Failure> {
        // SAFETY: UID queries take no pointers and change no state.
        let uid = unsafe { libc::geteuid() };
        if uid != unsafe { libc::getuid() } {
            return Err(Failure::SourceChanged);
        }
        crate::zcode_accounts::native_context::native_pids(uid)
            .map_err(|_| Failure::SourceChanged)?
            .into_iter()
            .map(|pid| u32::try_from(pid).map_err(|_| Failure::SourceChanged))
            .collect()
    }
    fn process(&mut self, pid: u32) -> Result<Process, Failure> {
        let raw_pid = i32::try_from(pid).map_err(|_| Failure::SourceChanged)?;
        // Reuse the original full-size BSD info, effective/real UID and process
        // start-identity checks around proc_pidpath; no second platform protocol.
        // SAFETY: geteuid takes no arguments or pointers.
        let uid = unsafe { libc::geteuid() };
        let (executable, start_identity) =
            crate::zcode_accounts::native_context::native_process_identity(raw_pid, uid)
                .map_err(|_| Failure::SourceChanged)?;
        Ok(Process {
            id: pid,
            start_identity,
            executable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixtureKernel {
        before: Vec<u32>,
        after: Vec<u32>,
        calls: usize,
        denied: bool,
        mismatch: bool,
    }
    impl Kernel for FixtureKernel {
        fn pids(&mut self) -> Result<Vec<u32>, Failure> {
            self.calls += 1;
            Ok(if self.calls == 1 {
                self.before.clone()
            } else {
                self.after.clone()
            })
        }
        fn process(&mut self, pid: u32) -> Result<Process, Failure> {
            if self.denied {
                return Err(Failure::SourceChanged);
            }
            Ok(Process {
                id: if self.mismatch { pid + 1 } else { pid },
                start_identity: [100, u64::from(pid)],
                executable: PathBuf::from("/synthetic/public/executable"),
            })
        }
    }
    fn kernel() -> FixtureKernel {
        FixtureKernel {
            before: vec![7, 8],
            after: vec![8, 7],
            calls: 0,
            denied: false,
            mismatch: false,
        }
    }
    #[test]
    fn process_snapshot_rejects_partial_denied_changed_and_truncated_observations() {
        for case in [
            "empty",
            "zero",
            "duplicate",
            "capacity",
            "denied",
            "identity",
            "new-pid",
            "exited-pid",
            "budget",
        ] {
            let mut k = kernel();
            let mut deadline = Instant::now() + OBSERVATION_BUDGET;
            match case {
                "empty" => k.before.clear(),
                "zero" => k.before = vec![0],
                "duplicate" => k.before = vec![7, 7],
                "capacity" => k.before = (1..=MAX_PROCESSES as u32).collect(),
                "denied" => k.denied = true,
                "identity" => k.mismatch = true,
                "new-pid" => k.after.push(9),
                "exited-pid" => {
                    k.after.pop().unwrap();
                }
                _ => deadline = Instant::now(),
            }
            assert!(snapshot(&mut k, deadline).is_err(), "admitted {case}");
        }
        assert_eq!(
            snapshot(&mut kernel(), Instant::now() + OBSERVATION_BUDGET)
                .unwrap()
                .len(),
            2
        );
    }
    enum Change {
        Start,
        Path,
        Denied,
    }
    struct ReusedPidKernel {
        change: Change,
        samples: std::collections::BTreeMap<u32, usize>,
    }
    impl Kernel for ReusedPidKernel {
        fn pids(&mut self) -> Result<Vec<u32>, Failure> {
            Ok(vec![7, 8])
        }
        fn process(&mut self, pid: u32) -> Result<Process, Failure> {
            let count = self.samples.entry(pid).or_default();
            *count += 1;
            let mut row = Process {
                id: pid,
                start_identity: [100, 1],
                executable: PathBuf::from("/synthetic/unrelated-executable"),
            };
            if pid == 7 && *count > 1 {
                match self.change {
                    Change::Start => row.start_identity = [100, 2],
                    Change::Path => row.executable = PathBuf::from("/synthetic/WorkBuddy.app/Contents/Frameworks/Renderer.app/Contents/MacOS/Renderer"),
                    Change::Denied => return Err(Failure::SourceChanged),
                }
            }
            Ok(row)
        }
    }
    fn reused(change: Change) -> ReusedPidKernel {
        ReusedPidKernel {
            change,
            samples: Default::default(),
        }
    }
    #[test]
    fn process_snapshot_reuse_refuses_same_pid_with_different_start_identity() {
        let mut k = reused(Change::Start);
        assert!(
            matches!(
                snapshot(&mut k, Instant::now() + OBSERVATION_BUDGET),
                Err(Failure::SourceChanged)
            ),
            "same PID set concealed changed start identity"
        );
    }
    #[test]
    fn process_snapshot_reuse_refuses_same_pid_with_different_executable_path() {
        let mut k = reused(Change::Path);
        assert!(
            matches!(
                snapshot(&mut k, Instant::now() + OBSERVATION_BUDGET),
                Err(Failure::SourceChanged)
            ),
            "same PID set concealed unrelated -> target helper path"
        );
    }
    #[test]
    fn process_snapshot_reuse_refuses_second_identity_query_failure() {
        let mut k = reused(Change::Denied);
        assert!(
            matches!(
                snapshot(&mut k, Instant::now() + OBSERVATION_BUDGET),
                Err(Failure::SourceChanged)
            ),
            "failed second identity observation was treated as stopped"
        );
    }

    fn install(root: &Path) -> (PathBuf, PathBuf) {
        let bundle = root.join("SyntheticWorkBuddy.app");
        let executable = bundle.join("Contents/MacOS/SyntheticWorkBuddy");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"synthetic fixture; never executed").unwrap();
        (bundle, executable)
    }
    fn file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"synthetic fixture; never executed").unwrap();
    }
    #[test]
    fn process_scope_covers_helpers_copies_and_aliases_without_claiming_provenance() {
        let root = crate::secrets::testing::tempdir().unwrap();
        let (bundle, executable) = install(root.path());
        let scope = Scope::at(&bundle, &executable).unwrap();
        assert!(scope.running(&executable).unwrap());
        for path in [
            bundle.join("Contents/Frameworks/Renderer.app/Contents/MacOS/arbitrary-renderer"),
            root.path()
                .join("another/SyntheticWorkBuddy.app/Contents/MacOS/arbitrary-renderer"),
            root.path()
                .join("Other.app/Contents/MacOS/SyntheticWorkBuddy"),
        ] {
            file(&path);
            assert!(scope.running(&path).unwrap());
        }
        let alias = root.path().join("renamed-alias");
        std::os::unix::fs::symlink(&executable, &alias).unwrap();
        assert!(scope.running(&alias).unwrap());
        let other = root.path().join("Other.app/Contents/MacOS/Unrelated");
        file(&other);
        assert!(!scope.running(&other).unwrap());
        assert!(scope.running(Path::new("relative/unknown")).is_err());
        assert!(scope.running(&root.path().join("missing/unknown")).is_err());
    }
    #[test]
    fn process_scope_refuses_physical_replacements_and_unsafe_locators() {
        for replace_bundle in [true, false] {
            let root = crate::secrets::testing::tempdir().unwrap();
            let (bundle, executable) = install(root.path());
            let scope = Scope::at(&bundle, &executable).unwrap();
            if replace_bundle {
                std::fs::rename(&bundle, root.path().join("old-app")).unwrap();
                install(root.path());
            } else {
                std::fs::rename(&executable, executable.with_extension("old")).unwrap();
                file(&executable);
            }
            assert!(scope.verify().is_err());
        }
        let root = crate::secrets::testing::tempdir().unwrap();
        let (bundle, executable) = install(root.path());
        assert!(Scope::at(&bundle, Path::new("relative")).is_err());
        assert!(Scope::at(root.path(), &executable).is_ok()); // locator only, not origin proof
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&bundle, &alias).unwrap();
        assert!(Scope::at(&alias, &alias.join("Contents/MacOS/SyntheticWorkBuddy")).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_probe_is_available_without_invoking_or_inspecting_a_client() {
        let _probe = MacProcesses;
    }
}
