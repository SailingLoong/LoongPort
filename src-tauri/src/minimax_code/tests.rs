use super::{catalog::*, checkin::Engine, http::*};
use crate::secrets::{owned_file::OwnedFile, session::SecretSession, VaultContext};
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
struct Fixture {
    _serial: tokio::sync::OwnedMutexGuard<()>,
    root: tempfile::TempDir,
    session: Arc<SecretSession>,
}
impl Fixture {
    async fn new() -> Self {
        // Tests that call process shutdown share its actual process-wide epoch.
        // Serialize synthetic sessions without changing production fencing.
        let serial = super::isolated_fixture_guard().await;
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        Self {
            _serial: serial,
            root,
            session,
        }
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
            .encode(&vault, &serde_json::to_vec(&doc).unwrap())
            .unwrap();
        crate::config_file_io::write_durable(&self.root.path().join(FILE), &bytes).unwrap();
    }
    fn raw(&self) -> Vec<u8> {
        std::fs::read(self.root.path().join(FILE)).unwrap()
    }
}
fn account() -> Account {
    Account {
        id: "synthetic-account".into(),
        label: "Synthetic".into(),
        region: Region::Cn,
        build_env: "prod".into(),
        real_user_id: "synthetic-user".into(),
        auth_generation: 1,
        credential_version: 1,
        credentials: Some(Credentials {
            access_token: "synthetic-access".into(),
            refresh_token: "synthetic-refresh".into(),
            expires_at: i64::MAX,
        }),
        status: Status::Claimable,
        operation: None,
        last_checked_at: None,
        last_observed_status: Some(Status::Claimable),
    }
}
fn panel(status: u64) -> Value {
    json!({"scene":2,"days":(1..=7).map(|n|json!({"day_no":n,"points":10,"status":if n==1{status}else{1},"is_today":n==1})).collect::<Vec<_>>()})
}
fn response(data: Value) -> Response {
    Response {
        status: 200,
        body: json!({"base_resp":{"status_code":0},"data":data}),
    }
}
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Endpoint>>,
    claim: Mutex<Option<Result<Response, Failure>>>,
}
impl Transport for Fake {
    fn request(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.endpoint);
            if r.endpoint == Endpoint::Claim {
                self.claim.lock().unwrap().take().unwrap_or_else(||Ok(response(json!({"claim_id":"synthetic-claim","claim_result":1,"day_no":1,"points":10,"expire_at_ms":1900000000000_i64,"panel":panel(3)}))))
            } else {
                Ok(response(panel(2)))
            }
        })
    }
}
#[test]
fn registered_encrypted_only_file_is_in_lifecycle_registry() {
    let file = OwnedFile::registered(FILE).expect("MiniMax Code file must join vault lifecycle");
    assert!(!file.allows_legacy_plaintext());
}
#[tokio::test]
async fn unknown_schema_is_rejected_without_empty_catalog_overwrite() {
    let f = Fixture::new().await;
    f.seed(|d| d.schema_version = 99);
    let before = f.raw();
    let t = Fake::default();
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    assert_eq!(
        engine.list().await.unwrap_err(),
        Failure::StorageUnavailable
    );
    assert_eq!(before, f.raw());
}
#[tokio::test]
async fn unauthenticated_or_token_plan_never_posts() {
    let f = Fixture::new().await;
    f.seed(|d| {
        d.accounts[0].credentials = None;
        d.accounts[0].status = Status::NeedsLogin
    });
    let t = Fake::default();
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    assert_eq!(
        engine.claim("synthetic-account").await.unwrap_err(),
        Failure::NeedsLogin
    );
    assert!(t.calls.lock().unwrap().is_empty());
    assert!(engine.claim("token-plan-api-key").await.is_err());
    assert!(t.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn successful_claim_is_encrypted_and_cannot_be_double_submitted() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = Fake::default();
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    let first = engine.claim("synthetic-account").await.unwrap();
    assert_eq!(first.status, Status::Claimed);
    let _ = engine.claim("synthetic-account").await;
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
    assert!(!String::from_utf8_lossy(&f.raw()).contains("synthetic-access"));
}
#[tokio::test]
async fn timeout_persists_unknown_across_restart_and_recheck_is_get_only() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = Fake {
        claim: Mutex::new(Some(Err(Failure::NetworkUnavailable))),
        ..Fake::default()
    };
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    let first = engine.claim("synthetic-account").await.unwrap();
    assert_eq!(first.status, Status::Unknown);
    let resumed = Engine {
        session: &f.session,
        transport: &t,
    };
    assert_eq!(resumed.list().await.unwrap()[0].status, Status::Unknown);
    assert_eq!(
        resumed.recheck("synthetic-account").await.unwrap().status,
        Status::Unknown
    );
    let _ = resumed.claim("synthetic-account").await;
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
}
#[tokio::test]
async fn malformed_200_keeps_unknown_and_retire_readd_cannot_clear_attempt() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = Fake {
        claim: Mutex::new(Some(Ok(response(json!({}))))),
        ..Fake::default()
    };
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    assert_eq!(
        engine.claim("synthetic-account").await.unwrap().status,
        Status::Unknown
    );
    engine.retire("synthetic-account").await.unwrap();
    let view = engine.list().await.unwrap().remove(0);
    assert!(!view.authorized);
    assert!(view.operation.is_some());
    assert!(!view.can_claim);
}
#[tokio::test]
async fn vault_block_refuses_read_and_post() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    f.session.set_blocked(true);
    let t = Fake::default();
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    assert_eq!(
        engine.list().await.unwrap_err(),
        Failure::StorageUnavailable
    );
    assert!(engine.claim("synthetic-account").await.is_err());
    assert!(t.calls.lock().unwrap().is_empty());
}

