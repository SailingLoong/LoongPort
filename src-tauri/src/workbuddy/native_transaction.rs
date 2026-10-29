//! Fixed two-file transaction core, not registered or connected to native clients.
//! JournalOwner must provide durable protected storage and exclusive internal ownership.
//! Official clients do not share that lock: stopped admission and image comparisons
//! detect changes but do NOT promise atomic CAS against an uncooperative process.
use super::native::NativeContext;
use std::path::{Component, Path, PathBuf};
use zeroize::Zeroizing;

const LIMIT: u64 = 4 * 1024 * 1024;
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Failure {
    UnsafePath,
    // Remove when the concrete Files IO backend has a runtime transaction caller.
    #[cfg_attr(not(test), allow(dead_code))]
    Io,
    Journal,
    SourceChanged,
    RecoveryRequired,
    RequestConflict,
    RolledBack,
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Image {
    auth: Option<Zeroizing<Vec<u8>>>,
    marker: Option<Zeroizing<Vec<u8>>>,
}
impl Image {
    #[cfg(unix)]
    pub(super) fn auth_bytes(&self) -> Option<&[u8]> {
        self.auth.as_deref().map(|bytes| bytes.as_slice())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Prepared,
    Committed,
    Restored,
}
#[derive(Clone)]
pub(crate) struct Journal {
    admission: Stamp,
    version: u32,
    request: String,
    root: PathBuf,
    root_identity: RootIdentity,
    context: NativeContext,
    before: Image,
    after: Image,
    phase: Phase,
}
impl Journal {
    #[cfg(all(unix, any(test, feature = "gui")))]
    pub(super) fn realm(&self) -> &str {
        &self.context.realm
    }
}
#[derive(Clone)]
pub(crate) struct Paths {
    root: PathBuf,
    auth: PathBuf,
    marker: PathBuf,
}
impl Paths {
    // Remove when the native profile discovery adapter constructs validated paths.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn at(root: &Path) -> Result<Self, Failure> {
        let paths = Self {
            root: root.into(),
            auth: root.join("workbuddy-desktop.info"),
            marker: root.join("workbuddy-desktop.info.logged-out"),
        };
        paths.validate()?;
        Ok(paths)
    }
    fn validate(&self) -> Result<(), Failure> {
        if !self.root.is_absolute()
            || self.root.components().any(|c| {
                !matches!(
                    c,
                    Component::RootDir | Component::Normal(_) | Component::Prefix(_)
                )
            })
            || !crate::config_file_io::directory_exists_no_alias(&self.root)
                .map_err(|_| Failure::UnsafePath)?
            || std::fs::canonicalize(&self.root).map_err(|_| Failure::UnsafePath)? != self.root
        {
            return Err(Failure::UnsafePath);
        }
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq)]
struct RootIdentity {
    device: u64,
    inode: u64,
    uid: u32,
}
fn bind_root(paths: &Paths) -> Result<(RootIdentity, std::fs::File), Failure> {
    paths.validate()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&paths.root)
            .map_err(|_| Failure::UnsafePath)?;
        let m = file.metadata().map_err(|_| Failure::UnsafePath)?;
        paths.validate()?;
        Ok((
            RootIdentity {
                device: m.dev(),
                inode: m.ino(),
                uid: m.uid(),
            },
            file,
        ))
    }
    #[cfg(not(unix))]
    {
        Err(Failure::UnsafePath)
    }
}
#[cfg(unix)]
pub(super) fn validate_admission_root(paths: &Paths) -> Result<std::fs::File, Failure> {
    bind_root(paths).map(|(_, pin)| pin)
}
/// Sealed preview intent. No Debug/serialization or mutable intent accessors.
/// Trusted UI/owner must create it at preview, not rebuild it at execution.
/// Machine/install truth remains a future adapter responsibility.
pub(crate) struct Admission {
    root: PathBuf,
    root_identity: RootIdentity,
    _root_pin: std::fs::File,
    context: NativeContext,
    stamp: Stamp,
    request: String,
    before: Image,
    target: Zeroizing<Vec<u8>>,
}
impl Admission {
    // Remove when the reviewed native-switch confirmation consumer is connected.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn preview(
        paths: &Paths,
        context: &NativeContext,
        stamp: &Stamp,
        request: &str,
        before: &Image,
        target: &[u8],
    ) -> Result<Self, Failure> {
        if request.is_empty() || request.len() > 256 || target.len() as u64 > LIMIT {
            return Err(Failure::RequestConflict);
        }
        let (root_identity, pin) = bind_root(paths)?;
        Ok(Self {
            root: paths.root.clone(),
            root_identity,
            _root_pin: pin,
            context: context.clone(),
            stamp: stamp.clone(),
            request: request.into(),
            before: before.clone(),
            target: Zeroizing::new(target.to_vec()),
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    generation: String,
    revision: u64,
}
pub(crate) trait JournalOwner {
    fn load(&mut self) -> Result<(Stamp, Option<Journal>), Failure>;
    /// Must be durable before returning; failures may have persisted and must be reread.
    fn persist(&mut self, expected: &Stamp, journal: &Journal) -> Result<Stamp, Failure>;
}
pub(crate) trait FileIo {
    fn read(&mut self, path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure>;
    fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure>;
    fn remove(&mut self, path: &Path) -> Result<(), Failure>;
}
// Remove when the native transaction driver selects this real IO backend.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Files;
impl FileIo for Files {
    fn read(&mut self, path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        crate::config_file_io::read_regular_file(path, LIMIT)
            .map(|v| v.map(Zeroizing::new))
            .map_err(|_| Failure::Io)
    }
    fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        crate::config_file_io::write_durable(path, bytes).map_err(|_| Failure::Io)
    }
    fn remove(&mut self, path: &Path) -> Result<(), Failure> {
        if crate::config_file_io::read_regular_file(path, LIMIT)
            .map_err(|_| Failure::Io)?
            .is_none()
        {
            return Ok(());
        }
        std::fs::remove_file(path).map_err(|_| Failure::Io)?;
        #[cfg(unix)]
        std::fs::File::open(path.parent().ok_or(Failure::UnsafePath)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| Failure::Io)?;
        Ok(())
    }
}
pub(crate) fn observe(io: &mut dyn FileIo, paths: &Paths) -> Result<Image, Failure> {
    Ok(Image {
        auth: io.read(&paths.auth)?,
        marker: io.read(&paths.marker)?,
    })
}
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Outcome {
    Applied,
    AlreadyApplied,
    Restored,
}
fn admitted(
    paths: &Paths,
    owner: &mut dyn JournalOwner,
    stamp: &Stamp,
    stopped: &mut dyn FnMut() -> bool,
    root_identity: &RootIdentity,
) -> Result<(), Failure> {
    paths.validate()?;
    if bind_root(paths)?.0 != *root_identity || !stopped() || owner.load()?.0 != *stamp {
        return Err(Failure::SourceChanged);
    }
    Ok(())
}
fn persist(owner: &mut dyn JournalOwner, stamp: &Stamp, j: &Journal) -> Result<Stamp, Failure> {
    let next = owner.persist(stamp, j)?;
    if next.generation != stamp.generation || stamp.revision.checked_add(1) != Some(next.revision) {
        return Err(Failure::Journal);
    }
    Ok(next)
}
fn valid_journal(j: &Journal, stamp: &Stamp, paths: &Paths, context: &NativeContext) -> bool {
    let offset = if j.phase == Phase::Prepared { 1 } else { 2 };
    j.version == 1
        && j.root == paths.root
        && j.context == *context
        && bind_root(paths).is_ok_and(|(id, _)| id == j.root_identity)
        && j.admission.generation == stamp.generation
        && j.admission.revision.checked_add(offset) == Some(stamp.revision)
}
// Remove when the native-switch operation consumer is connected.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn apply(
    paths: &Paths,
    io: &mut dyn FileIo,
    owner: &mut dyn JournalOwner,
    stopped: &mut dyn FnMut() -> bool,
    execution_context: &NativeContext,
    ticket: &Admission,
) -> Result<Outcome, Failure> {
    if paths.root != ticket.root
        || execution_context != &ticket.context
        || bind_root(paths)?.0 != ticket.root_identity
    {
        return Err(Failure::SourceChanged);
    }
    let request = ticket.request.as_str();
    let expected = &ticket.before;
    let target = ticket.target.as_slice();
    let admission = &ticket.stamp;
    let after = Image {
        auth: Some(Zeroizing::new(target.to_vec())),
        marker: None,
    };
    let (stamp, old) = owner.load()?;
    if let Some(old) = old {
        if !valid_journal(&old, &stamp, paths, execution_context) {
            return Err(Failure::RecoveryRequired);
        }
        if old.request == request {
            if old.admission != *admission || old.before != *expected || old.after != after {
                return Err(Failure::RequestConflict);
            }
            admitted(paths, owner, &stamp, stopped, &ticket.root_identity)?;
            return if old.phase == Phase::Committed && observe(io, paths)? == after {
                Ok(Outcome::AlreadyApplied)
            } else {
                Err(Failure::RecoveryRequired)
            };
        }
        if old.phase == Phase::Prepared {
            return Err(Failure::RecoveryRequired);
        }
    }
    if stamp != *admission {
        return Err(Failure::SourceChanged);
    }
    admitted(paths, owner, &stamp, stopped, &ticket.root_identity)?;
    if observe(io, paths)? != *expected {
        return Err(Failure::SourceChanged);
    }
    let mut j = Journal {
        admission: admission.clone(),
        version: 1,
        request: request.into(),
        root: paths.root.clone(),
        root_identity: ticket.root_identity.clone(),
        context: ticket.context.clone(),
        before: expected.clone(),
        after,
        phase: Phase::Prepared,
    };
    let prepared = persist(owner, &stamp, &j)?;
    let result = (|| {
        admitted(paths, owner, &prepared, stopped, &ticket.root_identity)?;
        if observe(io, paths)? != j.before {
            return Err(Failure::SourceChanged);
        }
        io.write(&paths.auth, target)?;
        admitted(paths, owner, &prepared, stopped, &ticket.root_identity)?;
        let mid = Image {
            auth: j.after.auth.clone(),
            marker: j.before.marker.clone(),
        };
        if observe(io, paths)? != mid {
            return Err(Failure::SourceChanged);
        }
        if mid.marker.is_some() {
            io.remove(&paths.marker)?;
        }
        admitted(paths, owner, &prepared, stopped, &ticket.root_identity)?;
        if observe(io, paths)? != j.after {
            return Err(Failure::SourceChanged);
        }
        Ok(())
    })();
    if result.is_err() {
        return match recover(paths, io, owner, stopped, &prepared, execution_context) {
            Ok(Outcome::Restored) => Err(Failure::RolledBack),
            _ => Err(Failure::RecoveryRequired),
        };
    }
    j.phase = Phase::Committed;
    persist(owner, &prepared, &j).map_err(|_| Failure::RecoveryRequired)?;
    Ok(Outcome::Applied)
}
fn known(current: &Image, j: &Journal) -> bool {
    (current.auth == j.before.auth || current.auth == j.after.auth)
        && (current.marker == j.before.marker || current.marker == j.after.marker)
}
fn restore_file(
    io: &mut dyn FileIo,
    path: &Path,
    before: &Option<Zeroizing<Vec<u8>>>,
) -> Result<(), Failure> {
    match before {
        Some(bytes) => io.write(path, bytes),
        None => io.remove(path),
    }
}
// Remove when the controlled native recovery consumer is connected.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn recover(
    paths: &Paths,
    io: &mut dyn FileIo,
    owner: &mut dyn JournalOwner,
    stopped: &mut dyn FnMut() -> bool,
    admission: &Stamp,
    execution_context: &NativeContext,
) -> Result<Outcome, Failure> {
    paths.validate()?;
    let (stamp, entry) = owner.load()?;
    let mut j = entry.ok_or(Failure::RecoveryRequired)?;
    if stamp != *admission || !valid_journal(&j, &stamp, paths, execution_context) {
        return Err(Failure::RecoveryRequired);
    }
    admitted(paths, owner, &stamp, stopped, &j.root_identity)
        .map_err(|_| Failure::RecoveryRequired)?;
    let mut current = observe(io, paths).map_err(|_| Failure::RecoveryRequired)?;
    if j.phase == Phase::Restored {
        return if current == j.before {
            Ok(Outcome::Restored)
        } else {
            Err(Failure::RecoveryRequired)
        };
    }
    if j.phase == Phase::Committed {
        return if current == j.after {
            Ok(Outcome::AlreadyApplied)
        } else {
            Err(Failure::RecoveryRequired)
        };
    }
    if !known(&current, &j) {
        return Err(Failure::RecoveryRequired);
    }
    if current.auth != j.before.auth {
        admitted(paths, owner, &stamp, stopped, &j.root_identity)
            .map_err(|_| Failure::RecoveryRequired)?;
        if observe(io, paths).map_err(|_| Failure::RecoveryRequired)? != current {
            return Err(Failure::RecoveryRequired);
        }
        restore_file(io, &paths.auth, &j.before.auth).map_err(|_| Failure::RecoveryRequired)?;
        current.auth = j.before.auth.clone();
    }
    if current.marker != j.before.marker {
        admitted(paths, owner, &stamp, stopped, &j.root_identity)
            .map_err(|_| Failure::RecoveryRequired)?;
        if observe(io, paths).map_err(|_| Failure::RecoveryRequired)? != current {
            return Err(Failure::RecoveryRequired);
        }
        restore_file(io, &paths.marker, &j.before.marker).map_err(|_| Failure::RecoveryRequired)?;
    }
    admitted(paths, owner, &stamp, stopped, &j.root_identity)
        .map_err(|_| Failure::RecoveryRequired)?;
    if observe(io, paths).map_err(|_| Failure::RecoveryRequired)? != j.before {
        return Err(Failure::RecoveryRequired);
    }
    j.phase = Phase::Restored;
    persist(owner, &stamp, &j).map_err(|_| Failure::RecoveryRequired)?;
    Ok(Outcome::Restored)
}

