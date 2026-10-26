//! Synthetic integration bridge; no real account, endpoint or root input.
use super::{catalog::*, http::*};
use crate::secrets::{owned_file::OwnedFile, session::SecretSession, VaultContext};
use serde_json::json;
use std::{future::Future, pin::Pin, sync::Arc};
struct Fixture {
    root: tempfile::TempDir,
    session: Arc<SecretSession>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        Self { root, session }
    }
    fn seed(&self, mutate: impl FnOnce(&mut Document)) {
        let mut doc = Document {
            schema_version: 1,
            revision: 1,
            accounts: vec![account()],
        };
        mutate(&mut doc);
        let vault = self.session.read().unwrap();
        let file = OwnedFile::registered(FILE).unwrap();
        let bytes = file
            .encode_storage(&vault, &serde_json::to_vec(&doc).unwrap())
            .unwrap();
        crate::config_file_io::write_durable(&self.root.path().join(FILE), &bytes).unwrap();
    }
}
fn account() -> Account {
    Account {
        auto_claim_enabled: false,
        auto_claim_revision: 0,
        auto_claim_failure: None,
        id: "synthetic-account".into(),
        label: "Synthetic".into(),
        region: Region::Cn,
        build_env: "prod".into(),
        real_user_id: "synthetic-user".into(),
        auth_generation: 1,
        credential_version: 1,
        credentials: Some(Credentials {
            access_token: "synthetic-access".into(),
            refresh_token: Some("synthetic-refresh".into()),
            kind: CredentialKind::DeviceOAuth,
            native_source: None,
            expires_at: i64::MAX,
        }),
        status: Status::Claimable,
        operation: None,
        last_checked_at: None,
        last_observed_status: Some(Status::Claimable),
    }
}

use crate::minimax_code::{auth, catalog};
use std::sync::atomic::{AtomicUsize, Ordering};

struct HeldRefresh {
    calls: AtomicUsize,
    fail: bool,
    release: tokio::sync::Semaphore,
}
impl HeldRefresh {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail: false,
            release: tokio::sync::Semaphore::new(0),
        }
    }
}
impl Transport for HeldRefresh {
    fn request(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            match r.endpoint {
                Endpoint::Refresh => {
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    self.release.acquire().await.unwrap().forget();
                    if self.fail {
                        return Err(Failure::NetworkUnavailable);
                    }
                    Ok(Response {
                        status: 200,
                        body: json!({"access_token":"synthetic-rotated", "refresh_token":"synthetic-rotated-refresh", "token_type":"Bearer", "expires_in":3600, "scope":"agent.default"}),
                    })
                }
                Endpoint::Profile => Ok(Response {
                    status: 200,
                    body: json!({"data":{"userInfo":{"realUserID":"synthetic-user","name":"Synthetic"}}}),
                }),
                _ => panic!("unexpected refresh-owner transport endpoint"),
            }
        })
    }
}

async fn same_version_refresh_is_single_flight_and_waiter_reuses_published_credentials() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let snapshot = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let transport = HeldRefresh::new();
    let first = auth::refresh_snapshot(&f.session, &transport, &snapshot);
    let second = auth::refresh_snapshot(&f.session, &transport, &snapshot);
    tokio::pin!(first, second);
    assert!(futures::poll!(&mut first).is_pending());
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(futures::poll!(&mut second).is_pending());
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        1,
        "same credential version must not submit a second refresh while the first is pending"
    );
    transport.release.add_permits(2);
    let (a, b) = tokio::join!(first, second);
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.account.credential_version, 2);
    assert_eq!(b.account.credential_version, 2);
    assert_eq!(
        a.account.credentials.unwrap().access_token,
        b.account.credentials.unwrap().access_token
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

async fn different_accounts_refresh_without_global_network_serialization() {
    let f = Fixture::new().await;
    f.seed(|d| {
        let mut second = account();
        second.id = "synthetic-second".into();
        second.real_user_id = "synthetic-user-second".into();
        d.accounts.push(second);
    });
    let a = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let b = catalog::snapshot(&f.session, "synthetic-second")
        .await
        .unwrap();
    let transport = HeldRefresh::new();
    let mut first = Box::pin(auth::refresh_snapshot(&f.session, &transport, &a));
    let mut second = Box::pin(auth::refresh_snapshot(&f.session, &transport, &b));
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    // No response has been received; neither result is published.
    drop((first, second));
    assert_eq!(
        catalog::snapshot(&f.session, "synthetic-account")
            .await
            .unwrap()
            .account
            .credential_version,
        1
    );
}

async fn cancelled_leader_returns_unknown_to_existing_waiter_without_replay() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let snapshot = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let transport = HeldRefresh::new();
    let mut first = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    assert!(futures::poll!(&mut first).is_pending());
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    let mut next = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    assert!(futures::poll!(&mut next).is_pending());
    drop(first);
    let result = futures::poll!(&mut next);
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        1,
        "a submitted refresh with no known result must not be replayed"
    );
    assert!(matches!(result, std::task::Poll::Ready(Err(_))));
}

async fn authorization_replaced_while_refresh_pending_cannot_publish_old_result() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let snapshot = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let transport = HeldRefresh::new();
    let mut first = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    assert!(futures::poll!(&mut first).is_pending());
    f.seed(|d| {
        d.accounts[0].auth_generation += 1;
    });
    transport.release.add_permits(1);
    assert!(matches!(first.await, Err(Failure::StaleIdentity)));
    let current = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    assert_eq!(current.account.auth_generation, 2);
    assert_eq!(current.account.credential_version, 1);
    assert_eq!(
        current.account.credentials.unwrap().access_token,
        "synthetic-access"
    );
}

