//! WorkBuddy-only switch orchestration. Platform drivers must verify the exact
//! installation, process identity and auth preimage across normal termination.
use super::native_transaction::{Failure, Outcome};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

// Single process-wide gate for the eventual installed-app entry point; it is
// not a durable transaction owner and carries no request or account data.
static SWITCH_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static LAUNCH_PENDING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn launch_pending() -> &'static Arc<AtomicBool> {
    LAUNCH_PENDING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

#[cfg(all(feature = "gui", target_os = "macos", target_arch = "aarch64"))]
pub(super) fn hold_launcher(child: std::process::Child) {
    hold_launcher_with_state(child, launch_pending().clone());
}

#[cfg(any(
    test,
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
fn hold_launcher_with_state(mut child: std::process::Child, pending: Arc<AtomicBool>) {
    pending.store(true, Ordering::Release);
    std::thread::spawn(move || {
        // A wait error is not proof of termination: keep the lease closed.
        if child.wait().is_ok() {
            pending.store(false, Ordering::Release);
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stop {
    AlreadyStopped,
    StoppedByUs,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Restart {
    NotNeeded,
    Requested,
    Failed,
    Unknown,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Error {
    Busy,
    QuitRefused,
    QuitTimeout,
    WritersRemaining,
    SourceChanged,
    Transaction(Failure),
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Report {
    pub transaction: Outcome,
    pub restart: Restart,
}
pub(super) trait Driver {
    fn stop_and_recheck(&mut self) -> Result<Stop, Error>;
    fn restart(&mut self) -> Restart;
}

pub(super) async fn run<F, Fut>(driver: &mut impl Driver, apply: F) -> Result<Report, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Outcome, Failure>>,
{
    run_with_lease(&SWITCH_GATE, launch_pending(), driver, apply).await
}

#[cfg(test)]
pub(super) async fn run_with_gate<F, Fut>(
    gate: &tokio::sync::Mutex<()>,
    driver: &mut impl Driver,
    apply: F,
) -> Result<Report, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Outcome, Failure>>,
{
    let pending = AtomicBool::new(false);
    run_with_lease(gate, &pending, driver, apply).await
}

async fn run_with_lease<F, Fut>(
    gate: &tokio::sync::Mutex<()>,
    pending: &AtomicBool,
    driver: &mut impl Driver,
    apply: F,
) -> Result<Report, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Outcome, Failure>>,
{
    let _guard = gate.try_lock().map_err(|_| Error::Busy)?;
    if pending.load(Ordering::Acquire) {
        return Err(Error::Busy);
    }
    let stopped = driver.stop_and_recheck()?;
    let transaction = apply().await.map_err(Error::Transaction)?;
    if !matches!(transaction, Outcome::Applied | Outcome::AlreadyApplied) {
        return Err(Error::Transaction(Failure::RecoveryRequired));
    }
    let restart = match stopped {
        Stop::AlreadyStopped => Restart::NotNeeded,
        Stop::StoppedByUs => match driver.restart() {
            Restart::NotNeeded => Restart::Unknown,
            value => value,
        },
    };
    Ok(Report {
        transaction,
        restart,
    })
}

#[cfg(all(feature = "gui", target_os = "macos", target_arch = "aarch64"))]
pub(super) async fn run_existing_transaction<F, Fut>(apply: F) -> Result<Outcome, Failure>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Outcome, Failure>>,
{
    run_existing_with_lease(&SWITCH_GATE, launch_pending(), apply).await
}

#[cfg(any(
    test,
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
async fn run_existing_with_lease<F, Fut>(
    gate: &tokio::sync::Mutex<()>,
    pending: &AtomicBool,
    apply: F,
) -> Result<Outcome, Failure>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Outcome, Failure>>,
{
    let _guard = gate.try_lock().map_err(|_| Failure::RequestConflict)?;
    if pending.load(Ordering::Acquire) {
        return Err(Failure::RequestConflict);
    }
    apply().await
}

#[cfg(test)]
mod lease_tests {
    use super::*;
    use std::process::{Child, ChildStdin, Command, Stdio};
    struct Fake {
        stops: usize,
        pending_on_restart: bool,
        pending: Arc<AtomicBool>,
    }
    impl Driver for Fake {
        fn stop_and_recheck(&mut self) -> Result<Stop, Error> {
            self.stops += 1;
            Ok(Stop::StoppedByUs)
        }
        fn restart(&mut self) -> Restart {
            if self.pending_on_restart {
                self.pending.store(true, Ordering::SeqCst);
                Restart::Unknown
            } else {
                Restart::Requested
            }
        }
    }
    #[tokio::test]
    async fn unresolved_launcher_blocks_next_request_until_confirmed_exit() {
        let pending = Arc::new(AtomicBool::new(false));
        let gate = tokio::sync::Mutex::new(());
        let mut first = Fake {
            stops: 0,
            pending_on_restart: true,
            pending: pending.clone(),
        };
        let result = run_with_lease(&gate, &pending, &mut first, || async {
            Ok(Outcome::Applied)
        })
        .await
        .unwrap();
        assert_eq!(result.restart, Restart::Unknown);
        let mut second = Fake {
            stops: 0,
            pending_on_restart: false,
            pending: pending.clone(),
        };
        assert_eq!(
            run_with_lease(&gate, &pending, &mut second, || async {
                Ok(Outcome::Applied)
            })
            .await,
            Err(Error::Busy)
        );
        assert_eq!(second.stops, 0);
        pending.store(false, Ordering::SeqCst);
        assert!(run_with_lease(&gate, &pending, &mut second, || async {
            Ok(Outcome::Applied)
        })
        .await
        .is_ok());
        assert_eq!(second.stops, 1);
    }

    fn waiting_launcher(exit_code: u8) -> (Child, ChildStdin) {
        let script = if exit_code == 0 {
            "read line || :; exit 0"
        } else {
            "read line || :; exit 7"
        };
        let mut child = Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        (child, input)
    }
    #[tokio::test]
    async fn delayed_launcher_keeps_new_and_existing_transactions_busy_until_exit() {
        for exit in [0, 7] {
            let pending = Arc::new(AtomicBool::new(false));
            let gate = tokio::sync::Mutex::new(());
            let (child, input) = waiting_launcher(exit);
            hold_launcher_with_state(child, pending.clone());
            let mut next = Fake {
                stops: 0,
                pending_on_restart: false,
                pending: pending.clone(),
            };
            assert_eq!(
                run_with_lease(&gate, &pending, &mut next, || async {
                    Ok(Outcome::Applied)
                })
                .await,
                Err(Error::Busy)
            );
            assert_eq!(
                run_existing_with_lease(&gate, &pending, || async { Ok(Outcome::Applied) }).await,
                Err(Failure::RequestConflict)
            );
            assert_eq!(next.stops, 0);
            drop(input);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while pending.load(Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(run_with_lease(&gate, &pending, &mut next, || async {
                Ok(Outcome::Applied)
            })
            .await
            .is_ok());
            assert_eq!(next.stops, 1);
        }
    }
    #[tokio::test]
    async fn dropping_waiter_does_not_release_pending_launcher_lease() {
        struct Launching {
            child: Option<Child>,
            pending: Arc<AtomicBool>,
            started: Option<tokio::sync::oneshot::Sender<()>>,
            resume: std::sync::mpsc::Receiver<()>,
        }
        impl Driver for Launching {
            fn stop_and_recheck(&mut self) -> Result<Stop, Error> {
                Ok(Stop::StoppedByUs)
            }
            fn restart(&mut self) -> Restart {
                hold_launcher_with_state(self.child.take().unwrap(), self.pending.clone());
                let _ = self.started.take().unwrap().send(());
                self.resume.recv().unwrap();
                Restart::Unknown
            }
        }
        let pending = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let (child, input) = waiting_launcher(0);
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let (finished, wait_finished) = tokio::sync::oneshot::channel();
        let worker_gate = gate.clone();
        let worker_pending = pending.clone();
        let runtime = tokio::runtime::Handle::current();
        let worker = tokio::task::spawn_blocking(move || {
            let result = runtime.block_on(async move {
                let mut driver = Launching {
                    child: Some(child),
                    pending: worker_pending.clone(),
                    started: Some(started),
                    resume,
                };
                run_with_lease(&worker_gate, &worker_pending, &mut driver, || async {
                    Ok(Outcome::Applied)
                })
                .await
            });
            let _ = finished.send(result);
        });
        wait_started.await.unwrap();
        drop(worker);
        let mut next = Fake {
            stops: 0,
            pending_on_restart: false,
            pending: pending.clone(),
        };
        assert_eq!(
            run_with_lease(&gate, &pending, &mut next, || async {
                Ok(Outcome::Applied)
            })
            .await,
            Err(Error::Busy)
        );
        assert_eq!(next.stops, 0);
        release.send(()).unwrap();
        assert_eq!(
            wait_finished.await.unwrap().unwrap().restart,
            Restart::Unknown
        );
        assert_eq!(
            run_with_lease(&gate, &pending, &mut next, || async {
                Ok(Outcome::Applied)
            })
            .await,
            Err(Error::Busy)
        );
        drop(input);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while pending.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(run_with_lease(&gate, &pending, &mut next, || async {
            Ok(Outcome::Applied)
        })
        .await
        .is_ok());
    }
}