// Admission pins are currently Unix-only; do not imply Windows activation support.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    struct Owner {
        entry: Option<Journal>,
        calls: usize,
        fail_at: Option<usize>,
        stamp: Stamp,
    }
    impl Default for Owner {
        fn default() -> Self {
            Self {
                entry: None,
                calls: 0,
                fail_at: None,
                stamp: stamp(0),
            }
        }
    }
    fn stamp(revision: u64) -> Stamp {
        Stamp {
            generation: "synthetic-generation".into(),
            revision,
        }
    }
    impl JournalOwner for Owner {
        fn load(&mut self) -> Result<(Stamp, Option<Journal>), Failure> {
            Ok((self.stamp.clone(), self.entry.clone()))
        }
        fn persist(&mut self, expected: &Stamp, j: &Journal) -> Result<Stamp, Failure> {
            self.calls += 1;
            if &self.stamp != expected || self.fail_at == Some(self.calls) {
                return Err(Failure::Journal);
            }
            self.entry = Some(j.clone());
            self.stamp.revision += 1;
            Ok(self.stamp.clone())
        }
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Fault {
        None,
        Marker,
        Readback,
        CrashAfterAuth,
        CrashAfterMarker,
        ForeignAfterAuth,
    }
    struct Io {
        fs: Files,
        writes: usize,
        fault: Fault,
        armed: bool,
    }
    impl Io {
        fn new(fault: Fault) -> Self {
            Self {
                fs: Files,
                writes: 0,
                fault,
                armed: true,
            }
        }
    }
    impl FileIo for Io {
        fn read(&mut self, p: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
            if self.armed && self.writes > 0 && self.fault == Fault::Readback {
                self.armed = false;
                return Err(Failure::Io);
            }
            self.fs.read(p)
        }
        fn write(&mut self, p: &Path, b: &[u8]) -> Result<(), Failure> {
            self.fs.write(p, b)?;
            self.writes += 1;
            if self.armed && self.fault == Fault::CrashAfterAuth {
                self.armed = false;
                panic!("synthetic crash after auth publication")
            }
            if self.armed && self.fault == Fault::ForeignAfterAuth {
                self.armed = false;
                std::fs::write(p, b"foreign").unwrap();
                return Err(Failure::Io);
            }
            Ok(())
        }
        fn remove(&mut self, p: &Path) -> Result<(), Failure> {
            if self.armed && self.fault == Fault::Marker {
                self.armed = false;
                return Err(Failure::Io);
            }
            self.fs.remove(p)?;
            self.writes += 1;
            if self.armed && self.fault == Fault::CrashAfterMarker {
                self.armed = false;
                panic!("synthetic crash after marker removal")
            }
            Ok(())
        }
    }
    fn setup(marker: bool) -> (tempfile::TempDir, Paths, Image) {
        let d = crate::secrets::testing::tempdir().unwrap();
        let p = Paths::at(d.path()).unwrap();
        std::fs::write(&p.auth, b"A").unwrap();
        if marker {
            std::fs::write(&p.marker, b"logged-out").unwrap();
        }
        std::fs::write(d.path().join("chat.db"), b"chat-sentinel").unwrap();
        std::fs::write(d.path().join("settings.json"), b"settings-sentinel").unwrap();
        let before = observe(&mut Files, &p).unwrap();
        (d, p, before)
    }
    fn context() -> NativeContext {
        NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "synthetic-local-context".into(),
        }
    }
    // Test compatibility helper: retain existing fixtures; the production API uses a sealed ticket.
    #[allow(clippy::too_many_arguments)]
    fn apply(
        paths: &Paths,
        io: &mut dyn FileIo,
        owner: &mut dyn JournalOwner,
        stopped: &mut dyn FnMut() -> bool,
        request: &str,
        before: &Image,
        target: &[u8],
        stamp: &Stamp,
    ) -> Result<Outcome, Failure> {
        let ticket = Admission::preview(paths, &context(), stamp, request, before, target)?;
        super::apply(paths, io, owner, stopped, &context(), &ticket)
    }
    fn recover_latest(
        p: &Paths,
        io: &mut dyn FileIo,
        owner: &mut Owner,
    ) -> Result<Outcome, Failure> {
        let admission = owner.stamp.clone();
        recover(p, io, owner, &mut || true, &admission, &context())
    }
    fn unchanged(p: &Paths, before: &Image) {
        let now = observe(&mut Files, p).unwrap();
        assert!(now == *before);
    }
    #[test]
    fn journal_failure_causes_zero_native_writes() {
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::None);
        let mut owner = Owner {
            fail_at: Some(1),
            ..Default::default()
        };
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::Journal)
        );
        assert_eq!(io.writes, 0);
        unchanged(&p, &b);
    }
    #[test]
    fn successful_a_b_a_preserves_sentinels_and_both_marker_states() {
        for marker in [false, true] {
            let (d, p, a) = setup(marker);
            let mut io = Io::new(Fault::None);
            let mut owner = Owner::default();
            assert_eq!(
                apply(
                    &p,
                    &mut io,
                    &mut owner,
                    &mut || true,
                    "1",
                    &a,
                    b"B",
                    &stamp(0)
                ),
                Ok(Outcome::Applied)
            );
            let b = observe(&mut Files, &p).unwrap();
            assert_eq!(
                b.auth.as_deref().map(|v| v.as_slice()),
                Some(b"B".as_slice())
            );
            assert!(b.marker.is_none());
            assert_eq!(
                apply(
                    &p,
                    &mut io,
                    &mut owner,
                    &mut || true,
                    "2",
                    &b,
                    b"A",
                    &stamp(2)
                ),
                Ok(Outcome::Applied)
            );
            assert_eq!(std::fs::read(&p.auth).unwrap(), b"A");
            assert_eq!(
                std::fs::read(d.path().join("chat.db")).unwrap(),
                b"chat-sentinel"
            );
            assert_eq!(
                std::fs::read(d.path().join("settings.json")).unwrap(),
                b"settings-sentinel"
            );
        }
    }
    #[test]
    fn marker_failure_after_auth_restores_original_pair() {
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::Marker);
        let mut owner = Owner::default();
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::RolledBack)
        );
        assert!(io.writes > 0);
        unchanged(&p, &b);
    }
    #[test]
    fn readback_failure_restores_original_pair() {
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::Readback);
        let mut owner = Owner::default();
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::RolledBack)
        );
        unchanged(&p, &b);
    }
    #[test]
    fn write_before_cas_change_and_running_writer_block_all_native_writes() {
        for stopped in [false, true] {
            let (_d, p, b) = setup(true);
            if stopped {
                std::fs::write(&p.auth, b"foreign").unwrap();
            }
            let mut io = Io::new(Fault::None);
            let mut owner = Owner::default();
            assert!(apply(
                &p,
                &mut io,
                &mut owner,
                &mut || stopped,
                "1",
                &b,
                b"B",
                &stamp(0)
            )
            .is_err());
            assert_eq!(io.writes, 0);
            assert!(owner.entry.is_none());
        }
    }
    #[test]
    fn crash_after_auth_recovers_on_new_invocation() {
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::CrashAfterAuth);
        let mut owner = Owner::default();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0),
            )
        }));
        assert!(crashed.is_err());
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"B");
        assert_eq!(
            recover_latest(&p, &mut Files, &mut owner),
            Ok(Outcome::Restored)
        );
        unchanged(&p, &b);
        assert_eq!(
            recover_latest(&p, &mut Files, &mut owner),
            Ok(Outcome::Restored)
        );
    }
    #[test]
    fn foreign_edit_after_write_is_not_overwritten_by_recovery() {
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::ForeignAfterAuth);
        let mut owner = Owner::default();
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::RecoveryRequired)
        );
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"foreign");
        assert_eq!(
            recover_latest(&p, &mut Files, &mut owner),
            Err(Failure::RecoveryRequired)
        );
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"foreign");
        assert!(owner.entry.is_some());
    }
    #[test]
    fn repeated_request_is_idempotent_and_reuse_with_new_payload_is_rejected() {
        let (_d, p, b) = setup(false);
        let mut io = Io::new(Fault::None);
        let mut owner = Owner::default();
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Ok(Outcome::Applied)
        );
        let n = io.writes;
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Ok(Outcome::AlreadyApplied)
        );
        assert_eq!(io.writes, n);
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"C",
                &stamp(0)
            ),
            Err(Failure::RequestConflict)
        );
        assert_eq!(io.writes, n);
    }
    #[test]
    fn committed_journal_persist_failure_keeps_recovery_evidence() {
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::None);
        let mut owner = Owner {
            fail_at: Some(2),
            ..Default::default()
        };
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::RecoveryRequired)
        );
        assert!(owner.entry.is_some());
        assert_eq!(
            recover_latest(&p, &mut Files, &mut owner),
            Ok(Outcome::Restored)
        );
        unchanged(&p, &b);
    }
    #[test]
    fn traversal_directory_and_symlink_paths_are_rejected() {
        let (d, p, b) = setup(true);
        assert!(Paths::at(&d.path().join("../escape")).is_err());
        std::fs::remove_file(&p.auth).unwrap();
        std::fs::create_dir(&p.auth).unwrap();
        let mut owner = Owner::default();
        assert!(apply(
            &p,
            &mut Files,
            &mut owner,
            &mut || true,
            "1",
            &b,
            b"B",
            &stamp(0)
        )
        .is_err());
        assert!(owner.entry.is_none());
        #[cfg(unix)]
        {
            std::fs::remove_dir(&p.auth).unwrap();
            let target = d.path().join("other");
            std::fs::write(&target, b"outside").unwrap();
            std::os::unix::fs::symlink(&target, &p.auth).unwrap();
            assert!(apply(
                &p,
                &mut Files,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            )
            .is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"outside");
            let alias = d.path().join("alias");
            std::os::unix::fs::symlink(d.path(), &alias).unwrap();
            assert!(Paths::at(&alias).is_err());
        }
    }
    #[test]
    fn aba_bytes_do_not_revive_old_confirmation_revision() {
        let (_d, p, a) = setup(false);
        let mut io = Io::new(Fault::None);
        let mut owner = Owner::default();
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &a,
                b"B",
                &stamp(0)
            ),
            Ok(Outcome::Applied)
        );
        let b = observe(&mut Files, &p).unwrap();
        let next = owner.stamp.clone();
        assert_eq!(
            apply(&p, &mut io, &mut owner, &mut || true, "2", &b, b"A", &next),
            Ok(Outcome::Applied)
        );
        let writes = io.writes;
        assert!(apply(
            &p,
            &mut io,
            &mut owner,
            &mut || true,
            "1",
            &a,
            b"B",
            &stamp(0)
        )
        .is_err());
        assert_eq!(io.writes, writes);
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"A");
    }
    #[test]
    fn unknown_journal_version_and_owner_generation_never_recover_or_clear() {
        for change_generation in [false, true] {
            let (_d, p, b) = setup(true);
            let mut io = Io::new(Fault::CrashAfterAuth);
            let mut owner = Owner::default();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply(
                    &p,
                    &mut io,
                    &mut owner,
                    &mut || true,
                    "1",
                    &b,
                    b"B",
                    &stamp(0),
                )
            }));
            assert!(owner.entry.is_some());
            if change_generation {
                owner.stamp.generation = "other-generation".into();
            } else {
                owner.entry.as_mut().unwrap().version = 99;
            }
            let before = observe(&mut Files, &p).unwrap();
            let calls = owner.calls;
            assert_eq!(
                recover_latest(&p, &mut Files, &mut owner),
                Err(Failure::RecoveryRequired)
            );
            unchanged(&p, &before);
            assert_eq!(owner.calls, calls);
            assert!(owner.entry.is_some());
        }
    }

    #[test]
    fn crash_after_marker_restores_both_files_without_cleaning_unknown_siblings() {
        let (d, p, b) = setup(true);
        std::fs::write(
            d.path().join("workbuddy-desktop.info.tmp.unknown"),
            b"not-owned",
        )
        .unwrap();
        let mut io = Io::new(Fault::CrashAfterMarker);
        let mut owner = Owner::default();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0),
            )
        }));
        assert!(crashed.is_err());
        assert!(!p.marker.exists());
        assert_eq!(
            recover_latest(&p, &mut Files, &mut owner),
            Ok(Outcome::Restored)
        );
        unchanged(&p, &b);
        assert_eq!(
            std::fs::read(d.path().join("workbuddy-desktop.info.tmp.unknown")).unwrap(),
            b"not-owned"
        );
    }
    #[test]
    fn writer_appearing_after_auth_prevents_rollback_until_stopped() {
        let (_d, p, b) = setup(true);
        let mut owner = Owner::default();
        let mut io = Io::new(Fault::None);
        let mut observations = 0;
        let mut stopped = || {
            observations += 1;
            observations <= 2
        };
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut stopped,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::RecoveryRequired)
        );
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"B");
        assert_eq!(io.writes, 1);
        assert_eq!(
            recover_latest(&p, &mut Files, &mut owner),
            Ok(Outcome::Restored)
        );
        unchanged(&p, &b);
    }
    #[test]
    fn marker_symlink_is_not_followed_or_removed() {
        #[cfg(unix)]
        {
            let (d, p, b) = setup(true);
            std::fs::remove_file(&p.marker).unwrap();
            let target = d.path().join("outside-marker");
            std::fs::write(&target, b"keep").unwrap();
            std::os::unix::fs::symlink(&target, &p.marker).unwrap();
            let mut io = Io::new(Fault::None);
            let mut owner = Owner::default();
            assert!(apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            )
            .is_err());
            assert_eq!(io.writes, 0);
            assert!(std::fs::symlink_metadata(&p.marker)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read(target).unwrap(), b"keep");
        }
    }
    #[test]
    fn real_pure_prepare_output_flows_through_real_file_transaction() {
        let (_d, p, _) = setup(false);
        let a=br#"{"account":{"uid":"a","type":"personal"},"auth":{"accessToken":"synthetic-a","domain":"www.workbuddy.cn"},"accounts":[],"allAccounts":[]}"#;
        let b=br#"{"account":{"uid":"b","type":"personal"},"auth":{"accessToken":"synthetic-b","domain":"www.workbuddy.cn"},"accounts":[],"allAccounts":[]}"#;
        let c = super::super::native::NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "synthetic-source".into(),
        };
        let prepared = super::super::native::prepare(
            &super::super::native::capture(a, &c).unwrap(),
            &super::super::native::capture(b, &c).unwrap(),
        )
        .unwrap();
        std::fs::write(&p.auth, a).unwrap();
        let before = observe(&mut Files, &p).unwrap();
        assert_eq!(
            apply(
                &p,
                &mut Files,
                &mut Owner::default(),
                &mut || true,
                "1",
                &before,
                &prepared,
                &stamp(0)
            ),
            Ok(Outcome::Applied)
        );
        assert_eq!(std::fs::read(&p.auth).unwrap(), prepared.as_slice());
    }

    #[test]
    fn owner_cas_change_during_journal_publication_prevents_native_writes() {
        struct RacingOwner(Owner);
        impl JournalOwner for RacingOwner {
            fn load(&mut self) -> Result<(Stamp, Option<Journal>), Failure> {
                self.0.load()
            }
            fn persist(&mut self, expected: &Stamp, j: &Journal) -> Result<Stamp, Failure> {
                self.0.stamp.revision += 1;
                self.0.persist(expected, j)
            }
        }
        let (_d, p, b) = setup(true);
        let mut io = Io::new(Fault::None);
        let mut owner = RacingOwner(Owner::default());
        assert_eq!(
            apply(
                &p,
                &mut io,
                &mut owner,
                &mut || true,
                "1",
                &b,
                b"B",
                &stamp(0)
            ),
            Err(Failure::Journal)
        );
        assert_eq!(io.writes, 0);
        unchanged(&p, &b);
    }
    #[test]
    fn preview_from_one_root_cannot_write_another_with_identical_bytes() {
        let (_a, paths_a, before_a) = setup(true);
        let (_b, paths_b, before_b) = setup(true);
        let mut io = Io::new(Fault::None);
        let mut owner = Owner::default();
        let ticket = Admission::preview(
            &paths_a,
            &context(),
            &stamp(0),
            "preview-for-a",
            &before_a,
            b"B",
        )
        .unwrap();
        let result = super::apply(
            &paths_b,
            &mut io,
            &mut owner,
            &mut || true,
            &context(),
            &ticket,
        );
        assert!(result.is_err());
        assert_eq!(io.writes, 0);
        unchanged(&paths_b, &before_b);
    }

    #[test]
    fn sealed_intent_rejects_context_change_and_copies_original_target() {
        let (_d, p, b) = setup(false);
        let mut target = b"B".to_vec();
        let ticket = Admission::preview(&p, &context(), &stamp(0), "1", &b, &target).unwrap();
        target[0] = b'C';
        let mut other = context();
        other.source = "another-install".into();
        let mut io = Io::new(Fault::None);
        let mut owner = Owner::default();
        assert_eq!(
            super::apply(&p, &mut io, &mut owner, &mut || true, &other, &ticket),
            Err(Failure::SourceChanged)
        );
        assert_eq!(io.writes, 0);
        assert_eq!(
            super::apply(&p, &mut io, &mut owner, &mut || true, &context(), &ticket),
            Ok(Outcome::Applied)
        );
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"B");
    }
    #[test]
    fn same_path_replaced_root_cannot_reuse_preview_identity() {
        let outer = crate::secrets::testing::tempdir().unwrap();
        let root = outer.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let p = Paths::at(&root).unwrap();
        std::fs::write(&p.auth, b"A").unwrap();
        let before = observe(&mut Files, &p).unwrap();
        let ticket = Admission::preview(&p, &context(), &stamp(0), "1", &before, b"B").unwrap();
        std::fs::rename(&root, outer.path().join("old-root")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(&p.auth, b"A").unwrap();
        let mut io = Io::new(Fault::None);
        assert_eq!(
            super::apply(
                &p,
                &mut io,
                &mut Owner::default(),
                &mut || true,
                &context(),
                &ticket
            ),
            Err(Failure::SourceChanged)
        );
        assert_eq!(io.writes, 0);
        assert_eq!(std::fs::read(&p.auth).unwrap(), b"A");
    }
    #[test]
    fn failed_auth_or_marker_rollback_keeps_prepared_journal_and_revision() {
        struct FailRestore {
            fs: Files,
            marker: bool,
        }
        impl FileIo for FailRestore {
            fn read(&mut self, p: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
                self.fs.read(p)
            }
            fn write(&mut self, p: &Path, b: &[u8]) -> Result<(), Failure> {
                if p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(".logged-out")
                    == self.marker
                {
                    Err(Failure::Io)
                } else {
                    self.fs.write(p, b)
                }
            }
            fn remove(&mut self, p: &Path) -> Result<(), Failure> {
                self.fs.remove(p)
            }
        }
        for marker in [false, true] {
            let (_d, p, b) = setup(true);
            let mut owner = Owner::default();
            let mut io = Io::new(Fault::CrashAfterMarker);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                apply(
                    &p,
                    &mut io,
                    &mut owner,
                    &mut || true,
                    "1",
                    &b,
                    b"B",
                    &stamp(0),
                )
            }));
            let before_stamp = owner.stamp.clone();
            let calls = owner.calls;
            assert_eq!(
                recover_latest(&p, &mut FailRestore { fs: Files, marker }, &mut owner),
                Err(Failure::RecoveryRequired)
            );
            assert!(owner.stamp == before_stamp);
            assert_eq!(owner.calls, calls);
            assert!(owner.entry.as_ref().unwrap().phase == Phase::Prepared);
            assert_eq!(
                recover_latest(&p, &mut Files, &mut owner),
                Ok(Outcome::Restored)
            );
            unchanged(&p, &b);
        }
    }
}