#[derive(Default)]
struct Gate {
    calls: Mutex<Vec<Endpoint>>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Transport for Gate {
    fn request(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.endpoint);
            if r.endpoint == Endpoint::Claim {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(response(
                    json!({"claim_id":"synthetic-claim","claim_result":1,"day_no":1,"points":10,"expire_at_ms":1900000000000_i64,"panel":panel(3)}),
                ))
            } else {
                Ok(response(panel(2)))
            }
        })
    }
}
#[tokio::test]
async fn actual_concurrent_double_click_dispatches_one_post() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let gate = Arc::new(Gate::default());
    let session = f.session.clone();
    let remote = gate.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    gate.entered.notified().await;
    let result = Engine {
        session: &f.session,
        transport: gate.as_ref(),
    }
    .claim("synthetic-account")
    .await;
    assert_eq!(result.unwrap_err(), Failure::Busy);
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap().status, Status::Claimed);
    assert_eq!(
        gate.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
}
#[tokio::test]
async fn blocked_then_unblocked_never_revives_an_inflight_post_lease() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let gate = Arc::new(Gate::default());
    let session = f.session.clone();
    let remote = gate.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    gate.entered.notified().await;
    let before = f.raw();
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap_err(), Failure::StaleIdentity);
    assert_eq!(before, f.raw());
    assert_eq!(
        Engine {
            session: &f.session,
            transport: gate.as_ref()
        }
        .list()
        .await
        .unwrap()[0]
            .status,
        Status::Unknown
    );
}
#[tokio::test]
async fn retire_during_claim_preserves_unknown_and_rejects_late_receipt() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let gate = Arc::new(Gate::default());
    let session = f.session.clone();
    let remote = gate.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    gate.entered.notified().await;
    let engine = Engine {
        session: &f.session,
        transport: gate.as_ref(),
    };
    engine.retire("synthetic-account").await.unwrap();
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap_err(), Failure::StaleIdentity);
    let a = engine.list().await.unwrap().remove(0);
    assert_eq!(a.status, Status::Unknown);
    assert!(!a.authorized);
    assert_eq!(a.auth_generation, 2);
    assert_eq!(a.operation.unwrap().status, AttemptStatus::Pending);
}
#[tokio::test]
async fn aborted_claim_keeps_marker_and_new_session_never_replays() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let gate = Arc::new(Gate::default());
    let session = f.session.clone();
    let remote = gate.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    gate.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let new_session =
        SecretSession::from_context(f.root.path().into(), f.session.read().unwrap().clone());
    let t = Fake::default();
    let engine = Engine {
        session: &new_session,
        transport: &t,
    };
    assert_eq!(engine.list().await.unwrap()[0].status, Status::Unknown);
    assert_eq!(
        engine.claim("synthetic-account").await.unwrap().status,
        Status::Unknown
    );
    engine.recheck("synthetic-account").await.unwrap();
    assert_eq!(*t.calls.lock().unwrap(), vec![Endpoint::Status]);
}
#[tokio::test]
async fn previous_day_unknown_is_not_erased_by_today_claimable_panel() {
    let f = Fixture::new().await;
    f.seed(|d| {
        d.accounts[0].operation = Some(Operation {
            attempt_id: "old-attempt".into(),
            started_at: 1600000000000,
            day: "2020-09-13".into(),
            bound_auth_generation: 1,
            claim_id: None,
            status: AttemptStatus::Unknown,
            credit_amount: None,
        });
        d.accounts[0].status = Status::Unknown;
    });
    let t = Fake::default();
    let engine = Engine {
        session: &f.session,
        transport: &t,
    };
    let view = engine.recheck("synthetic-account").await.unwrap();
    assert_eq!(view.status, Status::Unknown);
    assert_eq!(view.last_observed_status, Some(Status::Claimable));
    assert_eq!(view.operation.unwrap().attempt_id, "old-attempt");
    engine.claim("synthetic-account").await.unwrap();
    assert_eq!(*t.calls.lock().unwrap(), vec![Endpoint::Status]);
}
struct BreakStorage {
    path: std::path::PathBuf,
    calls: Mutex<Vec<Endpoint>>,
}
impl Transport for BreakStorage {
    fn request(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.endpoint);
            if r.endpoint == Endpoint::Status {
                std::fs::rename(&self.path, self.path.with_extension("fixture-preserved")).unwrap();
                std::fs::create_dir(&self.path).unwrap();
            }
            Ok(response(panel(2)))
        })
    }
}
#[tokio::test]
async fn pending_storage_failure_sends_zero_posts() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = BreakStorage {
        path: f.root.path().join(FILE),
        calls: Mutex::new(vec![]),
    };
    let result = Engine {
        session: &f.session,
        transport: &t,
    }
    .claim("synthetic-account")
    .await;
    assert_eq!(result.unwrap_err(), Failure::StorageUnavailable);
    assert_eq!(*t.calls.lock().unwrap(), vec![Endpoint::Status]);
}
#[tokio::test]
async fn application_shutdown_invalidates_inflight_response_without_clearing_pending() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let gate = Arc::new(Gate::default());
    let session = f.session.clone();
    let remote = gate.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    gate.entered.notified().await;
    let before = f.raw();
    super::shutdown();
    gate.release.notify_one();
    assert_eq!(task.await.unwrap().unwrap_err(), Failure::StaleIdentity);
    assert_eq!(before, f.raw());
}
#[test]
fn strict_panel_rejects_missing_duplicate_or_out_of_range_fields() {
    for p in [
        json!({}),
        json!({"scene":2,"days":[]}),
        json!({"scene":2,"days":vec![json!({"day_no":1,"points":10,"status":2,"is_today":true});7]}),
    ] {
        assert_eq!(
            super::checkin::panel_status(&p),
            Err(Failure::InvalidResponse)
        );
    }
    assert_eq!(
        super::checkin::panel_status(&panel(2)),
        Ok(Status::Claimable)
    );
}

