//! Exact WorkBuddy app identity for normal-termination admission.
use super::process::Process;
use crate::workbuddy::native_switch_lifecycle::Error;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AppIdentity {
    pub pid: u32,
    pub uid: u32,
    pub start: [u64; 2],
    pub bundle_id: String,
    pub bundle: PathBuf,
    pub executable: PathBuf,
}

pub(super) fn exact_target(
    expected_uid: u32,
    bundle: &std::path::Path,
    before: &AppIdentity,
    now: &AppIdentity,
) -> bool {
    bundle.is_absolute()
        && before == now
        && before.pid != 0
        && before.uid == expected_uid
        && before.start[0] != 0
        && before.bundle_id == "com.tencent.workbuddy.mac"
        && before.bundle == bundle
        && before.executable == bundle.join("Contents/MacOS/Electron")
}

fn kernel_and_app_agree(process: &Process, app: &AppIdentity, bundle: &std::path::Path) -> bool {
    process.id == app.pid
        && process.start_identity == app.start
        && process.executable == app.executable
        && process.executable == bundle.join("Contents/MacOS/Electron")
}

fn target_process<'a>(
    rows: &'a [Process],
    bundle: &std::path::Path,
) -> Result<Option<&'a Process>, Error> {
    let executable = bundle.join("Contents/MacOS/Electron");
    let mut matches = rows.iter().filter(|row| row.executable == executable);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(Error::SourceChanged);
    }
    Ok(first)
}

#[cfg(all(feature = "gui", target_os = "macos", target_arch = "aarch64"))]
pub(in crate::workbuddy::native_controller) mod mac {
    use super::super::{
        install,
        process::{MacProcesses, Process, Processes, Scope},
        ControlledSource,
    };
    use super::{exact_target, kernel_and_app_agree, AppIdentity};
    use crate::workbuddy::{
        model::Domain,
        native_source::{self, Pair},
        native_switch_lifecycle::{Driver, Error, Restart, Stop},
        native_transaction::Failure,
    };
    use objc2_app_kit::NSRunningApplication;
    use std::{
        cell::RefCell,
        path::{Path, PathBuf},
        rc::Rc,
        time::{Duration, Instant},
    };

