//! Explicit process-observation fixtures; original sealed fixture source and production transaction IO.
use super::super::process::{self, Process, ProcessBound, Processes, Scope};
use super::*;

struct Probe {
    rows: Vec<Process>,
    unavailable: bool,
}
impl process::sealed::Sealed for Probe {}
impl Processes for Probe {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure> {
        if self.unavailable {
            Err(Failure::SourceChanged)
        } else {
            Ok(self.rows.clone())
        }
    }
}
fn install(f: &Fixture) -> (Scope, PathBuf) {
    let bundle = f._root.path().join("SyntheticWorkBuddy.app");
    let executable = bundle.join("Contents/MacOS/SyntheticWorkBuddy");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(
        &executable,
        b"synthetic public executable fixture; never executed",
    )
    .unwrap();
    (Scope::at(&bundle, &executable).unwrap(), executable)
}
fn journal(f: &Fixture) -> PathBuf {
    f.session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
}

#[tokio::test]
async fn process_admission_refuses_main_and_helper_before_any_native_or_journal_write() {
    for helper in [false, true] {
        let f = Fixture::new().await;
        let b = f.binding();
        let c = f.controller(&b);
        let target = c.select(&f.key()).await.unwrap();
        let (scope, main) = install(&f);
        let running = if helper {
            main.parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("Frameworks/Renderer Helper.app/Contents/MacOS/Renderer Helper")
        } else {
            main
        };
        if helper {
            std::fs::create_dir_all(running.parent().unwrap()).unwrap();
            std::fs::write(&running, b"synthetic helper; never executed").unwrap();
        }
        let mut source = ProcessBound::new(
            f.port(),
            scope,
            Probe {
                rows: vec![Process {
                    id: 101,
                    start_identity: [100, 1],
                    executable: running,
                }],
                unavailable: false,
            },
        );
        let before = std::fs::read(f.auth()).unwrap();
        let accounts = std::fs::read(f.session.root().join("workbuddy_accounts.json")).unwrap();
        assert!(
            c.prepare(&target, &mut source, &mut Files).await.is_err(),
            "running main/helper bypassed stopped admission"
        );
        assert_eq!(std::fs::read(f.auth()).unwrap(), before);
        assert_eq!(
            std::fs::read(f.session.root().join("workbuddy_accounts.json")).unwrap(),
            accounts
        );
        assert!(!journal(&f).exists());
    }
}

#[tokio::test]
async fn process_admission_never_treats_failed_observation_as_stopped() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, _) = install(&f);
    let mut source = ProcessBound::new(
        f.port(),
        scope,
        Probe {
            rows: vec![],
            unavailable: true,
        },
    );
    let before = std::fs::read(f.auth()).unwrap();
    assert!(c.prepare(&target, &mut source, &mut Files).await.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert!(!journal(&f).exists());
}

#[tokio::test]
async fn process_admission_quiet_observation_preserves_original_four_block_apply() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, _) = install(&f);
    let mut source = ProcessBound::new(
        f.port(),
        scope,
        Probe {
            rows: quiet(),
            unavailable: false,
        },
    );
    let before: Value = serde_json::from_slice(&std::fs::read(f.auth()).unwrap()).unwrap();
    let plan = c.prepare(&target, &mut source, &mut Files).await.unwrap();
    assert_eq!(
        c.apply(&plan, &mut source, &mut Files).await.unwrap(),
        Outcome::Applied
    );
    let after: Value = serde_json::from_slice(&std::fs::read(f.auth()).unwrap()).unwrap();
    assert_eq!(after["account"]["uid"], "B");
    assert_eq!(after["preserve"], before["preserve"]);
    assert!(!f.client.join("workbuddy-desktop.info.logged-out").exists());
}

fn quiet() -> Vec<Process> {
    vec![Process {
        id: 7,
        start_identity: [100, 1],
        executable: std::env::current_exe().unwrap().canonicalize().unwrap(),
    }]
}
struct LiveProbe(Rc<std::cell::RefCell<Vec<Process>>>);
impl process::sealed::Sealed for LiveProbe {}
impl Processes for LiveProbe {
    fn snapshot(&mut self) -> Result<Vec<Process>, Failure> {
        Ok(self.0.borrow().clone())
    }
}

#[tokio::test]
async fn process_admission_rechecks_restart_after_waiting_for_original_owner_lock() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, main) = install(&f);
    let rows = Rc::new(std::cell::RefCell::new(quiet()));
    let mut source = ProcessBound::new(f.port(), scope, LiveProbe(rows.clone()));
    let held = crate::services::sync_protocol::sync_mutex().lock().await;
    let mut io = Files;
    let mut pending = Box::pin(c.prepare(&target, &mut source, &mut io));
    assert!(matches!(
        futures::poll!(&mut pending),
        std::task::Poll::Pending
    ));
    rows.borrow_mut().push(Process {
        id: 101,
        start_identity: [100, 1],
        executable: main,
    });
    let before = std::fs::read(f.auth()).unwrap();
    drop(held);
    assert!(pending.await.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert!(!journal(&f).exists());
}