async fn failed_refresh_result_is_shared_without_a_waiter_retry() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let snapshot = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let mut transport = HeldRefresh::new();
    transport.fail = true;
    let mut first = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    let mut second = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    assert!(futures::poll!(&mut first).is_pending());
    assert!(futures::poll!(&mut second).is_pending());
    transport.release.add_permits(2);
    let (a, b) = tokio::join!(first, second);
    assert!(matches!(a, Err(Failure::NetworkUnavailable)));
    assert!(matches!(b, Err(Failure::NetworkUnavailable)));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        catalog::snapshot(&f.session, "synthetic-account")
            .await
            .unwrap()
            .account
            .credential_version,
        1
    );
}

struct NoInference;
impl super::inference::StreamTransport for NoInference {
    fn request(
        &self,
        _: super::inference::StreamRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        crate::proxy::hyper_client::ProxyResponse,
                        crate::proxy::ProxyError,
                    >,
                > + Send
                + '_,
        >,
    > {
        panic!("failed refresh must not dispatch inference")
    }
}
async fn refresh_network_failure_is_not_reported_as_account_authentication_failure() {
    use axum::response::IntoResponse;
    let f = Fixture::new().await;
    f.seed(|d| d.accounts[0].credentials.as_mut().unwrap().expires_at = now_ms());
    let mut transport = HeldRefresh::new();
    transport.fail = true;
    transport.release.add_permits(1);
    let lease = super::inference::Lease::acquire(
        f.session.clone(),
        "synthetic-account",
        "synthetic-request",
    )
    .await
    .unwrap();
    let result = lease
        .dispatch_with(
            json!({"model":"synthetic-model","messages":[],"stream":true}),
            std::time::Duration::from_secs(1),
            &transport,
            &NoInference,
        )
        .await;
    let error = match result {
        Ok(_) => panic!("refresh failed but inference succeeded"),
        Err(e) => e,
    };
    assert!(
        matches!(error, crate::proxy::ProxyError::ForwardFailed(_)),
        "network/unknown refresh result must not become AuthError"
    );
    assert_eq!(
        error.into_response().status(),
        http::StatusCode::BAD_GATEWAY
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

async fn successful_flight_waiter_rechecks_authorization_before_returning_credentials() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let snapshot = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let transport = HeldRefresh::new();
    let mut leader = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    let mut waiter = Box::pin(auth::refresh_snapshot(&f.session, &transport, &snapshot));
    assert!(futures::poll!(&mut leader).is_pending());
    assert!(futures::poll!(&mut waiter).is_pending());
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    transport.release.add_permits(1);
    assert_eq!(leader.await.unwrap().account.credential_version, 2);
    // The waiter has seen no completion yet. Replace the authorization through
    // the real encrypted owner before it may return the completed result.
    catalog::access(&f.session, Some(&snapshot.binding), |document, _| {
        document.accounts[0].auth_generation += 1;
        Ok(((), true))
    })
    .await
    .unwrap();
    assert!(matches!(waiter.await, Err(Failure::StaleIdentity)));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

async fn late_old_snapshot_after_flight_exit_keeps_original_stale_guard_without_network() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let snapshot = catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let transport = HeldRefresh::new();
    transport.release.add_permits(1);
    assert_eq!(
        auth::refresh_snapshot(&f.session, &transport, &snapshot)
            .await
            .unwrap()
            .account
            .credential_version,
        2
    );
    assert!(matches!(
        auth::refresh_snapshot(&f.session, &transport, &snapshot).await,
        Err(Failure::StaleIdentity)
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
}

pub(crate) async fn verify(case: &str) {
    // Separate integration cases share production catalog admission. Serialize
    // only these synthetic fixtures, not the concurrent calls inside each case.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _serial = SERIAL.lock().await;
    match case {
        "late_old_snapshot_after_flight_exit_keeps_original_stale_guard_without_network" => {
            late_old_snapshot_after_flight_exit_keeps_original_stale_guard_without_network().await
        }
        "successful_flight_waiter_rechecks_authorization_before_returning_credentials" => {
            successful_flight_waiter_rechecks_authorization_before_returning_credentials().await
        }
        "refresh_network_failure_is_not_reported_as_account_authentication_failure" => {
            refresh_network_failure_is_not_reported_as_account_authentication_failure().await
        }
        "same_version_refresh_is_single_flight_and_waiter_reuses_published_credentials" => {
            same_version_refresh_is_single_flight_and_waiter_reuses_published_credentials().await
        }
        "different_accounts_refresh_without_global_network_serialization" => {
            different_accounts_refresh_without_global_network_serialization().await
        }
        "cancelled_leader_returns_unknown_to_existing_waiter_without_replay" => {
            cancelled_leader_returns_unknown_to_existing_waiter_without_replay().await
        }
        "authorization_replaced_while_refresh_pending_cannot_publish_old_result" => {
            authorization_replaced_while_refresh_pending_cannot_publish_old_result().await
        }
        "failed_refresh_result_is_shared_without_a_waiter_retry" => {
            failed_refresh_result_is_shared_without_a_waiter_retry().await
        }
        _ => panic!("unknown synthetic refresh-owner case"),
    }
}
