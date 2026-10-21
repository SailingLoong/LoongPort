use super::*;
use crate::{proxy::hyper_client::ProxyResponse, secrets::VaultContext};
use bytes::Bytes;
use serde_json::json;
use std::{
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};
struct Fixture {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    _root: tempfile::TempDir,
    session: Arc<SecretSession>,
}
impl Fixture {
    async fn new() -> Self {
        let guard = store::fixture_guard().await;
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        Self {
            _guard: guard,
            _root: root,
            session,
        }
    }
    fn selected_path(&self, relative: &str) -> String {
        // The temporary fixture root supplies the platform's absolute prefix.
        // Begin records these synthetic paths only; it must not read them.
        let path = self._root.path().join(relative);
        assert!(path.is_absolute());
        path.to_str().unwrap().to_owned()
    }
    async fn begin(&self) -> FlowView {
        begin(
            &self.session,
            self.selected_path("Trae/User/globalStorage/storage.json"),
            self.selected_path("Trae/resources/app/product.json"),
            Realm::Sg,
        )
        .await
        .unwrap()
    }
}
fn material() -> import::Material {
    let mut m = import::synthetic_material();
    m.account.realm = Realm::Sg;
    m.account.device.auth_host = Realm::Sg.auth().into();
    m.account.device.version = "3.6.2".into();
    m
}
fn reply(value: serde_json::Value) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::OK,
        http::HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&value).unwrap()),
    )
}
#[derive(Default)]
struct Wire {
    fault: Option<(PathBuf, super::super::tests::WriteFault)>,
    calls: Mutex<Vec<String>>,
    pause: &'static str,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Transport for Wire {
    fn send(
        &self,
        r: inference::Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.url.clone());
            if !self.pause.is_empty() && r.url.ends_with(self.pause) {
                self.reached.notify_one();
                self.release.notified().await;
            }
            if r.url.ends_with("/ExchangeToken") {
                Ok(reply(
                    json!({"Result":{"Token":"synthetic-entry-access","RefreshToken":"synthetic-entry-rotated","TokenExpireAt":4102444800000_i64,"BoundDeviceID":"1234567890123456","DeviceBindStatus":"BOUND"}}),
                ))
            } else {
                assert!(r.url.ends_with("/GetUserInfo"));
                if let Some((path, fault)) = &self.fault {
                    super::super::tests::arm_write_fault(path.clone(), *fault);
                }
                Ok(reply(
                    json!({"Result":{"UserID":"synthetic-user","AIRegion":"SG"}}),
                ))
            }
        })
    }
}
#[tokio::test]
async fn explicit_confirmation_is_only_read_and_activation_entry() {
    let f = Fixture::new().await;
    let before = store::list(&f.session).await.unwrap();
    let flow = f.begin().await;
    assert_eq!(store::list(&f.session).await.unwrap(), before);
    let dto = serde_json::to_string(&flow).unwrap();
    for secret in ["synthetic-user", "storage.json", "token", "private"] {
        assert!(!dto.contains(secret));
    }
    let reads = AtomicUsize::new(0);
    let wire = Wire::default();
    let saved = confirm_with(f.session.clone(), &flow.flow_id, &wire, |_| async {
        reads.fetch_add(1, Ordering::SeqCst);
        Ok(material())
    })
    .await
    .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(wire.calls.lock().unwrap().len(), 2);
    assert_eq!(saved.status, store::AccountStatus::Stored);
    let safe = serde_json::to_string(&saved).unwrap();
    assert!(!safe.contains("synthetic-user"));
    assert!(!safe.contains("access"));
}
#[tokio::test]
async fn cancel_before_confirmation_reads_nothing_and_sends_nothing() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    cancel(&f.session, &flow.flow_id).await.unwrap();
    let reads = AtomicUsize::new(0);
    let wire = Wire::default();
    assert!(
        confirm_with(f.session.clone(), &flow.flow_id, &wire, |_| async {
            reads.fetch_add(1, Ordering::SeqCst);
            Ok(material())
        })
        .await
        .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(wire.calls.lock().unwrap().is_empty());
    assert!(list(&f.session).await.unwrap().is_empty());
}
async fn cancelled_during_network(pause: &'static str, expected_calls: usize) {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let wire = Arc::new(Wire {
        pause,
        ..Default::default()
    });
    let task = {
        let session = f.session.clone();
        let id = flow.flow_id.clone();
        let wire = wire.clone();
        tokio::spawn(async move {
            confirm_with(session, &id, wire.as_ref(), |_| async { Ok(material()) }).await
        })
    };
    wire.reached.notified().await;
    cancel(&f.session, &flow.flow_id).await.unwrap();
    wire.release.notify_one();
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        wire.calls.lock().unwrap().len(),
        expected_calls,
        "cancel must prevent later network admission"
    );
    let views = list(&f.session).await.unwrap();
    assert!(
        views
            .iter()
            .all(|v| v.status == store::AccountStatus::Retired),
        "cancelled attempt must not leave an enabled grant"
    );
    for row in views {
        assert!(store::snapshot(&f.session, &row.id).await.is_err());
    }
}
#[tokio::test]
async fn cancel_during_exchange_prevents_profile_and_late_publication() {
    cancelled_during_network("/ExchangeToken", 1).await
}
#[tokio::test]
async fn cancel_during_profile_prevents_late_publication() {
    cancelled_during_network("/GetUserInfo", 2).await
}
#[tokio::test]
async fn cancel_after_saved_revokes_only_that_generation() {
    let f = Fixture::new().await;
    let first = f.begin().await;
    let wire = Wire::default();
    let saved = confirm_with(f.session.clone(), &first.flow_id, &wire, |_| async {
        Ok(material())
    })
    .await
    .unwrap();
    cancel(&f.session, &first.flow_id).await.unwrap();
    assert!(store::snapshot(&f.session, &saved.id).await.is_err());
    assert_eq!(
        list(&f.session).await.unwrap()[0].status,
        store::AccountStatus::Retired
    );
}
#[tokio::test]
async fn repeated_confirm_does_not_read_or_exchange_twice() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let wire = Arc::new(Wire {
        pause: "/ExchangeToken",
        ..Default::default()
    });
    let task = {
        let session = f.session.clone();
        let id = flow.flow_id.clone();
        let wire = wire.clone();
        tokio::spawn(async move {
            confirm_with(session, &id, wire.as_ref(), |_| async { Ok(material()) }).await
        })
    };
    wire.reached.notified().await;
    let reads = AtomicUsize::new(0);
    assert!(
        confirm_with(f.session.clone(), &flow.flow_id, wire.as_ref(), |_| async {
            reads.fetch_add(1, Ordering::SeqCst);
            Ok(material())
        })
        .await
        .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    wire.release.notify_one();
    assert!(task.await.unwrap().is_ok());
    assert_eq!(wire.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn cancelled_selected_read_cannot_start_authorization() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let wire = Arc::new(Wire::default());
    let (started, reading) = tokio::sync::oneshot::channel();
    let (release, ready) = tokio::sync::oneshot::channel();
    let task = {
        let session = f.session.clone();
        let id = flow.flow_id.clone();
        let wire = wire.clone();
        tokio::spawn(async move {
            confirm_with(session, &id, wire.as_ref(), |_| async move {
                started.send(()).unwrap();
                ready.await.unwrap();
                Ok(material())
            })
            .await
        })
    };
    reading.await.unwrap();
    cancel(&f.session, &flow.flow_id).await.unwrap();
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(wire.calls.lock().unwrap().is_empty());
    assert!(list(&f.session).await.unwrap().is_empty());
}
#[tokio::test]
async fn relock_during_selected_read_cannot_publish_into_new_epoch() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let wire = Arc::new(Wire::default());
    let (started, reading) = tokio::sync::oneshot::channel();
    let (release, ready) = tokio::sync::oneshot::channel();
    let task = {
        let session = f.session.clone();
        let id = flow.flow_id.clone();
        let wire = wire.clone();
        tokio::spawn(async move {
            confirm_with(session, &id, wire.as_ref(), |_| async move {
                started.send(()).unwrap();
                ready.await.unwrap();
                Ok(material())
            })
            .await
        })
    };
    reading.await.unwrap();
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    release.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert!(wire.calls.lock().unwrap().is_empty());
    assert!(list(&f.session).await.unwrap().is_empty());
}
#[tokio::test]
async fn selected_host_and_realm_mismatch_never_send_credentials() {
    for wrong_host in [true, false] {
        let f = Fixture::new().await;
        let flow = f.begin().await;
        let wire = Wire::default();
        let mut m = material();
        if wrong_host {
            m.account.device.auth_host = "https://api-sg-central.trae.ai".into()
        } else {
            m.account.realm = Realm::Us
        }
        assert!(
            confirm_with(f.session.clone(), &flow.flow_id, &wire, |_| async { Ok(m) })
                .await
                .is_err()
        );
        assert!(wire.calls.lock().unwrap().is_empty());
        assert!(list(&f.session).await.unwrap().is_empty());
    }
}
#[tokio::test]
async fn cancelling_older_flow_cannot_retire_newer_same_account_login() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new().await;
    let first = f.begin().await;
    let wire = Wire::default();
    let first_saved = confirm_with(f.session.clone(), &first.flow_id, &wire, |_| async {
        Ok(material())
    })
    .await
    .unwrap();
    let second = f.begin().await;
    let mut new = material();
    new.account.grant.as_mut().unwrap().refresh = "synthetic-new-official-login-refresh".into();
    new.account.imported_refresh_hash =
        hex::encode(Sha256::digest(b"synthetic-new-official-login-refresh"));
    let second_saved = confirm_with(f.session.clone(), &second.flow_id, &wire, |_| async {
        Ok(new)
    })
    .await
    .unwrap();
    assert_eq!(first_saved.id, second_saved.id);
    let generation = store::snapshot(&f.session, &second_saved.id)
        .await
        .unwrap()
        .account
        .generation;
    cancel(&f.session, &first.flow_id).await.unwrap();
    assert_eq!(
        store::snapshot(&f.session, &second_saved.id)
            .await
            .unwrap()
            .account
            .generation,
        generation
    );
    cancel(&f.session, &second.flow_id).await.unwrap();
    assert!(store::snapshot(&f.session, &second_saved.id).await.is_err());
}
#[tokio::test]
async fn unknown_authorization_publication_can_be_cancelled_without_replay() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let wire = Wire {
        fault: Some((
            f.session.root().join("trae_accounts.json"),
            super::super::tests::WriteFault::PostRenameSync,
        )),
        ..Default::default()
    };
    assert_eq!(
        confirm_with(f.session.clone(), &flow.flow_id, &wire, |_| async {
            Ok(material())
        })
        .await
        .unwrap_err(),
        Error::PublicationUnknown
    );
    assert_eq!(
        list(&f.session).await.unwrap()[0].status,
        store::AccountStatus::Stored
    );
    cancel(&f.session, &flow.flow_id).await.unwrap();
    assert_eq!(
        list(&f.session).await.unwrap()[0].status,
        store::AccountStatus::Retired
    );
    assert_eq!(wire.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn uncertain_cancellation_durability_is_not_reported_as_success() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let wire = Wire::default();
    let saved = confirm_with(f.session.clone(), &flow.flow_id, &wire, |_| async {
        Ok(material())
    })
    .await
    .unwrap();
    super::super::tests::arm_write_fault(
        f.session.root().join("trae_accounts.json"),
        super::super::tests::WriteFault::PostRenameSync,
    );
    assert_eq!(
        cancel(&f.session, &flow.flow_id).await.unwrap_err(),
        Error::PublicationUnknown
    );
    assert!(store::snapshot(&f.session, &saved.id).await.is_err());
    assert!(
        confirm_with(f.session.clone(), &flow.flow_id, &wire, |_| async {
            Ok(material())
        })
        .await
        .is_err()
    );
    assert_eq!(wire.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn foreign_vault_or_unsupported_selection_has_no_read_entry() {
    let f = Fixture::new().await;
    let flow = f.begin().await;
    let other = tempfile::tempdir().unwrap();
    let other_session =
        SecretSession::from_context(other.path().into(), VaultContext::generate().unwrap());
    let reads = AtomicUsize::new(0);
    let wire = Wire::default();
    assert!(
        confirm_with(other_session, &flow.flow_id, &wire, |_| async {
            reads.fetch_add(1, Ordering::SeqCst);
            Ok(material())
        })
        .await
        .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(begin(
        &f.session,
        f.selected_path("storage.json"),
        f.selected_path("product.json"),
        Realm::Cn
    )
    .await
    .is_err());
    assert!(begin(
        &f.session,
        "relative/storage.json".into(),
        f.selected_path("product.json"),
        Realm::Sg
    )
    .await
    .is_err());
    assert!(wire.calls.lock().unwrap().is_empty());
}
