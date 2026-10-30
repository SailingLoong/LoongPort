//! Synthetic real files and original controller, owner and journal.
use super::*;

async fn activate_once(
    controller: &Controller<'_>,
    key: &SelectionKey,
    source: &mut impl ControlledSource,
    io: &mut dyn FileIo,
) -> Result<Outcome, Failure> {
    controller.activate(key, source, io).await
}

async fn journal_realm(
    controller: &Controller<'_>,
) -> Result<crate::workbuddy::model::Domain, Failure> {
    controller.journal_binding().await.map(|(realm, _)| realm)
}

async fn pending_stamp(controller: &Controller<'_>) -> crate::workbuddy::native_transaction::Stamp {
    crate::workbuddy::native_owner::with_selected_owner(
        controller.catalog.session,
        controller.catalog.identity,
        controller.catalog.epoch,
        controller.catalog.selection,
        |owner| Ok(owner.load()?.0),
    )
    .await
    .unwrap()
}
async fn recover_previous(
    controller: &Controller<'_>,
    source: &mut impl ControlledSource,
    original: &crate::workbuddy::native_transaction::Stamp,
) -> Result<Outcome, Failure> {
    controller
        .recover_bound(source, &mut Files, Some(original))
        .await
}

#[tokio::test]
async fn recovery_never_rolls_back_a_new_same_realm_pending_intent() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&plan, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    let original = pending_stamp(&c).await;
    source.stopped.set(true);
    assert_eq!(
        c.recover(&mut source, &mut Files).await.unwrap(),
        Outcome::Restored
    );
    let second = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&second, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    assert!(original != pending_stamp(&c).await);
    source.stopped.set(true);
    let journal = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    let journal_before = std::fs::read(&journal).unwrap();
    let auth_before = std::fs::read(f.auth()).unwrap();
    assert_eq!(
        recover_previous(&c, &mut source, &original).await,
        Err(Failure::RecoveryRequired)
    );
    assert_eq!(std::fs::read(journal).unwrap(), journal_before);
    assert_eq!(std::fs::read(f.auth()).unwrap(), auth_before);
}

struct FailNativeWrite {
    index: usize,
    count: usize,
    after: bool,
}
impl FileIo for FailNativeWrite {
    fn read(&mut self, p: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        Files.read(p)
    }
    fn write(&mut self, p: &Path, bytes: &[u8]) -> Result<(), Failure> {
        self.count += 1;
        if self.count == self.index && !self.after {
            return Err(Failure::Io);
        }
        Files.write(p, bytes)?;
        if self.count == self.index && self.after {
            return Err(Failure::Io);
        }
        Ok(())
    }
    fn remove(&mut self, p: &Path) -> Result<(), Failure> {
        self.count += 1;
        if self.count == self.index && !self.after {
            return Err(Failure::Io);
        }
        Files.remove(p)?;
        if self.count == self.index && self.after {
            return Err(Failure::Io);
        }
        Ok(())
    }
}
#[tokio::test]
async fn every_native_write_failure_restores_known_real_files() {
    for index in [1, 2] {
        for after in [false, true] {
            let f = Fixture::new().await;
            let binding = f.binding();
            let c = f.controller(&binding);
            let mut source = f.port();
            let original = std::fs::read(f.auth()).unwrap();
            let marker = f.client.join("workbuddy-desktop.info.logged-out");
            let original_marker = std::fs::read(&marker).unwrap();
            let mut io = FailNativeWrite {
                index,
                count: 0,
                after,
            };
            assert_eq!(
                activate_once(&c, &f.key(), &mut source, &mut io).await,
                Err(Failure::RolledBack)
            );
            assert_eq!(std::fs::read(f.auth()).unwrap(), original);
            assert_eq!(std::fs::read(marker).unwrap(), original_marker);
            assert_eq!(
                crate::workbuddy::native_owner::inspect(
                    f.session.root(),
                    &f.session.read().unwrap()
                )
                .unwrap(),
                crate::workbuddy::native_owner::Disposition::Clear
            );
        }
    }
}