struct LoginHttp {
    calls: Mutex<Vec<Endpoint>>,
    uid: String,
    refresh_uid: Option<String>,
    status_401: Mutex<bool>,
    claim_401: bool,
}
impl LoginHttp {
    fn new(uid: &str) -> Self {
        Self {
            calls: Mutex::new(vec![]),
            uid: uid.into(),
            refresh_uid: None,
            status_401: Mutex::new(false),
            claim_401: false,
        }
    }
}
impl Transport for LoginHttp {
    fn request(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.endpoint);
            match r.endpoint {
                Endpoint::DeviceCode => {
                    let b = r.body.unwrap();
                    assert_eq!(b["client_id"], "mcode-public");
                    assert_eq!(b["scope"], "agent.default");
                    assert_eq!(b["code_challenge_method"], "S256");
                    assert_eq!(b["code_challenge"].as_str().unwrap().len(), 43);
                    Ok(Response {
                        status: 200,
                        body: json!({"device_code":"synthetic-device-code","user_code":"ABCD-EFGH","verification_uri":"https://account.minimax.cn/oauth2/device/authorize","expires_in":600,"interval":5}),
                    })
                }
                Endpoint::Token | Endpoint::Refresh => {
                    let b = r.body.unwrap();
                    assert_eq!(b["client_id"], "mcode-public");
                    if r.endpoint == Endpoint::Token {
                        assert_eq!(
                            b["grant_type"],
                            "urn:ietf:params:oauth:grant-type:device_code"
                        );
                        assert_eq!(b["device_code"], "synthetic-device-code");
                        assert_eq!(b["code_verifier"].as_str().unwrap().len(), 43);
                    } else {
                        assert_eq!(b["grant_type"], "refresh_token");
                    }
                    Ok(Response {
                        status: 200,
                        body: json!({"access_token":"synthetic-new-access","refresh_token":"synthetic-new-refresh","token_type":"Bearer","scope":"agent.default","expires_in":3600}),
                    })
                }
                Endpoint::Profile => {
                    let uid = if self.calls.lock().unwrap().contains(&Endpoint::Refresh) {
                        self.refresh_uid.as_deref().unwrap_or(&self.uid)
                    } else {
                        &self.uid
                    };
                    Ok(response(
                        json!({"userInfo":{"realUserID":uid,"name":"Synthetic User"}}),
                    ))
                }
                Endpoint::Status => {
                    let mut unauthorized = self.status_401.lock().unwrap();
                    if *unauthorized {
                        *unauthorized = false;
                        Ok(Response {
                            status: 401,
                            body: json!({}),
                        })
                    } else {
                        Ok(response(panel(2)))
                    }
                }
                Endpoint::Claim => {
                    if self.claim_401 {
                        Ok(Response {
                            status: 401,
                            body: json!({}),
                        })
                    } else {
                        Ok(response(
                            json!({"claim_id":"synthetic-claim","claim_result":1,"day_no":1,"points":10,"expire_at_ms":1900000000000_i64,"panel":panel(3)}),
                        ))
                    }
                }
            }
        })
    }
}
#[tokio::test]
async fn official_device_authorization_verifies_profile_before_encrypted_admission() {
    let f = Fixture::new().await;
    let t = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &t, Region::Cn)
        .await
        .unwrap();
    assert_eq!(login.user_code, "ABCD-EFGH");
    let result = super::auth::poll(&f.session, &t, &login.flow_id)
        .await
        .unwrap();
    assert_eq!(result.state, super::auth::AuthorizationState::Saved);
    let a = result.account.unwrap();
    assert_eq!(a.real_user_id, "synthetic-user");
    assert!(a.authorized);
    assert!(!String::from_utf8_lossy(&f.raw()).contains("synthetic-new-access"));
    assert_eq!(
        *t.calls.lock().unwrap(),
        vec![Endpoint::DeviceCode, Endpoint::Token, Endpoint::Profile]
    );
}
#[tokio::test]
async fn status_401_refreshes_same_verified_user_once_without_changing_generation() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = LoginHttp {
        status_401: Mutex::new(true),
        ..LoginHttp::new("synthetic-user")
    };
    let view = Engine {
        session: &f.session,
        transport: &t,
    }
    .refresh("synthetic-account")
    .await
    .unwrap();
    assert_eq!(view.auth_generation, 1);
    assert_eq!(view.credential_version, 2);
    assert_eq!(
        *t.calls.lock().unwrap(),
        vec![
            Endpoint::Status,
            Endpoint::Refresh,
            Endpoint::Profile,
            Endpoint::Status
        ]
    );
}
#[tokio::test]
async fn refresh_rejects_different_profile_id_without_post_or_overwrite() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let before = f.raw();
    let t = LoginHttp {
        status_401: Mutex::new(true),
        refresh_uid: Some("different-user".into()),
        ..LoginHttp::new("synthetic-user")
    };
    let result = Engine {
        session: &f.session,
        transport: &t,
    }
    .claim("synthetic-account")
    .await;
    assert_eq!(result.unwrap_err(), Failure::StaleIdentity);
    assert_eq!(before, f.raw());
    assert!(!t.calls.lock().unwrap().contains(&Endpoint::Claim));
}
#[tokio::test]
async fn claim_401_refreshes_but_never_replays_post() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = LoginHttp {
        claim_401: true,
        ..LoginHttp::new("synthetic-user")
    };
    let view = Engine {
        session: &f.session,
        transport: &t,
    }
    .claim("synthetic-account")
    .await
    .unwrap();
    assert_eq!(view.status, Status::Unknown);
    assert_eq!(view.credential_version, 2);
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Refresh)
            .count(),
        1
    );
}
#[tokio::test]
async fn canceled_device_flow_cannot_admit_a_token() {
    let f = Fixture::new().await;
    let t = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &t, Region::Cn)
        .await
        .unwrap();
    super::auth::cancel(&f.session, &login.flow_id)
        .await
        .unwrap();
    assert_eq!(
        super::auth::poll(&f.session, &t, &login.flow_id)
            .await
            .unwrap_err(),
        Failure::ExpiredAuthorization
    );
    assert_eq!(*t.calls.lock().unwrap(), vec![Endpoint::DeviceCode]);
}
#[tokio::test]
async fn reauthorization_same_identity_preserves_unknown_attempt_after_retirement() {
    let f = Fixture::new().await;
    let t = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &t, Region::Cn)
        .await
        .unwrap();
    let id = super::auth::poll(&f.session, &t, &login.flow_id)
        .await
        .unwrap()
        .account
        .unwrap()
        .id;
    let fail = Fake {
        claim: Mutex::new(Some(Err(Failure::NetworkUnavailable))),
        ..Fake::default()
    };
    let engine = Engine {
        session: &f.session,
        transport: &fail,
    };
    let before = engine
        .claim(&id)
        .await
        .unwrap()
        .operation
        .unwrap()
        .attempt_id;
    engine.retire(&id).await.unwrap();
    let login = super::auth::begin(&f.session, &t, Region::Cn)
        .await
        .unwrap();
    let view = super::auth::poll(&f.session, &t, &login.flow_id)
        .await
        .unwrap()
        .account
        .unwrap();
    assert_eq!(view.id, id);
    assert_eq!(view.operation.unwrap().attempt_id, before);
    assert_eq!(view.status, Status::Unknown);
    assert!(!view.can_claim);
}