/// Private LoongPort owner wire format; never an official-client auth schema.
pub(super) mod owner_wire {
    use super::*;
    use serde::{Deserialize, Serialize};
    pub(in super::super) const MAX_BYTES: u64 = 80 * 1024 * 1024;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WireImage {
        #[serde(with = "secret_bytes")]
        auth: Option<Zeroizing<Vec<u8>>>,
        #[serde(with = "secret_bytes")]
        marker: Option<Zeroizing<Vec<u8>>>,
    }
    mod secret_bytes {
        use super::*;
        pub fn serialize<S: serde::Serializer>(
            v: &Option<Zeroizing<Vec<u8>>>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            v.as_ref().map(|v| v.as_slice()).serialize(s)
        }
        pub fn deserialize<'de, D: serde::Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, D::Error> {
            Option::<Vec<u8>>::deserialize(d).map(|v| v.map(Zeroizing::new))
        }
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WireJournal {
        version: u32,
        admission: (String, u64),
        request: String,
        root: PathBuf,
        root_identity: (u64, u64, u32),
        context: [String; 4],
        before: WireImage,
        after: WireImage,
        phase: u8,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WireState {
        version: u32,
        generation: String,
        revision: u64,
        journal: Option<WireJournal>,
    }
    pub(in super::super) struct State {
        pub(in super::super) stamp: Stamp,
        pub(in super::super) journal: Option<Journal>,
    }
    impl State {
        /// Synthetic codec/phase fixture, NOT a valid filesystem admission or native activation.
        /// Kept inside the real wire owner so tests do not duplicate its private JSON schema.
        #[cfg(test)]
        pub(in super::super) fn fixture(root: &Path, phase: Phase) -> Self {
            let mut state = Self::initial("synthetic-phase-fixture".into());
            let admission = state.stamp.clone();
            state.stamp.revision = if phase == Phase::Prepared { 1 } else { 2 };
            state.journal = Some(Journal {
                admission,
                version: 1,
                request: "synthetic-receipt".into(),
                root: root.into(),
                root_identity: RootIdentity {
                    device: 0,
                    inode: 0,
                    uid: 0,
                },
                context: NativeContext {
                    product: "workbuddy".into(),
                    region: "cn".into(),
                    realm: "www.workbuddy.cn".into(),
                    source: "synthetic-machine".into(),
                },
                before: Image {
                    auth: Some(Zeroizing::new(b"A".to_vec())),
                    marker: None,
                },
                after: Image {
                    auth: Some(Zeroizing::new(b"B".to_vec())),
                    marker: None,
                },
                phase,
            });
            state
        }
        pub(in super::super) fn initial(generation: String) -> Self {
            Self {
                stamp: Stamp {
                    generation,
                    revision: 0,
                },
                journal: None,
            }
        }
        pub(in super::super) fn pending(&self) -> bool {
            self.journal
                .as_ref()
                .is_some_and(|j| j.phase == Phase::Prepared)
        }
        pub(in super::super) fn encode(&self) -> Result<Zeroizing<Vec<u8>>, Failure> {
            let journal = self.journal.as_ref().map(|j| WireJournal {
                version: j.version,
                admission: (j.admission.generation.clone(), j.admission.revision),
                request: j.request.clone(),
                root: j.root.clone(),
                root_identity: (
                    j.root_identity.device,
                    j.root_identity.inode,
                    j.root_identity.uid,
                ),
                context: [
                    j.context.product.clone(),
                    j.context.region.clone(),
                    j.context.realm.clone(),
                    j.context.source.clone(),
                ],
                before: WireImage {
                    auth: j.before.auth.clone(),
                    marker: j.before.marker.clone(),
                },
                after: WireImage {
                    auth: j.after.auth.clone(),
                    marker: j.after.marker.clone(),
                },
                phase: match j.phase {
                    Phase::Prepared => 0,
                    Phase::Committed => 1,
                    Phase::Restored => 2,
                },
            });
            let bytes = Zeroizing::new(
                serde_json::to_vec(&WireState {
                    version: 1,
                    generation: self.stamp.generation.clone(),
                    revision: self.stamp.revision,
                    journal,
                })
                .map_err(|_| Failure::Journal)?,
            );
            if bytes.len() as u64 > MAX_BYTES {
                return Err(Failure::Journal);
            }
            Ok(bytes)
        }
        pub(in super::super) fn decode(bytes: &[u8]) -> Result<Self, Failure> {
            if bytes.len() as u64 > MAX_BYTES {
                return Err(Failure::Journal);
            }
            let w: WireState = serde_json::from_slice(bytes).map_err(|_| Failure::Journal)?;
            if w.version != 1 || w.generation.is_empty() || w.generation.len() > 128 {
                return Err(Failure::Journal);
            }
            let stamp = Stamp {
                generation: w.generation,
                revision: w.revision,
            };
            let journal = match w.journal {
                None => {
                    if stamp.revision != 0 {
                        return Err(Failure::Journal);
                    }
                    None
                }
                Some(j) => {
                    let phase = match j.phase {
                        0 => Phase::Prepared,
                        1 => Phase::Committed,
                        2 => Phase::Restored,
                        _ => return Err(Failure::Journal),
                    };
                    if j.version != 1
                        || j.admission.0 != stamp.generation
                        || j.admission
                            .1
                            .checked_add(if phase == Phase::Prepared { 1 } else { 2 })
                            != Some(stamp.revision)
                        || j.request.is_empty()
                        || j.request.len() > 256
                        || !j.root.is_absolute()
                        || j.root.components().any(|c| {
                            !matches!(
                                c,
                                Component::RootDir | Component::Normal(_) | Component::Prefix(_)
                            )
                        })
                        || j.context.iter().any(|s| s.is_empty())
                        || [
                            &j.before.auth,
                            &j.before.marker,
                            &j.after.auth,
                            &j.after.marker,
                        ]
                        .iter()
                        .any(|v| v.as_ref().is_some_and(|b| b.len() as u64 > LIMIT))
                        || j.after.auth.is_none()
                        || j.after.marker.is_some()
                    {
                        return Err(Failure::Journal);
                    }
                    let [product, region, realm, source] = j.context;
                    Some(Journal {
                        version: j.version,
                        admission: Stamp {
                            generation: j.admission.0,
                            revision: j.admission.1,
                        },
                        request: j.request,
                        root: j.root,
                        root_identity: RootIdentity {
                            device: j.root_identity.0,
                            inode: j.root_identity.1,
                            uid: j.root_identity.2,
                        },
                        context: NativeContext {
                            product,
                            region,
                            realm,
                            source,
                        },
                        before: Image {
                            auth: j.before.auth,
                            marker: j.before.marker,
                        },
                        after: Image {
                            auth: j.after.auth,
                            marker: j.after.marker,
                        },
                        phase,
                    })
                }
            };
            Ok(Self { stamp, journal })
        }
        pub(in super::super) fn next(
            &self,
            expected: &Stamp,
            journal: &Journal,
        ) -> Result<Self, Failure> {
            if expected != &self.stamp {
                return Err(Failure::SourceChanged);
            }
            if journal.phase == Phase::Prepared {
                if self.pending() || journal.admission != self.stamp {
                    return Err(Failure::Journal);
                }
            } else {
                let old = self.journal.as_ref().ok_or(Failure::Journal)?;
                if old.phase != Phase::Prepared
                    || old.admission != journal.admission
                    || old.request != journal.request
                    || old.root != journal.root
                    || old.root_identity != journal.root_identity
                    || old.context != journal.context
                    || old.before != journal.before
                    || old.after != journal.after
                {
                    return Err(Failure::Journal);
                }
            }
            let next = Self {
                stamp: Stamp {
                    generation: self.stamp.generation.clone(),
                    revision: self.stamp.revision.checked_add(1).ok_or(Failure::Journal)?,
                },
                journal: Some(journal.clone()),
            };
            // Decode the private canonical representation to enforce the persisted contract.
            Self::decode(&next.encode()?)
        }
    }
}

#[cfg(all(test, not(unix)))]
mod unsupported_platform_tests {
    use super::*;
    #[test]
    fn unsupported_admission_refuses_without_touching_native_files() {
        let dir = crate::secrets::testing::tempdir().unwrap();
        let paths = Paths::at(dir.path()).unwrap();
        std::fs::write(&paths.auth, b"A").unwrap();
        std::fs::write(&paths.marker, b"logged-out").unwrap();
        let before = observe(&mut Files, &paths).unwrap();
        let stamp = owner_wire::State::initial("synthetic-owner".into()).stamp;
        let context = NativeContext {
            product: "workbuddy".into(),
            region: "cn".into(),
            realm: "www.workbuddy.cn".into(),
            source: "synthetic-machine".into(),
        };
        assert!(matches!(
            Admission::preview(&paths, &context, &stamp, "preview", &before, b"B"),
            Err(Failure::UnsafePath)
        ));
        assert!(observe(&mut Files, &paths).unwrap() == before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert!(Paths::at(&dir.path().join("../escape")).is_err());
    }
    #[test]
    fn unsupported_apply_and_recovery_reject_even_a_synthetic_private_ticket_without_writes() {
        struct RecordingIo {
            writes: usize,
        }
        impl FileIo for RecordingIo {
            fn read(&mut self, path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
                Files.read(path)
            }
            fn write(&mut self, _: &Path, _: &[u8]) -> Result<(), Failure> {
                self.writes += 1;
                Err(Failure::Io)
            }
            fn remove(&mut self, _: &Path) -> Result<(), Failure> {
                self.writes += 1;
                Err(Failure::Io)
            }
        }
        // Synthetic owner adapter, not a protected production journal owner.
        struct Owner {
            state: owner_wire::State,
            writes: usize,
        }
        impl JournalOwner for Owner {
            fn load(&mut self) -> Result<(Stamp, Option<Journal>), Failure> {
                Ok((self.state.stamp.clone(), self.state.journal.clone()))
            }
            fn persist(&mut self, expected: &Stamp, journal: &Journal) -> Result<Stamp, Failure> {
                self.writes += 1;
                self.state = self.state.next(expected, journal)?;
                Ok(self.state.stamp.clone())
            }
        }
        let dir = crate::secrets::testing::tempdir().unwrap();
        let paths = Paths::at(dir.path()).unwrap();
        std::fs::write(&paths.auth, b"synthetic-A").unwrap();
        std::fs::write(&paths.marker, b"logged-out").unwrap();
        let before = observe(&mut Files, &paths).unwrap();
        let mut owner = Owner {
            state: owner_wire::State::fixture(dir.path(), Phase::Prepared),
            writes: 0,
        };
        let journal_before = owner.state.encode().unwrap();
        let journal_path = dir.path().join("synthetic-owner-journal.json");
        std::fs::write(&journal_path, &journal_before).unwrap();
        let context = owner.state.journal.as_ref().unwrap().context.clone();
        let stamp = owner.state.stamp.clone();
        // Adversarial test-only construction: this file handle is NOT a valid
        // directory admission pin. No production caller can construct this ticket.
        // Both execution entrances must independently refuse unsupported pinning.
        let ticket = Admission {
            root: paths.root.clone(),
            root_identity: owner.state.journal.as_ref().unwrap().root_identity.clone(),
            _root_pin: std::fs::File::open(&paths.auth).unwrap(),
            context: context.clone(),
            stamp: stamp.clone(),
            request: "synthetic-bypass".into(),
            before: before.clone(),
            target: Zeroizing::new(b"synthetic-B".to_vec()),
        };
        let mut io = RecordingIo { writes: 0 };
        assert_eq!(
            apply(&paths, &mut io, &mut owner, &mut || true, &context, &ticket),
            Err(Failure::UnsafePath)
        );
        assert_eq!(
            recover(&paths, &mut io, &mut owner, &mut || true, &stamp, &context),
            Err(Failure::RecoveryRequired)
        );
        assert_eq!(io.writes, 0);
        assert_eq!(owner.writes, 0);
        assert!(owner.state.stamp == stamp);
        assert!(owner.state.encode().unwrap() == journal_before);
        assert_eq!(
            std::fs::read(&journal_path).unwrap(),
            journal_before.as_slice()
        );
        assert!(observe(&mut Files, &paths).unwrap() == before);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }
}