    pub(in crate::workbuddy::native_controller) type SharedSource = Rc<RefCell<install::Source>>;
    pub(in crate::workbuddy::native_controller) type SharedWitness = Rc<RefCell<Option<Pair>>>;
    pub(in crate::workbuddy::native_controller) struct MacDriver {
        bundle: PathBuf,
        scope: Scope,
        source: SharedSource,
        witness: SharedWitness,
        uid: u32,
    }
    pub(in crate::workbuddy::native_controller) fn create(
        bundle: &Path,
        realm: Domain,
    ) -> Result<(MacDriver, SharedSource, SharedWitness), Failure> {
        let source = Rc::new(RefCell::new(install::open(bundle, realm)?));
        let scope = Scope::at(bundle, &bundle.join("Contents/MacOS/Electron"))?;
        let witness = Rc::new(RefCell::new(None));
        let uid = unsafe { libc::geteuid() };
        Ok((
            MacDriver {
                bundle: bundle.into(),
                scope,
                source: source.clone(),
                witness: witness.clone(),
                uid,
            },
            source,
            witness,
        ))
    }
    fn running_app(
        process: &Process,
        uid: u32,
        expected_bundle: &Path,
    ) -> Result<(objc2::rc::Retained<NSRunningApplication>, AppIdentity), Error> {
        let pid = i32::try_from(process.id).map_err(|_| Error::SourceChanged)?;
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .ok_or(Error::SourceChanged)?;
        let bundle = app
            .bundleURL()
            .and_then(|u| u.path())
            .map(|p| PathBuf::from(p.to_string()))
            .ok_or(Error::SourceChanged)?;
        let executable = app
            .executableURL()
            .and_then(|u| u.path())
            .map(|p| PathBuf::from(p.to_string()))
            .ok_or(Error::SourceChanged)?;
        let bundle_id = app
            .bundleIdentifier()
            .map(|s| s.to_string())
            .ok_or(Error::SourceChanged)?;
        let identity = AppIdentity {
            pid: process.id,
            uid,
            start: process.start_identity,
            bundle_id,
            bundle,
            executable,
        };
        if !kernel_and_app_agree(process, &identity, expected_bundle) {
            return Err(Error::SourceChanged);
        }
        Ok((app, identity))
    }
    impl MacDriver {
        fn processes(&self) -> Result<Vec<Process>, Error> {
            MacProcesses.snapshot().map_err(|_| Error::SourceChanged)
        }
        fn unchanged(&self, before: &Pair) -> Result<(), Error> {
            self.source
                .borrow_mut()
                .source_mut()
                .verify_unchanged()
                .map_err(|_| Error::SourceChanged)?;
            let path = self.source.borrow().root().join("workbuddy-desktop.info");
            if native_source::read_pair(&path).map_err(|_| Error::SourceChanged)? != *before {
                return Err(Error::SourceChanged);
            }
            self.scope.verify().map_err(|_| Error::SourceChanged)
        }
        fn stopped(&self, rows: &[Process]) -> Result<bool, Error> {
            for row in rows {
                if self
                    .scope
                    .running(&row.executable)
                    .map_err(|_| Error::SourceChanged)?
                {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
    impl Driver for MacDriver {
        fn stop_and_recheck(&mut self) -> Result<Stop, Error> {
            self.source
                .borrow_mut()
                .source_mut()
                .begin_operation()
                .map_err(|_| Error::SourceChanged)?;
            let auth = self.source.borrow().root().join("workbuddy-desktop.info");
            let before = native_source::read_pair(&auth).map_err(|_| Error::SourceChanged)?;
            let rows = self.processes()?;
            let executable = self.bundle.join("Contents/MacOS/Electron");
            let Some(process) = super::target_process(&rows, &self.bundle)? else {
                if !self.stopped(&rows)? {
                    return Err(Error::WritersRemaining);
                }
                self.source
                    .borrow_mut()
                    .verify_stopped()
                    .map_err(|_| Error::SourceChanged)?;
                self.unchanged(&before)?;
                *self.witness.borrow_mut() = Some(before);
                return Ok(Stop::AlreadyStopped);
            };
            let (_, first) = running_app(process, self.uid, &self.bundle)?;
            let again = self.processes()?;
            let current = again
                .iter()
                .find(|p| p.id == first.pid)
                .ok_or(Error::SourceChanged)?;
            let (_, now) = running_app(current, self.uid, &self.bundle)?;
            if !exact_target(self.uid, &self.bundle, &first, &now) {
                return Err(Error::SourceChanged);
            }
            self.unchanged(&before)?;
            let last = self.processes()?;
            let current = last
                .iter()
                .find(|p| p.id == first.pid)
                .ok_or(Error::SourceChanged)?;
            let (app, now) = running_app(current, self.uid, &self.bundle)?;
            if !exact_target(self.uid, &self.bundle, &first, &now) {
                return Err(Error::SourceChanged);
            }
            if !app.terminate() {
                return Err(Error::QuitRefused);
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let rows = self.processes()?;
                if let Some(p) = rows.iter().find(|p| p.id == first.pid) {
                    if p.start_identity != first.start || p.executable != executable {
                        return Err(Error::SourceChanged);
                    }
                }
                if self.stopped(&rows)? {
                    self.source
                        .borrow_mut()
                        .verify_stopped()
                        .map_err(|_| Error::SourceChanged)?;
                    self.unchanged(&before)?;
                    *self.witness.borrow_mut() = Some(before);
                    return Ok(Stop::StoppedByUs);
                }
                if Instant::now() >= deadline {
                    return Err(if rows.iter().any(|p| p.id == first.pid) {
                        Error::QuitTimeout
                    } else {
                        Error::WritersRemaining
                    });
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        fn restart(&mut self) -> Restart {
            if self.source.borrow_mut().verify_stopped().is_err() {
                return Restart::Unknown;
            }
            let Ok(mut child) = std::process::Command::new("/usr/bin/open")
                .arg("-a")
                .arg(&self.bundle)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            else {
                return Restart::Failed;
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) if status.success() => return Restart::Requested,
                    Ok(Some(_)) => return Restart::Unknown,
                    Err(_) => {
                        crate::workbuddy::native_switch_lifecycle::hold_launcher(child);
                        return Restart::Unknown;
                    }
                    Ok(None) if Instant::now() >= deadline => {
                        crate::workbuddy::native_switch_lifecycle::hold_launcher(child);
                        return Restart::Unknown;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_and_app_paths_must_agree_even_for_same_pid_and_start() {
        let bundle = PathBuf::from("/synthetic/WorkBuddy.app");
        let app = AppIdentity {
            pid: 42,
            uid: 501,
            start: [123, 456],
            bundle_id: "com.tencent.workbuddy.mac".into(),
            executable: bundle.join("Contents/MacOS/Electron"),
            bundle: bundle.clone(),
        };
        let kernel = Process {
            id: 42,
            start_identity: [123, 456],
            executable: app.executable.clone(),
        };
        assert!(kernel_and_app_agree(&kernel, &app, &bundle));
        let mut changed = kernel.clone();
        changed.executable = "/synthetic/Other.app/Contents/MacOS/Electron".into();
        assert!(!kernel_and_app_agree(&changed, &app, &bundle));
        let mut changed_app = app.clone();
        changed_app.executable = "/synthetic/Other.app/Contents/MacOS/Electron".into();
        assert!(!kernel_and_app_agree(&kernel, &changed_app, &bundle));
    }
    #[test]
    fn quit_target_requires_exact_workbuddy_bundle_uid_pid_and_start() {
        let bundle = PathBuf::from("/synthetic/WorkBuddy.app");
        let app = AppIdentity {
            pid: 42,
            uid: 501,
            start: [123, 456],
            bundle_id: "com.tencent.workbuddy.mac".into(),
            executable: bundle.join("Contents/MacOS/Electron"),
            bundle: bundle.clone(),
        };
        assert!(exact_target(501, &bundle, &app, &app));
        assert!(!exact_target(502, &bundle, &app, &app));
        let mut invalid = app.clone();
        invalid.bundle_id = "other.app".into();
        assert!(!exact_target(501, &bundle, &invalid, &invalid));
        invalid = app.clone();
        invalid.pid = 0;
        assert!(!exact_target(501, &bundle, &invalid, &invalid));
        invalid = app.clone();
        invalid.start = [0, 0];
        assert!(!exact_target(501, &bundle, &invalid, &invalid));
        for change in 0..6 {
            let mut other = app.clone();
            match change {
                0 => other.pid += 1,
                1 => other.uid += 1,
                2 => other.start[0] += 1,
                3 => other.bundle_id = "other.app".into(),
                4 => other.bundle = "/synthetic/Other.app".into(),
                _ => other.executable = "/synthetic/Other.app/Contents/MacOS/Electron".into(),
            }
            assert!(!exact_target(501, &bundle, &app, &other));
        }
    }

    #[test]
    fn target_lookup_never_selects_other_electron_and_rejects_duplicate_main() {
        let bundle = PathBuf::from("/synthetic/WorkBuddy.app");
        let target = Process {
            id: 42,
            start_identity: [123, 456],
            executable: bundle.join("Contents/MacOS/Electron"),
        };
        let other = Process {
            id: 77,
            start_identity: [321, 654],
            executable: "/synthetic/Other.app/Contents/MacOS/Electron".into(),
        };
        let rows = vec![other.clone(), target.clone()];
        assert_eq!(target_process(&rows, &bundle).unwrap().unwrap().id, 42);
        assert!(target_process(&[other], &bundle).unwrap().is_none());
        assert!(target_process(&[target.clone(), target], &bundle).is_err());
    }
}