#[test]
fn official_status_wire_is_identity_bound_and_profile_uses_distinct_query_shape() {
    let client = reqwest::Client::new();
    let s = super::http::build_request(
        &client,
        Request {
            endpoint: Endpoint::Status,
            region: Region::Cn,
            access_token: Some("synthetic-access".into()),
            real_user_id: Some("synthetic-user".into()),
            body: None,
        },
        1700000000123,
        28800,
        "linux",
        "1.2.3",
    )
    .unwrap();
    assert_eq!(s.method(), reqwest::Method::GET);
    assert_eq!(s.url().as_str(),"https://agent.minimaxi.com/minimax-cloud/api/v1/signin/status?device_platform=web&biz_id=3&app_id=3001&version_code=22201&is_desktop=1&desktop_version=1.2.3&unix=1700000000123&timezone_offset=28800&sys_language=zh&lang=zh&device_id=0&os_name=linux&browser_name=mcode&user_id=synthetic-user&client=mcode");
    assert_eq!(s.headers()["x-timestamp"], "1700000000");
    assert_eq!(s.headers()["authorization"], "Bearer synthetic-access");
    let p = super::http::build_request(
        &client,
        Request {
            endpoint: Endpoint::Profile,
            region: Region::Global,
            access_token: Some("synthetic-access".into()),
            real_user_id: None,
            body: None,
        },
        1700000000123,
        0,
        "win32",
        "1.2.3",
    )
    .unwrap();
    assert!(p
        .url()
        .as_str()
        .starts_with("https://agent.minimax.io/v1/api/user/info?device_platform=mcode&"));
    assert!(!p.url().as_str().contains("desktop_version"));
    assert!(p.url().as_str().contains("user_id=0"));
}
#[test]
fn official_oauth_wire_uses_account_origins_and_form_encoding() {
    let client = reqwest::Client::new();
    let r=super::http::build_request(&client,Request{endpoint:Endpoint::DeviceCode,region:Region::Cn,access_token:None,real_user_id:None,body:Some(json!({"client_id":"mcode-public","scope":"agent.default","audience":"agent-backend","code_challenge":"synthetic-challenge","code_challenge_method":"S256"}))},1700000000123,0,"linux","1.2.3").unwrap();
    assert_eq!(
        r.url().as_str(),
        "https://account.minimax.cn/oauth2/device/code"
    );
    assert_eq!(
        r.headers()["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert!(
        String::from_utf8_lossy(r.body().unwrap().as_bytes().unwrap())
            .contains("client_id=mcode-public")
    );
    assert!(r.headers().get("authorization").is_none());
}
#[test]
fn claim_wire_has_exact_empty_object_and_rejects_missing_verified_id() {
    let client = reqwest::Client::new();
    let r = super::http::build_request(
        &client,
        Request {
            endpoint: Endpoint::Claim,
            region: Region::Global,
            access_token: Some("synthetic-access".into()),
            real_user_id: Some("synthetic-user".into()),
            body: Some(json!({})),
        },
        1700000000123,
        0,
        "darwin",
        "1.2.3",
    )
    .unwrap();
    assert_eq!(r.method(), reqwest::Method::POST);
    assert_eq!(r.body().unwrap().as_bytes().unwrap(), b"{}");
    assert_ne!(r.headers()["x-signature"], "unverified");
    let bad = super::http::build_request(
        &client,
        Request {
            endpoint: Endpoint::Claim,
            region: Region::Cn,
            access_token: Some("api-key".into()),
            real_user_id: None,
            body: Some(json!({})),
        },
        1700000000123,
        0,
        "linux",
        "1.2.3",
    );
    assert_eq!(bad.unwrap_err(), Failure::NeedsLogin);
}

#[tokio::test]
async fn pending_operation_records_its_auth_generation_and_verified_receipt() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let t = Fake::default();
    Engine {
        session: &f.session,
        transport: &t,
    }
    .claim("synthetic-account")
    .await
    .unwrap();
    let vault = f.session.read().unwrap();
    let decoded = OwnedFile::registered(FILE)
        .unwrap()
        .decode(&vault, &f.raw())
        .unwrap();
    let saved: Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(saved["accounts"][0]["operation"]["boundAuthGeneration"], 1);
    assert_eq!(
        saved["accounts"][0]["operation"]["claimId"],
        "synthetic-claim"
    );
}
#[tokio::test]
async fn invalid_observed_status_fails_closed_instead_of_exposing_unknown_enum() {
    let f = Fixture::new().await;
    f.seed(|d| d.accounts[0].last_observed_status = Some(Status::Claimed));
    let t = Fake::default();
    assert_eq!(
        Engine {
            session: &f.session,
            transport: &t
        }
        .list()
        .await
        .unwrap_err(),
        Failure::StorageUnavailable
    );
}

