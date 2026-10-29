//! Real temporary installed bytes, original Catalog/Owner/store. No native client.
use super::*;

async fn capture(
    controller: &Controller<'_>,
    source: &mut impl ControlledSource,
    expected: Option<&SelectionKey>,
    label: &str,
) -> Result<Receipt, Failure> {
    controller.capture(source, expected, label).await
}
async fn save_checked(
    catalog: &Catalog<'_>,
    observation: &Observation,
    check: impl FnMut() -> Result<(), Failure>,
) -> Result<Receipt, Failure> {
    catalog
        .save_checked(observation, None, "Boundary capture", check)
        .await
}
async fn fixture() -> InstalledFixture {
    let f = InstalledFixture::new().await;
    std::fs::remove_file(f.f.client.join("workbuddy-desktop.info.logged-out")).unwrap();
    let binding = f.f.binding();
    store::save(&f.f.session, &binding, vec![]).unwrap();
    f
}
fn accounts(f: &InstalledFixture) -> Vec<u8> {
    std::fs::read(f.f.session.root().join(store::FILE)).unwrap()
}
fn no_journal(f: &InstalledFixture) {
    assert!(!f
        .f
        .session
        .root()
        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE)
        .exists());
}

#[tokio::test]
async fn verified_capture_returns_only_the_original_persistent_row_choice() {
    let f = fixture().await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let mut source = f.controlled(f.origin().unwrap());
    let auth = std::fs::read(f.f.auth()).unwrap();
    let receipt = capture(&c, &mut source, None, "Native A").await.unwrap();
    let wire = serde_json::to_value(receipt.selection_key().unwrap()).unwrap();
    let mut fields = wire
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    fields.sort();
    assert_eq!(fields, ["generation", "id", "incarnation"]);
    assert_eq!(wire["generation"], 0);
    assert_eq!(wire["id"].as_str().unwrap().len(), 64);
    assert!(uuid::Uuid::parse_str(wire["incarnation"].as_str().unwrap()).is_ok());
    let row = c.catalog.load(&receipt).await.unwrap();
    assert_eq!(row.account.uid, "A");
    assert!(!row.account.is_api());
    assert!(!row.automatic_enabled());
    assert!(!row.inference_allowed());
    assert_eq!(std::fs::read(f.f.auth()).unwrap(), auth);
    assert!(!String::from_utf8_lossy(&accounts(&f)).contains("opaque-A"));
    no_journal(&f);
}

#[tokio::test]
async fn queued_capture_rechecks_install_auth_marker_owner_and_recovery() {
    for kind in [
        "installation",
        "auth",
        "marker",
        "epoch",
        "selector",
        "native-pending",
        "bootstrap",
    ] {
        let f = fixture().await;
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let mut source = f.controlled(f.origin().unwrap());
        let held = crate::services::sync_protocol::sync_mutex().lock().await;
        let mut waiting = Box::pin(capture(&c, &mut source, None, "Queued"));
        assert!(futures::poll!(&mut waiting).is_pending());
        match kind {
            "installation" => mutate_install(&f.bundle, "same-length"),
            "auth" => std::fs::write(f.f.auth(), image("externally-replaced")).unwrap(),
            "marker" => std::fs::write(
                f.f.client.join("workbuddy-desktop.info.logged-out"),
                b"external-logout",
            )
            .unwrap(),
            "epoch" => f.f.session.invalidate_database_leases(),
            "selector" => std::fs::write(
                f.f._root.path().join("control/app_paths.json"),
                b"invalid-selector",
            )
            .unwrap(),
            "native-pending" => {
                let pending = crate::workbuddy::native_owner::testing::phase_receipts(
                    f.f.session.root(),
                    &f.f.session.read().unwrap(),
                    &f.f.client,
                )
                .0;
                std::fs::write(
                    f.f.session
                        .root()
                        .join(crate::secrets::owned_file::WORKBUDDY_NATIVE_JOURNAL_FILE),
                    pending,
                )
                .unwrap();
            }
            _ => std::fs::write(
                f.f.session.root().join(".vault-bootstrap"),
                b"pending-bootstrap",
            )
            .unwrap(),
        }
        let before = accounts(&f);
        let auth = std::fs::read(f.f.auth()).unwrap();
        drop(held);
        assert!(waiting.await.is_err(), "captured queued {kind}");
        assert_eq!(accounts(&f), before);
        assert_eq!(std::fs::read(f.f.auth()).unwrap(), auth);
        if kind != "native-pending" {
            no_journal(&f);
        }
    }
}

#[tokio::test]
async fn source_callback_runs_at_final_account_publication_not_only_preflight() {
    let f = fixture().await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let mut source = f.controlled(f.origin().unwrap());
    let observation = Observation::read(&f.f.auth(), source.context()).unwrap();
    let before = accounts(&f);
    let mut first = true;
    let result = save_checked(&c.catalog, &observation, || {
        source.verify_stopped()?;
        if first {
            first = false;
            mutate_install(&f.bundle, "replace");
        }
        Ok(())
    })
    .await;
    assert!(
        result.is_err(),
        "published after the only source preflight became stale"
    );
    assert_eq!(accounts(&f), before);
    no_journal(&f);
}