struct BlockFinalJournal {
    root: PathBuf,
}
impl FileIo for BlockFinalJournal {
    fn read(&mut self, p: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        Files.read(p)
    }
    fn write(&mut self, p: &Path, bytes: &[u8]) -> Result<(), Failure> {
        use std::os::unix::fs::PermissionsExt;
        Files.write(p, bytes)?;
        std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o500)).unwrap();
        Ok(())
    }
    fn remove(&mut self, p: &Path) -> Result<(), Failure> {
        Files.remove(p)
    }
}
#[tokio::test]
async fn failed_final_journal_write_keeps_pending_until_original_recovery() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let mut source = f.port();
    let original = std::fs::read(f.auth()).unwrap();
    let mut io = BlockFinalJournal {
        root: f.session.root().into(),
    };
    let result = activate_once(&c, &f.key(), &mut source, &mut io).await;
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, Err(Failure::RecoveryRequired));
    assert_eq!(
        crate::workbuddy::native_owner::inspect(f.session.root(), &f.session.read().unwrap())
            .unwrap(),
        crate::workbuddy::native_owner::Disposition::Pending
    );
    assert_eq!(
        c.recover(&mut source, &mut Files).await.unwrap(),
        Outcome::Restored
    );
    assert_eq!(std::fs::read(f.auth()).unwrap(), original);
}

#[tokio::test]
async fn failed_initial_journal_creation_never_writes_native_files() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let auth = std::fs::read(f.auth()).unwrap();
    let marker_path = f.client.join("workbuddy-desktop.info.logged-out");
    let marker = std::fs::read(&marker_path).unwrap();
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = c.prepare(&selected, &mut source, &mut Files).await;
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(result, Err(Failure::Journal)));
    assert_eq!(std::fs::read(f.auth()).unwrap(), auth);
    assert_eq!(std::fs::read(marker_path).unwrap(), marker);
    assert!(!f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
}

#[tokio::test]
async fn cancelled_activation_waiter_may_commit_after_unknown_state_query() {
    let f = Fixture::new().await;
    let session = f.session.clone();
    let worker_session = session.clone();
    let selection = f.selection.clone();
    let binding = f.binding();
    let c = f.controller(&binding);
    let client = f.client.clone();
    let context = f.context.clone();
    let key = f.key();
    let (release, start) = std::sync::mpsc::channel::<()>();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (finished, wait_finished) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let worker = crate::workbuddy::native_capture::schedule_capture(
        &session,
        || Ok(selection),
        move |identity, epoch, selection| {
            let _ = started.send(());
            if start.recv().is_err() {
                return;
            }
            let result = runtime.block_on(async {
                let c = Controller {
                    catalog: Catalog {
                        session: &worker_session,
                        identity: &identity,
                        epoch,
                        selection: &selection,
                    },
                };
                let mut source = FixtureSource {
                    root_pin: std::fs::File::open(&client).unwrap(),
                    root: client,
                    context: context.clone(),
                    expected: context,
                    stopped: Rc::new(Cell::new(true)),
                    verified: true,
                };
                c.activate(&key, &mut source, &mut Files).await
            });
            let _ = finished.send(result);
        },
    )
    .unwrap();
    wait_started.await.unwrap();
    drop(worker);
    let status = serde_json::to_value(
        crate::workbuddy::native_activation::state(&c.catalog)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["phase"], "absent");
    assert_eq!(status["requestOutcome"], "unknown");
    release.send(()).unwrap();
    assert_eq!(wait_finished.await.unwrap(), Ok(Outcome::Applied));
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(f.auth()).unwrap()).unwrap()["account"]
            ["uid"],
        "B"
    );
}

#[tokio::test]
async fn activation_refuses_running_source_and_stale_incarnation_before_journal() {
    for kind in ["running", "incarnation"] {
        let f = Fixture::new().await;
        let binding = f.binding();
        let c = f.controller(&binding);
        let mut source = f.port();
        let mut key = f.key();
        if kind == "running" {
            source.stopped.set(false);
        } else {
            key.incarnation = uuid::Uuid::nil();
        }
        let before = std::fs::read(f.auth()).unwrap();
        assert!(activate_once(&c, &key, &mut source, &mut Files)
            .await
            .is_err());
        assert_eq!(std::fs::read(f.auth()).unwrap(), before);
        assert!(!f
            .session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
            .exists());
    }
}