#[test]
fn public_signature_matches_independently_computed_golden_bytes() {
    let client = reqwest::Client::new();
    for (endpoint, body, yy, signature) in [
        (
            Endpoint::Status,
            None,
            "b55c868c45b2e784edca4e8077826117",
            "05064c6d0266350eeac121b673536c14",
        ),
        (
            Endpoint::Claim,
            Some(json!({})),
            "23c36a4c5eb84d47fc526091e3cfa36e",
            "b0c74b46e28053fb22e53239b1061505",
        ),
    ] {
        let r = super::http::build_request(
            &client,
            Request {
                endpoint,
                region: Region::Cn,
                access_token: Some("synthetic-access".into()),
                real_user_id: Some("synthetic-user".into()),
                body,
            },
            1700000000123,
            28800,
            "linux",
            "1.2.3",
        )
        .unwrap();
        assert_eq!(r.headers()["yy"], yy);
        assert_eq!(r.headers()["x-signature"], signature);
    }
}
#[tokio::test]
async fn real_http_client_does_not_follow_redirect_or_retry_claim() {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    for redirect in [true, false] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(2);
            let mut count = 0;
            while Instant::now() < until {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        count += 1;
                        stream
                            .set_read_timeout(Some(Duration::from_millis(200)))
                            .unwrap();
                        let mut bytes = [0u8; 4096];
                        let _ = stream.read(&mut bytes);
                        if redirect {
                            write!(stream,"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{addr}/again\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
                        }
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            let until = Instant::now() + Duration::from_millis(250);
            while Instant::now() < until {
                if listener.accept().is_ok() {
                    count += 1
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            count
        });
        let client = super::http::new_client(Duration::from_millis(500)).unwrap();
        let request = client
            .post(format!("http://{addr}/claim"))
            .body("{}")
            .build()
            .unwrap();
        let result = super::http::send_once(&client, request).await;
        if redirect {
            assert_eq!(result.unwrap().status, 307)
        } else {
            assert_eq!(result.err(), Some(Failure::NetworkUnavailable))
        }
        assert_eq!(worker.join().unwrap(), 1);
    }
}

#[tokio::test]
async fn real_http_401_without_json_still_reaches_same_account_refresh_policy() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4096];
        assert!(stream.read(&mut buffer).unwrap() > 0);
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\nexpired").unwrap();
    });
    let client = super::http::new_client(std::time::Duration::from_secs(1)).unwrap();
    let response = super::http::send_once(
        &client,
        client.get(format!("http://{addr}/status")).build().unwrap(),
    )
    .await;
    server.join().unwrap();
    assert_eq!(response.unwrap().status, 401);
}