#[tokio::test]
async fn process_admission_rechecks_restart_before_apply_without_touching_receipt() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, main) = install(&f);
    let rows = Rc::new(std::cell::RefCell::new(quiet()));
    let mut source = ProcessBound::new(f.port(), scope, LiveProbe(rows.clone()));
    let plan = c.prepare(&target, &mut source, &mut Files).await.unwrap();
    let before = std::fs::read(f.auth()).unwrap();
    let journal_before = std::fs::read(journal(&f)).unwrap();
    rows.borrow_mut().push(Process {
        id: 101,
        start_identity: [100, 1],
        executable: main,
    });
    assert!(c.apply(&plan, &mut source, &mut Files).await.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert_eq!(std::fs::read(journal(&f)).unwrap(), journal_before);
}

struct ProcessInterrupted {
    rows: Rc<std::cell::RefCell<Vec<Process>>>,
    running: PathBuf,
    wrote: bool,
}
impl FileIo for ProcessInterrupted {
    fn read(&mut self, p: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        Files.read(p)
    }
    fn remove(&mut self, p: &Path) -> Result<(), Failure> {
        Files.remove(p)
    }
    fn write(&mut self, p: &Path, bytes: &[u8]) -> Result<(), Failure> {
        Files.write(p, bytes)?;
        if !self.wrote && p.file_name().unwrap() == "workbuddy-desktop.info" {
            self.wrote = true;
            self.rows.borrow_mut().push(Process {
                id: 101,
                start_identity: [100, 1],
                executable: self.running.clone(),
            });
        }
        Ok(())
    }
}
#[tokio::test]
async fn process_admission_post_write_restart_preserves_pending_then_original_recovery() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, main) = install(&f);
    let rows = Rc::new(std::cell::RefCell::new(quiet()));
    let mut source = ProcessBound::new(f.port(), scope, LiveProbe(rows.clone()));
    let plan = c.prepare(&target, &mut source, &mut Files).await.unwrap();
    let before = std::fs::read(f.auth()).unwrap();
    let marker = std::fs::read(f.client.join("workbuddy-desktop.info.logged-out")).unwrap();
    let mut io = ProcessInterrupted {
        rows: rows.clone(),
        running: main,
        wrote: false,
    };
    assert_eq!(
        c.apply(&plan, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    assert_ne!(std::fs::read(f.auth()).unwrap(), before);
    let pending = std::fs::read(journal(&f)).unwrap();
    assert!(c.recover(&mut source, &mut Files).await.is_err());
    assert_eq!(std::fs::read(journal(&f)).unwrap(), pending);
    assert_ne!(std::fs::read(f.auth()).unwrap(), before);
    *rows.borrow_mut() = quiet();
    // A fresh driver uses the original persisted owner intent, not a new ticket.
    let fresh = f.controller(&b);
    assert_eq!(
        fresh.recover(&mut source, &mut Files).await.unwrap(),
        Outcome::Restored
    );
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert_eq!(
        std::fs::read(f.client.join("workbuddy-desktop.info.logged-out")).unwrap(),
        marker
    );
    assert_eq!(
        fresh.recover(&mut source, &mut Files).await.unwrap(),
        Outcome::Restored
    );
}

#[tokio::test]
async fn process_admission_quiet_probe_never_replaces_the_source_proof() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, _) = install(&f);
    let mut origin = f.port();
    origin.verified = false;
    let mut source = ProcessBound::new(
        origin,
        scope,
        Probe {
            rows: quiet(),
            unavailable: false,
        },
    );
    let before = std::fs::read(f.auth()).unwrap();
    assert!(c.prepare(&target, &mut source, &mut Files).await.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert!(!journal(&f).exists());
}

#[tokio::test]
async fn process_admission_rejects_hardlink_alias_outside_bundle_with_another_name() {
    let f = Fixture::new().await;
    let b = f.binding();
    let c = f.controller(&b);
    let target = c.select(&f.key()).await.unwrap();
    let (scope, main) = install(&f);
    let alias = f._root.path().join("different-main-name");
    std::fs::hard_link(&main, &alias).unwrap();
    let mut rows = quiet();
    rows.push(Process {
        id: 101,
        start_identity: [100, 1],
        executable: alias,
    });
    let mut source = ProcessBound::new(
        f.port(),
        scope,
        Probe {
            rows,
            unavailable: false,
        },
    );
    let before = std::fs::read(f.auth()).unwrap();
    let accounts = std::fs::read(f.session.root().join("workbuddy_accounts.json")).unwrap();
    assert!(
        c.prepare(&target, &mut source, &mut Files).await.is_err(),
        "running same-inode main hardlink bypassed stopped admission"
    );
    assert_eq!(std::fs::read(f.auth()).unwrap(), before);
    assert_eq!(
        std::fs::read(f.session.root().join("workbuddy_accounts.json")).unwrap(),
        accounts
    );
    assert!(!journal(&f).exists());
}