#[tokio::test]
async fn concurrent_capture_cannot_duplicate_or_replace_the_first_row() {
    let f = fixture().await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let mut first = f.controlled(f.origin().unwrap());
    let mut second = f.controlled(f.origin().unwrap());
    let (a, b) = tokio::join!(
        capture(&c, &mut first, None, "First"),
        capture(&c, &mut second, None, "Second")
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let rows = store::load(&f.f.session, &binding).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].generation, 0);
    no_journal(&f);
}

#[tokio::test]
async fn explicit_recapture_preserves_incarnation_and_rejects_stale_choice() {
    let f = fixture().await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let mut source = f.controlled(f.origin().unwrap());
    let first = capture(&c, &mut source, None, "First")
        .await
        .unwrap()
        .selection_key()
        .unwrap();
    let second = capture(&c, &mut source, Some(&first), "Recaptured")
        .await
        .unwrap()
        .selection_key()
        .unwrap();
    assert_eq!(second.incarnation, first.incarnation);
    assert_eq!(second.generation, 1);
    let before = accounts(&f);
    assert!(capture(&c, &mut source, Some(&first), "Stale")
        .await
        .is_err());
    assert_eq!(accounts(&f), before);
    assert!(capture(&c, &mut source, None, "Duplicate").await.is_err());
    assert_eq!(accounts(&f), before);
    no_journal(&f);
}

#[tokio::test]
async fn logged_out_missing_or_invalid_auth_never_creates_native_pending() {
    for kind in ["missing", "invalid", "logged-out"] {
        let f = fixture().await;
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let mut source = f.controlled(f.origin().unwrap());
        match kind {
            "missing" => std::fs::remove_file(f.f.auth()).unwrap(),
            "invalid" => std::fs::write(f.f.auth(), b"{invalid fixture").unwrap(),
            _ => std::fs::write(
                f.f.client.join("workbuddy-desktop.info.logged-out"),
                b"logout",
            )
            .unwrap(),
        }
        let before = accounts(&f);
        assert!(capture(&c, &mut source, None, "No session").await.is_err());
        assert_eq!(accounts(&f), before);
        no_journal(&f);
    }
}

#[tokio::test]
async fn actual_capture_dispatch_preserves_epoch_and_selector_before_worker_start() {
    for kind in ["epoch", "selector"] {
        let f = fixture().await;
        let session = f.f.session.clone();
        let selection = f.f.selection.clone();
        let control = f.f._root.path().join("control/app_paths.json");
        let before = accounts(&f);
        let (release, start) = std::sync::mpsc::channel::<()>();
        let (ack, wait_ack) = std::sync::mpsc::channel::<()>();
        let (started, wait_started) = tokio::sync::oneshot::channel();
        let (finished, wait_finished) = tokio::sync::oneshot::channel();
        let runtime = tokio::runtime::Handle::current();
        let task = crate::workbuddy::native_capture::schedule_capture(
            &session,
            || Ok(selection),
            move |identity, epoch, selection| {
                let _ = started.send(());
                if start.recv().is_err() {
                    return;
                }
                let result = runtime.block_on(async {
                    let controller = Controller {
                        catalog: Catalog {
                            session: &f.f.session,
                            identity: &identity,
                            epoch,
                            selection: &selection,
                        },
                    };
                    let mut source = f.controlled(f.origin().unwrap());
                    capture(&controller, &mut source, None, "Queued")
                        .await
                        .is_err()
                });
                let _ = finished.send(result);
                let _ = wait_ack.recv();
            },
        )
        .unwrap();
        wait_started.await.unwrap();
        if kind == "epoch" {
            session.invalidate_database_leases();
        } else {
            std::fs::write(&control, b"changed selector").unwrap();
        }
        release.send(()).unwrap();
        assert!(wait_finished.await.unwrap(), "accepted changed {kind}");
        assert_eq!(
            std::fs::read(session.root().join(store::FILE)).unwrap(),
            before
        );
        ack.send(()).unwrap();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn queued_worker_cannot_bind_to_a_new_epoch_after_scheduling() {
    let f = fixture().await;
    let binding = f.f.binding();
    let controller = f.f.controller(&binding);
    let (release, start) = tokio::sync::oneshot::channel::<()>();
    // Bind the original session and selector before the worker starts.
    let mut operation = Box::pin(async {
        start.await.unwrap();
        let mut source = f.controlled(f.origin().unwrap());
        capture(&controller, &mut source, None, "Queued").await
    });
    assert!(futures::poll!(&mut operation).is_pending());
    f.f.session.invalidate_database_leases();
    let before = accounts(&f);
    release.send(()).unwrap();
    assert!(operation.await.is_err(), "worker accepted a later epoch");
    assert_eq!(accounts(&f), before);
}

#[tokio::test]
async fn cancelled_waiter_query_reports_unknown_before_detached_capture_commits() {
    let f = fixture().await;
    let binding = f.f.binding();
    let session = f.f.session.clone();
    let selection = f.f.selection.clone();
    let epoch = session.lifecycle_epoch();
    let controller = Controller {
        catalog: Catalog {
            session: &session,
            identity: &binding,
            epoch,
            selection: &selection,
        },
    };
    let (release, start) = std::sync::mpsc::channel::<()>();
    let (ack, wait_ack) = std::sync::mpsc::channel::<()>();
    let (started, wait_started) = tokio::sync::oneshot::channel();
    let (finished, wait_finished) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let worker = tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        if start.recv().is_err() {
            return;
        }
        let result = runtime.block_on(async {
            let binding = f.f.binding();
            let controller = f.f.controller(&binding);
            let mut source = f.controlled(f.origin().unwrap());
            capture(&controller, &mut source, None, "Queued")
                .await
                .is_ok()
        });
        let _ = finished.send(result);
        let _ = wait_ack.recv();
    });
    wait_started.await.unwrap();
    drop(worker); // The blocked worker remains alive, as in the command.
    let before = controller.catalog.list_captures().await.unwrap();
    let wire = serde_json::to_value(crate::workbuddy::native_capture::CurrentCaptures::unknown(
        before,
    ))
    .unwrap();
    assert_eq!(wire["requestOutcome"], "unknown");
    assert_eq!(wire["activationAvailable"], false);
    assert!(wire["accounts"].as_array().unwrap().is_empty());
    release.send(()).unwrap();
    assert!(wait_finished.await.unwrap());
    assert_eq!(controller.catalog.list_captures().await.unwrap().len(), 1);
    ack.send(()).unwrap();
}