struct CommitPause {
    root: std::path::PathBuf,
    hit: std::sync::mpsc::Sender<()>,
    resume: Arc<std::sync::Barrier>,
}
static COMMIT_PAUSE: Mutex<Option<CommitPause>> = Mutex::new(None);
pub(super) fn before_durable(root: &std::path::Path) {
    let pause = {
        let mut slot = COMMIT_PAUSE.lock().unwrap();
        if slot.as_ref().is_some_and(|p| p.root == root) {
            slot.take()
        } else {
            None
        }
    };
    if let Some(pause) = pause {
        pause.hit.send(()).unwrap();
        pause.resume.wait();
    }
}
fn pause_commit(f: &Fixture) -> (std::sync::mpsc::Receiver<()>, Arc<std::sync::Barrier>) {
    let (hit, receiver) = std::sync::mpsc::channel();
    let resume = Arc::new(std::sync::Barrier::new(2));
    *COMMIT_PAUSE.lock().unwrap() = Some(CommitPause {
        root: f.root.path().into(),
        hit,
        resume: resume.clone(),
    });
    (receiver, resume)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_commit_fence_cancel_cannot_return_before_publication() {
    let f = Fixture::new().await;
    let remote = Arc::new(LoginHttp::new("synthetic-user"));
    let login = super::auth::begin(&f.session, remote.as_ref(), Region::Cn)
        .await
        .unwrap();
    let (hit, resume) = pause_commit(&f);
    let session = f.session.clone();
    let remote_poll = remote.clone();
    let flow = login.flow_id.clone();
    let poll =
        tokio::spawn(async move { super::auth::poll(&session, remote_poll.as_ref(), &flow).await });
    hit.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    let (returned, received) = std::sync::mpsc::channel();
    let session = f.session.clone();
    let handle = tokio::runtime::Handle::current();
    let (started, start) = std::sync::mpsc::channel();
    let cancel = std::thread::spawn(move || {
        started.send(()).unwrap();
        returned
            .send(handle.block_on(super::auth::cancel(&session, &login.flow_id)))
            .unwrap()
    });
    start
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    let early = received.recv_timeout(std::time::Duration::from_millis(100));
    resume.wait();
    let outcome = match early {
        Ok(v) => v,
        Err(_) => received
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap(),
    };
    cancel.join().unwrap();
    assert_eq!(
        poll.await.unwrap().unwrap().state,
        super::auth::AuthorizationState::Saved
    );
    assert!(
        early.is_err(),
        "cancel returned while admitted credentials were still unpublished"
    );
    assert_eq!(
        outcome,
        Err(Failure::AuthorizationCompleted),
        "completed authorization must not be reported canceled"
    );
    let vault = f.session.read().unwrap();
    let plaintext = OwnedFile::registered(FILE)
        .unwrap()
        .decode(&vault, &f.raw())
        .unwrap();
    let saved: Document = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(saved.accounts[0].real_user_id, "synthetic-user");
    assert!(saved.accounts[0].credentials.is_some());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_commit_fence_shutdown_cannot_return_before_publication() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let before = f.raw();
    let (hit, resume) = pause_commit(&f);
    let session = f.session.clone();
    let publish = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: &Fake::default(),
        }
        .refresh("synthetic-account")
        .await
    });
    hit.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    let (returned, received) = std::sync::mpsc::channel();
    let (started, start) = std::sync::mpsc::channel();
    let stop = std::thread::spawn(move || {
        started.send(()).unwrap();
        super::shutdown();
        returned.send(()).unwrap();
    });
    start
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap();
    let early = received.recv_timeout(std::time::Duration::from_millis(100));
    resume.wait();
    if early.is_err() {
        received
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
    }
    stop.join().unwrap();
    publish.await.unwrap().unwrap();
    assert_ne!(
        before,
        f.raw(),
        "admitted durable write must finish before shutdown returns"
    );
    assert!(
        early.is_err(),
        "shutdown returned while an old owner could still publish"
    );
}
#[tokio::test]
async fn lifecycle_commit_fence_shutdown_permanently_closes_new_requests() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    super::shutdown();
    let remote = LoginHttp::new("synthetic-user");
    let begin = super::auth::begin(&f.session, &remote, Region::Cn).await;
    let claim = Engine {
        session: &f.session,
        transport: &remote,
    }
    .claim("synthetic-account")
    .await;
    assert!(begin.is_err(), "shutdown must close new OAuth admission");
    assert!(claim.is_err(), "shutdown must close new claim admission");
    assert!(
        remote.calls.lock().unwrap().is_empty(),
        "stopped owner sent network requests"
    );
}