#[tokio::test]
async fn concurrent_explicit_activations_never_publish_conflicting_images() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let key = f.key();
    let mut a = f.port();
    let mut b = f.port();
    let mut io_a = Files;
    let mut io_b = Files;
    let held = crate::services::sync_protocol::sync_mutex().lock().await;
    let mut first = Box::pin(activate_once(&c, &key, &mut a, &mut io_a));
    let mut second = Box::pin(activate_once(&c, &key, &mut b, &mut io_b));
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());
    drop(held);
    let (a, b) = tokio::join!(first, second);
    assert_eq!(
        usize::from(a == Ok(Outcome::Applied)) + usize::from(b == Ok(Outcome::Applied)),
        1
    );
    assert!(a.is_err() || b.is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(f.auth()).unwrap()).unwrap()["account"]
            ["uid"],
        "B"
    );
    assert_eq!(
        crate::workbuddy::native_owner::inspect(f.session.root(), &f.session.read().unwrap())
            .unwrap(),
        crate::workbuddy::native_owner::Disposition::Clear
    );
}

#[tokio::test]
async fn concurrent_different_targets_do_not_apply_a_stale_preimage() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let marker = f.client.join("workbuddy-desktop.info.logged-out");
    let old_marker = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&marker).unwrap();
    let a = Observation::read(&f.auth(), &f.context).unwrap();
    let key_a = c
        .catalog
        .save(&a, None, "Synthetic A")
        .await
        .unwrap()
        .selection_key()
        .unwrap();
    std::fs::write(&marker, old_marker).unwrap();
    let key_b = f.key();
    let mut source_a = f.port();
    let mut source_b = f.port();
    let mut io_a = Files;
    let mut io_b = Files;
    let held = crate::services::sync_protocol::sync_mutex().lock().await;
    let mut first = Box::pin(activate_once(&c, &key_a, &mut source_a, &mut io_a));
    let mut second = Box::pin(activate_once(&c, &key_b, &mut source_b, &mut io_b));
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());
    drop(held);
    let (a, b) = tokio::join!(first, second);
    assert_eq!(
        usize::from(a == Ok(Outcome::Applied)) + usize::from(b == Ok(Outcome::Applied)),
        1
    );
    assert!(a.is_err() || b.is_err());
    let uid = serde_json::from_slice::<Value>(&std::fs::read(f.auth()).unwrap()).unwrap()
        ["account"]["uid"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(uid, if a == Ok(Outcome::Applied) { "A" } else { "B" });
}

#[tokio::test]
async fn sequential_explicit_same_target_requests_are_separate_local_transactions() {
    struct CountWrites(usize);
    impl FileIo for CountWrites {
        fn read(&mut self, path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
            Files.read(path)
        }
        fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<(), Failure> {
            self.0 += 1;
            Files.write(path, bytes)
        }
        fn remove(&mut self, path: &Path) -> Result<(), Failure> {
            Files.remove(path)
        }
    }
    let f = Fixture::new().await;
    let binding = f.binding();
    let controller = f.controller(&binding);
    let mut source = f.port();
    let mut io = CountWrites(0);
    assert_eq!(
        activate_once(&controller, &f.key(), &mut source, &mut io).await,
        Ok(Outcome::Applied)
    );
    assert_eq!(io.0, 1);
    let after_first = std::fs::read(f.auth()).unwrap();
    assert_eq!(
        activate_once(&controller, &f.key(), &mut source, &mut io).await,
        Ok(Outcome::Applied)
    );
    assert_eq!(io.0, 2);
    assert_eq!(std::fs::read(f.auth()).unwrap(), after_first);
}

#[tokio::test]
async fn recovery_status_is_redacted_and_does_not_claim_a_request_finished() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let wire = serde_json::to_value(
        crate::workbuddy::native_activation::state(&c.catalog)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        wire,
        json!({"phase":"absent", "requestOutcome":"unknown", "canRequestActivation":true})
    );
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&plan, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    let wire = serde_json::to_value(
        crate::workbuddy::native_activation::state(&c.catalog)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        wire,
        json!({"phase":"pending", "requestOutcome":"unknown", "canRequestActivation":false})
    );
}

