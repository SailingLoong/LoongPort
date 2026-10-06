//! Exact controlled-instance coordination; no credential or vault IO.
use super::admission::WriterState;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Instance {
    pub pid: i32,
    pub uid: u32,
    pub start: [u64; 2],
    pub executable: PathBuf,
    pub bundle_id: String,
}
#[derive(Clone)]
pub(super) struct BoundInstance {
    pub source: String,
    pub instance: Instance,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    SourceChanged,
    Unbound,
    InstanceChanged,
    QuitRefused,
    QuitTimeout,
    RestartFailed,
}
pub(super) trait Driver {
    fn source(&mut self) -> Result<String, Failure>;
    fn writers(&mut self) -> Result<WriterState, Failure>;
    fn instance(&mut self, pid: i32) -> Result<Option<Instance>, Failure>;
    fn quit(&mut self, instance: &Instance) -> Result<bool, Failure>;
    fn launch(&mut self) -> Result<Instance, Failure>;
    fn wait_step(&mut self);
}
pub(super) fn stop(
    driver: &mut dyn Driver,
    source: &str,
    bound: Option<&BoundInstance>,
) -> Result<(), Failure> {
    if driver.source()? != source {
        return Err(Failure::SourceChanged);
    }
    match driver.writers()? {
        WriterState::Stopped => return Ok(()),
        WriterState::Unknown => return Err(Failure::InstanceChanged),
        WriterState::Running => (),
    }
    let bound = bound
        .filter(|bound| bound.source == source)
        .ok_or(Failure::Unbound)?;
    if driver.instance(bound.instance.pid)?.as_ref() != Some(&bound.instance) {
        return Err(Failure::InstanceChanged);
    }
    // Repeat source and instance checks immediately before requesting normal quit.
    if driver.source()? != source {
        return Err(Failure::SourceChanged);
    }
    if driver.instance(bound.instance.pid)?.as_ref() != Some(&bound.instance) {
        return Err(Failure::InstanceChanged);
    }
    if !driver.quit(&bound.instance)? {
        return Err(Failure::QuitRefused);
    }
    for _ in 0..100 {
        match driver.instance(bound.instance.pid)? {
            Some(instance) if instance != bound.instance => return Err(Failure::InstanceChanged),
            None if driver.writers()? == WriterState::Stopped => {
                if driver.source()? != source {
                    return Err(Failure::SourceChanged);
                }
                return Ok(());
            }
            _ => (),
        }
        driver.wait_step();
    }
    Err(Failure::QuitTimeout)
}
pub(super) fn restart(
    driver: &mut dyn Driver,
    source: &str,
    old: Option<&BoundInstance>,
) -> Result<BoundInstance, Failure> {
    if driver.source()? != source {
        return Err(Failure::SourceChanged);
    }
    if driver.writers()? != WriterState::Stopped {
        return Err(Failure::RestartFailed);
    }
    let new = driver.launch()?;
    if old.is_some_and(|bound| bound.instance == new)
        || driver.instance(new.pid)?.as_ref() != Some(&new)
        || driver.source()? != source
    {
        return Err(Failure::RestartFailed);
    }
    Ok(BoundInstance {
        source: source.to_owned(),
        instance: new,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        source: String,
        instance: Option<Instance>,
        writers: WriterState,
        quit_allowed: bool,
        stop_after_quit: bool,
        helper_remains: bool,
        source_after_quit: bool,
        launch_old: bool,
        quits: usize,
        launches: usize,
        waits: usize,
    }
    fn instance(start: u64) -> Instance {
        Instance {
            pid: 42,
            uid: 501,
            start: [start, 0],
            executable: "/synthetic/ZCode.app/Contents/MacOS/ZCode".into(),
            bundle_id: "synthetic.zcode".into(),
        }
    }
    fn fake() -> Fake {
        Fake {
            source: "source".into(),
            instance: Some(instance(1)),
            writers: WriterState::Running,
            quit_allowed: true,
            stop_after_quit: true,
            helper_remains: false,
            source_after_quit: false,
            launch_old: false,
            quits: 0,
            launches: 0,
            waits: 0,
        }
    }
    fn bound() -> BoundInstance {
        BoundInstance {
            source: "source".into(),
            instance: instance(1),
        }
    }
    impl Driver for Fake {
        fn source(&mut self) -> Result<String, Failure> {
            Ok(self.source.clone())
        }
        fn writers(&mut self) -> Result<WriterState, Failure> {
            Ok(self.writers)
        }
        fn instance(&mut self, _: i32) -> Result<Option<Instance>, Failure> {
            Ok(self.instance.clone())
        }
        fn quit(&mut self, _: &Instance) -> Result<bool, Failure> {
            self.quits += 1;
            if self.quit_allowed && self.stop_after_quit {
                self.instance = None;
                self.writers = if self.helper_remains {
                    WriterState::Running
                } else {
                    WriterState::Stopped
                };
                if self.source_after_quit {
                    self.source = "changed".into();
                }
            }
            Ok(self.quit_allowed)
        }
        fn launch(&mut self) -> Result<Instance, Failure> {
            self.launches += 1;
            let new = instance(if self.launch_old { 1 } else { 2 });
            self.instance = Some(new.clone());
            self.writers = WriterState::Running;
            Ok(new)
        }
        fn wait_step(&mut self) {
            self.waits += 1;
        }
    }
    #[test]
    fn exact_instance_quits_and_verified_new_instance_restarts() {
        let mut f = fake();
        stop(&mut f, "source", Some(&bound())).unwrap();
        assert_eq!(f.quits, 1);
        assert_eq!(f.waits, 0);
        let new = restart(&mut f, "source", Some(&bound())).unwrap();
        assert_eq!(new.instance.start, [2, 0]);
    }
    #[test]
    fn unbound_or_changed_instance_never_receives_quit() {
        for mode in 0..3 {
            let mut f = fake();
            if mode == 1 {
                f.instance = Some(instance(9));
            }
            if mode == 2 {
                f.source = "other".into();
            }
            let binding = bound();
            assert!(stop(
                &mut f,
                "source",
                if mode == 0 { None } else { Some(&binding) }
            )
            .is_err());
            assert_eq!(f.quits, 0);
            assert_eq!(f.launches, 0);
        }
    }
    #[test]
    fn refusing_quit_and_remaining_helpers_cannot_become_stopped() {
        let mut f = fake();
        f.quit_allowed = false;
        assert_eq!(
            stop(&mut f, "source", Some(&bound())),
            Err(Failure::QuitRefused)
        );
        let mut f = fake();
        f.helper_remains = true;
        assert_eq!(
            stop(&mut f, "source", Some(&bound())),
            Err(Failure::QuitTimeout)
        );
        assert_eq!(f.waits, 100);
    }
    #[test]
    fn timeout_and_source_drift_after_quit_refuse() {
        let mut f = fake();
        f.stop_after_quit = false;
        assert_eq!(
            stop(&mut f, "source", Some(&bound())),
            Err(Failure::QuitTimeout)
        );
        let mut f = fake();
        f.source_after_quit = true;
        assert_eq!(
            stop(&mut f, "source", Some(&bound())),
            Err(Failure::SourceChanged)
        );
    }
    #[test]
    fn already_stopped_needs_no_quit_but_old_restart_pid_is_rejected() {
        let mut f = fake();
        f.writers = WriterState::Stopped;
        f.instance = None;
        stop(&mut f, "source", None).unwrap();
        assert_eq!(f.quits, 0);
        f.launch_old = true;
        assert_eq!(
            restart(&mut f, "source", Some(&bound())).err(),
            Some(Failure::RestartFailed)
        );
    }
    #[test]
    fn unknown_writers_never_request_quit() {
        let mut f = fake();
        f.writers = WriterState::Unknown;
        assert!(stop(&mut f, "source", Some(&bound())).is_err());
        assert_eq!(f.quits, 0);
    }
}