struct DispatchPause {
    root: std::path::PathBuf,
    endpoint: Endpoint,
    reached: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
}
static DISPATCH_PAUSE: Mutex<Option<DispatchPause>> = Mutex::new(None);
pub(super) async fn before_dispatch(root: &std::path::Path, endpoint: Endpoint) {
    let pause = {
        let mut slot = DISPATCH_PAUSE.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|p| p.root == root && p.endpoint == endpoint)
        {
            slot.take()
        } else {
            None
        }
    };
    if let Some(p) = pause {
        p.reached.notify_one();
        p.resume.notified().await;
    }
}
fn pause_dispatch(
    f: &Fixture,
    endpoint: Endpoint,
) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let reached = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    *DISPATCH_PAUSE.lock().unwrap() = Some(DispatchPause {
        root: f.root.path().into(),
        endpoint,
        reached: reached.clone(),
        resume: resume.clone(),
    });
    (reached, resume)
}
#[tokio::test]
async fn lifecycle_dispatch_shutdown_wins_before_claim_admission() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let (reached, resume) = pause_dispatch(&f, Endpoint::Claim);
    let remote = Arc::new(Fake::default());
    let task_remote = remote.clone();
    let session = f.session.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: task_remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    reached.notified().await;
    let before = f.raw();
    super::shutdown();
    resume.notify_one();
    assert!(task.await.unwrap().is_err());
    assert_eq!(before, f.raw(), "unresolved marker must survive shutdown");
    assert_eq!(
        *remote.calls.lock().unwrap(),
        vec![Endpoint::Status],
        "no claim admitted after shutdown returned"
    );
}
#[tokio::test]
async fn lifecycle_dispatch_retire_wins_before_claim_admission() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let (reached, resume) = pause_dispatch(&f, Endpoint::Claim);
    let remote = Arc::new(Fake::default());
    let task_remote = remote.clone();
    let session = f.session.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: task_remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    reached.notified().await;
    Engine {
        session: &f.session,
        transport: remote.as_ref(),
    }
    .retire("synthetic-account")
    .await
    .unwrap();
    resume.notify_one();
    assert!(task.await.unwrap().is_err());
    let view = Engine {
        session: &f.session,
        transport: remote.as_ref(),
    }
    .list()
    .await
    .unwrap()
    .remove(0);
    assert!(!view.authorized);
    assert_eq!(view.status, Status::Unknown);
    assert_eq!(
        *remote.calls.lock().unwrap(),
        vec![Endpoint::Status],
        "no claim admitted after retirement returned"
    );
}
#[tokio::test]
async fn lifecycle_dispatch_shutdown_wins_before_device_admission() {
    let f = Fixture::new().await;
    let (reached, resume) = pause_dispatch(&f, Endpoint::DeviceCode);
    let remote = Arc::new(LoginHttp::new("synthetic-user"));
    let task_remote = remote.clone();
    let session = f.session.clone();
    let task = tokio::spawn(async move {
        super::auth::begin(&session, task_remote.as_ref(), Region::Cn).await
    });
    reached.notified().await;
    super::shutdown();
    resume.notify_one();
    assert!(task.await.unwrap().is_err());
    assert!(
        remote.calls.lock().unwrap().is_empty(),
        "no device request admitted after shutdown returned"
    );
}

static AUTH_COMMIT_PAUSE: Mutex<Option<DispatchPause>> = Mutex::new(None);
pub(super) async fn before_authorization_commit(root: &std::path::Path) {
    let pause = {
        let mut slot = AUTH_COMMIT_PAUSE.lock().unwrap();
        if slot.as_ref().is_some_and(|p| p.root == root) {
            slot.take()
        } else {
            None
        }
    };
    if let Some(p) = pause {
        p.reached.notify_one();
        p.resume.notified().await;
    }
}
#[tokio::test]
async fn lifecycle_cancel_wins_after_profile_before_commit_no_credentials_published() {
    let f = Fixture::new().await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    *AUTH_COMMIT_PAUSE.lock().unwrap() = Some(DispatchPause {
        root: f.root.path().into(),
        endpoint: Endpoint::Profile,
        reached: reached.clone(),
        resume: resume.clone(),
    });
    let remote = Arc::new(LoginHttp::new("synthetic-user"));
    let login = super::auth::begin(&f.session, remote.as_ref(), Region::Cn)
        .await
        .unwrap();
    let session = f.session.clone();
    let remote_poll = remote.clone();
    let flow = login.flow_id.clone();
    let task =
        tokio::spawn(async move { super::auth::poll(&session, remote_poll.as_ref(), &flow).await });
    reached.notified().await;
    super::auth::cancel(&f.session, &login.flow_id)
        .await
        .unwrap();
    resume.notify_one();
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        Failure::ExpiredAuthorization
    );
    assert!(!f.root.path().join(FILE).exists());
    assert_eq!(
        *remote.calls.lock().unwrap(),
        vec![Endpoint::DeviceCode, Endpoint::Token, Endpoint::Profile]
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_pending_durable_rename_failure_dispatches_zero_claims() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let remote = Arc::new(Fake::default());
    let task_remote = remote.clone();
    let (hit, resume) = pause_commit(&f);
    let session = f.session.clone();
    let task = tokio::spawn(async move {
        Engine {
            session: &session,
            transport: task_remote.as_ref(),
        }
        .claim("synthetic-account")
        .await
    });
    hit.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
    let path = f.root.path().join(FILE);
    std::fs::rename(&path, path.with_extension("preserved-fixture")).unwrap();
    std::fs::create_dir(&path).unwrap();
    resume.wait();
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        Failure::StorageUnavailable
    );
    assert_eq!(*remote.calls.lock().unwrap(), vec![Endpoint::Status]);
}

#[tokio::test]
async fn lifecycle_old_device_flow_cannot_restore_credentials_after_retirement() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let remote = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &remote, Region::Cn)
        .await
        .unwrap();
    Engine {
        session: &f.session,
        transport: &remote,
    }
    .retire("synthetic-account")
    .await
    .unwrap();
    let before = f.raw();
    let result = super::auth::poll(&f.session, &remote, &login.flow_id).await;
    assert_eq!(
        result.unwrap_err(),
        Failure::StaleIdentity,
        "a device flow started before retirement must not restore that identity"
    );
    assert_eq!(before, f.raw());
    let view = Engine {
        session: &f.session,
        transport: &remote,
    }
    .list()
    .await
    .unwrap()
    .remove(0);
    assert!(!view.authorized);
    assert_eq!(view.auth_generation, 2);
}