#[tokio::test]
async fn saved_capture_query_uses_only_original_catalog_and_ignores_native_auth_damage() {
    let f = fixture().await;
    let binding = f.f.binding();
    let c = f.f.controller(&binding);
    let mut source = f.controlled(f.origin().unwrap());
    let receipt = capture(&c, &mut source, None, "Saved A").await.unwrap();
    std::fs::write(f.f.auth(), b"{broken native file after a lost response").unwrap();
    let before = accounts(&f);
    let list = c.catalog.list_captures().await.unwrap();
    let wire = serde_json::to_value(&list).unwrap();
    assert_eq!(wire.as_array().unwrap().len(), 1);
    assert_eq!(
        wire[0]["selection"],
        serde_json::to_value(receipt.selection_key().unwrap()).unwrap()
    );
    assert_eq!(wire[0]["label"], "Saved A");
    assert_eq!(wire[0]["realm"], "www.workbuddy.cn");
    assert_eq!(wire[0].as_object().unwrap().len(), 3);
    assert_eq!(accounts(&f), before);
    no_journal(&f);
}

#[tokio::test]
async fn capture_query_rechecks_epoch_and_selector_after_waiting_for_original_owner() {
    for kind in ["epoch", "selector"] {
        let f = fixture().await;
        let binding = f.f.binding();
        let c = f.f.controller(&binding);
        let held = crate::services::sync_protocol::sync_mutex().lock().await;
        let mut query = Box::pin(c.catalog.list_captures());
        assert!(futures::poll!(&mut query).is_pending());
        if kind == "epoch" {
            f.f.session.invalidate_database_leases();
        } else {
            std::fs::write(
                f.f._root.path().join("control/app_paths.json"),
                b"changed selector",
            )
            .unwrap();
        }
        let before = accounts(&f);
        drop(held);
        assert!(query.await.is_err());
        assert_eq!(accounts(&f), before);
        no_journal(&f);
    }
}

#[test]
fn capture_input_cannot_supply_provenance_or_incomplete_row_lifetimes() {
    use crate::workbuddy::native_capture::Input;
    let key = json!({"id":"synthetic-id","generation":0,"incarnation":"00000000-0000-4000-8000-000000000001"});
    assert!(serde_json::from_value::<Input>(json!({"label":"A","expected":key})).is_ok());
    assert!(serde_json::from_value::<Input>(json!({"label":"A","expected":null})).is_ok());
    for field in ["path", "source", "realm", "uid", "snapshot", "admission"] {
        let mut wire = json!({"label":"A","expected":null});
        wire[field] = json!("caller data");
        assert!(serde_json::from_value::<Input>(wire).is_err());
    }
    assert!(serde_json::from_value::<Input>(
        json!({"label":"A","expected":{"id":"old","generation":0}})
    )
    .is_err());
}

#[test]
fn capture_failures_have_only_fixed_nonsecret_error_codes() {
    for (error, wire) in [
        (Failure::UnsafePath, "unsafePath"),
        (Failure::Io, "io"),
        (Failure::Journal, "journal"),
        (Failure::SourceChanged, "sourceChanged"),
        (Failure::RecoveryRequired, "recoveryRequired"),
        (Failure::RequestConflict, "requestConflict"),
        (Failure::RolledBack, "rolledBack"),
    ] {
        assert_eq!(serde_json::to_value(error).unwrap(), json!(wire));
    }
}