#[tokio::test]
async fn single_action_activates_a_b_a_with_original_saved_lifetimes() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    std::fs::remove_file(f.client.join("workbuddy-desktop.info.logged-out")).unwrap();
    let observed = Observation::read(&f.auth(), &f.context).unwrap();
    let key_a = c
        .catalog
        .save(&observed, None, "Synthetic A")
        .await
        .unwrap()
        .selection_key()
        .unwrap();
    let mut source = f.port();
    let b = activate_once(&c, &f.key(), &mut source, &mut Files)
        .await
        .unwrap();
    assert_eq!(b, Outcome::Applied);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(f.auth()).unwrap()).unwrap()["account"]
            ["uid"],
        "B"
    );
    let a = activate_once(&c, &key_a, &mut source, &mut Files)
        .await
        .unwrap();
    assert_eq!(a, Outcome::Applied);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(f.auth()).unwrap()).unwrap()["account"]
            ["uid"],
        "A"
    );
}

#[tokio::test]
async fn recovery_realm_comes_from_original_pending_journal_when_auth_is_missing() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&plan, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    let journal = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    let before = std::fs::read(&journal).unwrap();
    std::fs::remove_file(f.auth()).unwrap();
    assert_eq!(
        journal_realm(&c).await.unwrap(),
        crate::workbuddy::model::Domain::WorkBuddy
    );
    assert_eq!(std::fs::read(journal).unwrap(), before);
}

#[tokio::test]
async fn queued_recovery_does_not_bind_to_a_later_pending_journal() {
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let selected = c.select(&f.key()).await.unwrap();
    let mut source = f.port();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&plan, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    let session = f.session.clone();
    let worker_session = session.clone();
    let selection = f.selection.clone();
    let client = f.client.clone();
    let context = f.context.clone();
    let (release, start) = std::sync::mpsc::channel::<()>();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (finished, wait_finished) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let worker = crate::workbuddy::native_activation::schedule_recovery(
        &session,
        || Ok(selection),
        move |identity, epoch, selection, _realm, stamp| {
            let _ = started.send(());
            if start.recv().is_err() {
                return;
            }
            let result = runtime.block_on(async {
                let c = Controller {
                    catalog: Catalog {
                        session: &worker_session,
                        identity: &identity,
                        epoch,
                        selection: &selection,
                    },
                };
                let mut source = FixtureSource {
                    root_pin: std::fs::File::open(&client).unwrap(),
                    root: client,
                    context: context.clone(),
                    expected: context,
                    stopped: Rc::new(Cell::new(true)),
                    verified: true,
                };
                c.recover_bound(&mut source, &mut Files, Some(&stamp)).await
            });
            let _ = finished.send(result);
        },
    )
    .await
    .unwrap();
    wait_started.await.unwrap();
    source.stopped.set(true);
    assert_eq!(
        c.recover(&mut source, &mut Files).await.unwrap(),
        Outcome::Restored
    );
    let second = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&second, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    let journal = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    let before = std::fs::read(&journal).unwrap();
    let auth = std::fs::read(f.auth()).unwrap();
    release.send(()).unwrap();
    assert_eq!(wait_finished.await.unwrap(), Err(Failure::RecoveryRequired));
    worker.await.unwrap();
    assert_eq!(std::fs::read(journal).unwrap(), before);
    assert_eq!(std::fs::read(f.auth()).unwrap(), auth);
}