#[tokio::test]
async fn lifecycle_old_flow_missing_identity_cannot_bypass_new_account_then_retirement() {
    let f = Fixture::new().await;
    let remote = LoginHttp::new("synthetic-user");
    let old = super::auth::begin(&f.session, &remote, Region::Cn)
        .await
        .unwrap();
    let new = super::auth::begin(&f.session, &remote, Region::Cn)
        .await
        .unwrap();
    let saved = super::auth::poll(&f.session, &remote, &new.flow_id)
        .await
        .unwrap()
        .account
        .unwrap();
    Engine {
        session: &f.session,
        transport: &remote,
    }
    .retire(&saved.id)
    .await
    .unwrap();
    let before = f.raw();
    assert_eq!(
        super::auth::poll(&f.session, &remote, &old.flow_id)
            .await
            .unwrap_err(),
        Failure::StaleIdentity
    );
    assert_eq!(before, f.raw());
}
#[tokio::test]
async fn lifecycle_same_identity_refresh_does_not_invalidate_explicit_device_flow() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let remote = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &remote, Region::Cn)
        .await
        .unwrap();
    *remote.status_401.lock().unwrap() = true;
    let refreshed = Engine {
        session: &f.session,
        transport: &remote,
    }
    .refresh("synthetic-account")
    .await
    .unwrap();
    assert_eq!(refreshed.auth_generation, 1);
    assert_eq!(refreshed.credential_version, 2);
    let saved = super::auth::poll(&f.session, &remote, &login.flow_id)
        .await
        .unwrap()
        .account
        .unwrap();
    assert_eq!(saved.auth_generation, 2);
    assert_eq!(saved.credential_version, 3);
}

#[tokio::test]
async fn lifecycle_other_identity_retirement_does_not_invalidate_target_flow() {
    let f = Fixture::new().await;
    f.seed(|d| {
        let mut other = account();
        other.id = "synthetic-other-account".into();
        other.real_user_id = "synthetic-other-user".into();
        d.accounts.push(other);
    });
    let remote = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &remote, Region::Cn)
        .await
        .unwrap();
    Engine {
        session: &f.session,
        transport: &remote,
    }
    .retire("synthetic-other-account")
    .await
    .unwrap();
    let saved = super::auth::poll(&f.session, &remote, &login.flow_id)
        .await
        .unwrap()
        .account
        .unwrap();
    assert_eq!(saved.real_user_id, "synthetic-user");
    assert_eq!(saved.auth_generation, 2);
    let views = Engine {
        session: &f.session,
        transport: &remote,
    }
    .list()
    .await
    .unwrap();
    assert!(
        !views
            .iter()
            .find(|a| a.id == "synthetic-other-account")
            .unwrap()
            .authorized
    );
}

static READBACK_FAULT: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
pub(super) fn fault_readback(root: &std::path::Path, bytes: Vec<u8>) -> Vec<u8> {
    let mut slot = READBACK_FAULT.lock().unwrap();
    if slot.as_deref() == Some(root) {
        slot.take();
        vec![0] // Only corrupt the returned buffer; the real durable file stays intact.
    } else {
        bytes
    }
}
#[tokio::test]
async fn lifecycle_partial_authorization_publication_remains_uncertain_without_replay() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let remote = LoginHttp::new("synthetic-user");
    let login = super::auth::begin(&f.session, &remote, Region::Cn)
        .await
        .unwrap();
    *READBACK_FAULT.lock().unwrap() = Some(f.root.path().into());
    let initial = super::auth::poll(&f.session, &remote, &login.flow_id).await;
    assert_eq!(initial.unwrap_err(), Failure::StorageUnavailable);
    assert!(
        READBACK_FAULT.lock().unwrap().is_none(),
        "fault must be one-shot and reached after real write"
    );
    let published = f.raw();
    {
        let vault = f.session.read().unwrap();
        let plain = OwnedFile::registered(FILE)
            .unwrap()
            .decode(&vault, &published)
            .unwrap();
        let document: Document = serde_json::from_slice(&plain).unwrap();
        assert_eq!(document.accounts[0].auth_generation, 2);
        assert_eq!(document.accounts[0].credential_version, 2);
        assert!(
            document.accounts[0].credentials.is_some(),
            "the durable publication must really have succeeded"
        );
    }
    let calls = remote.calls.lock().unwrap().clone();
    let cancel = super::auth::cancel(&f.session, &login.flow_id).await;
    let late = super::auth::poll(&f.session, &remote, &login.flow_id).await;
    assert_eq!(
        cancel,
        Err(Failure::StorageUnavailable),
        "unknown publication is not an expired or successfully canceled flow"
    );
    assert_eq!(late.unwrap_err(), Failure::StorageUnavailable);
    assert_eq!(
        calls,
        *remote.calls.lock().unwrap(),
        "late polling must never replay OAuth after partial publication"
    );
    assert_eq!(
        published,
        f.raw(),
        "must retain the published credentials without another generation increment"
    );
}

#[path = "inference_tests.rs"]
mod inference_tests;