#[tokio::test]
async fn queued_activation_refuses_changed_vault_epoch_selector_or_row_lifetime() {
    for kind in ["vault", "epoch", "selector", "generation", "incarnation"] {
        let f = Fixture::new().await;
        let session = f.session.clone();
        let worker_session = session.clone();
        let selection = f.selection.clone();
        let client = f.client.clone();
        let context = f.context.clone();
        let key = f.key();
        let control = f._root.path().join("control/app_paths.json");
        let before = std::fs::read(f.auth()).unwrap();
        let (release, start) = std::sync::mpsc::channel::<()>();
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let (finished, wait_finished) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Handle::current();
        let worker = crate::workbuddy::native_capture::schedule_capture(
            &session,
            || Ok(selection),
            move |identity, epoch, selection| {
                let _ = started.send(());
                if start.recv().is_err() {
                    return;
                }
                let result = runtime.block_on(async {
                    let c = Controller {
                        catalog: Catalog {
                            session: &worker_session,
                            identity: &identity,
                            epoch,
                            selection: &selection,
                        },
                    };
                    let mut source = FixtureSource {
                        root_pin: std::fs::File::open(&client).unwrap(),
                        root: client,
                        context: context.clone(),
                        expected: context,
                        stopped: Rc::new(Cell::new(true)),
                        verified: true,
                    };
                    c.activate(&key, &mut source, &mut Files).await
                });
                let _ = finished.send(result);
            },
        )
        .unwrap();
        wait_started.await.unwrap();
        match kind {
            "vault" => {
                *session.write().unwrap() = crate::secrets::storage::StorageContext::Encrypted(
                    VaultContext::generate().unwrap(),
                )
            }
            "epoch" => session.invalidate_database_leases(),
            "selector" => std::fs::write(control, b"invalid selector").unwrap(),
            _ => {
                let binding = store::binding(&session).unwrap();
                let mut rows =
                    serde_json::to_value(store::load(&session, &binding).unwrap()).unwrap();
                if kind == "generation" {
                    rows[0]["generation"] = json!(f.receipt.generation + 1);
                } else {
                    rows[0]["account"]["native"]["incarnation"] =
                        json!("33333333-3333-4333-8333-333333333333");
                }
                store::save(&session, &binding, serde_json::from_value(rows).unwrap()).unwrap();
            }
        }
        release.send(()).unwrap();
        assert!(
            wait_finished.await.unwrap().is_err(),
            "accepted changed {kind}"
        );
        worker.await.unwrap();
        assert_eq!(std::fs::read(f.auth()).unwrap(), before);
        assert!(!session
            .root()
            .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
            .exists());
    }
}

#[test]
fn transaction_result_dto_has_only_fixed_local_outcome_strings() {
    assert_eq!(
        serde_json::to_value(Outcome::Applied).unwrap(),
        json!("applied")
    );
    assert_eq!(
        serde_json::to_value(Outcome::AlreadyApplied).unwrap(),
        json!("alreadyApplied")
    );
    assert_eq!(
        serde_json::to_value(Outcome::Restored).unwrap(),
        json!("restored")
    );
}
#[tokio::test]
async fn failed_pending_journal_write_never_touches_native_preimage() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let mut source = f.port();
    let selected = c.select(&f.key()).await.unwrap();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let auth = std::fs::read(f.auth()).unwrap();
    let marker_path = f.client.join("workbuddy-desktop.info.logged-out");
    let marker = std::fs::read(&marker_path).unwrap();
    let journal_path = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    let journal = std::fs::read(&journal_path).unwrap();
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = c.apply(&plan, &mut source, &mut Files).await;
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read(f.auth()).unwrap(), auth);
    assert_eq!(std::fs::read(marker_path).unwrap(), marker);
    assert_eq!(std::fs::read(journal_path).unwrap(), journal);
}

#[tokio::test]
async fn failed_recovery_journal_write_can_be_retried_without_losing_pending() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let binding = f.binding();
    let c = f.controller(&binding);
    let mut source = f.port();
    let auth = std::fs::read(f.auth()).unwrap();
    let selected = c.select(&f.key()).await.unwrap();
    let plan = c.prepare(&selected, &mut source, &mut Files).await.unwrap();
    let mut io = Interrupted {
        files: Files,
        stopped: source.stopped.clone(),
        written: false,
    };
    assert_eq!(
        c.apply(&plan, &mut source, &mut io).await,
        Err(Failure::RecoveryRequired)
    );
    source.stopped.set(true);
    let journal_path = f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE);
    let pending = std::fs::read(&journal_path).unwrap();
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let result = c.recover(&mut source, &mut Files).await;
    std::fs::set_permissions(f.session.root(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, Err(Failure::RecoveryRequired));
    assert_eq!(std::fs::read(&journal_path).unwrap(), pending);
    assert_eq!(
        c.recover(&mut source, &mut Files).await.unwrap(),
        Outcome::Restored
    );
    assert_eq!(std::fs::read(f.auth()).unwrap(), auth);
}
