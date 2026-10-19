use super::store::*;
use crate::secrets::{session::SecretSession, VaultContext};
use std::sync::Arc;
struct Fixture {
    _guard: tokio::sync::OwnedMutexGuard<()>,
    root: tempfile::TempDir,
    session: Arc<SecretSession>,
}
impl Fixture {
    async fn new() -> Self {
        let g = fixture_guard().await;
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        Self {
            _guard: g,
            root,
            session,
        }
    }
    async fn seed(&self, realm: Realm) -> String {
        let id = format!("synthetic-{realm:?}");
        access(&self.session, None, |d, _| {
            d.accounts.push(account(&id, realm));
            Ok(((), true))
        })
        .await
        .unwrap();
        id
    }
}
fn account(id: &str, realm: Realm) -> Account {
    Account {
        id: id.into(),
        uid: "synthetic-user".into(),
        realm,
        generation: 1,
        version: 1,
        device: Device {
            auth_host: realm.auth().into(),
            uid: "synthetic-user".into(),
            id: "synthetic-device".into(),
            machine: "synthetic-machine".into(),
            client: "ono9krqynydwx5".into(),
            version: "synthetic-version".into(),
            version_code: "20260917".into(),
            os: "linux".into(),
            os_version: "synthetic-os".into(),
            source_digest: "a".repeat(64),
            private_key: String::new(),
            public_key: String::new(),
        },
        grant: Some(Grant {
            access: "synthetic-access".into(),
            refresh: "synthetic-refresh".into(),
            expires: now() + 3600000,
        }),
        refresh_pending: false,
        checkin_day: None,
        imported_refresh_hash: {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(b"synthetic-refresh"))
        },
    }
}
#[tokio::test]
async fn encrypted_owner_survives_restart_without_plaintext() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    let raw = std::fs::read(f.root.path().join("trae_accounts.json")).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("synthetic-access"));
    let vault = f.session.read().unwrap().clone();
    let restart = SecretSession::from_context(f.root.path().into(), vault);
    assert_eq!(
        snapshot(&restart, &id).await.unwrap().account.uid,
        "synthetic-user"
    );
}
#[tokio::test]
async fn retired_locked_and_shutdown_accounts_fail_closed() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    let before = snapshot(&f.session, &id).await.unwrap();
    retire(&f.session, &id).await.unwrap();
    assert!(access(&f.session, Some(&before.binding), |d, _| {
        matched(d, &before)?;
        Ok(((), false))
    })
    .await
    .is_err());
    f.session.set_blocked(true);
    assert!(snapshot(&f.session, &id).await.is_err());
    f.session.set_blocked(false);
    assert!(
        access(&f.session, Some(&before.binding), |_, _| Ok(((), false)))
            .await
            .is_err()
    );
    shutdown();
    assert!(snapshot(&f.session, &id).await.is_err());
}
#[tokio::test]
async fn pending_rotation_is_not_replayed_after_restart() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    access(&f.session, None, |d, _| {
        d.accounts[0].refresh_pending = true;
        Ok(((), true))
    })
    .await
    .unwrap();
    assert!(snapshot(&f.session, &id).await.is_err());
}
use super::inference::*;
use crate::proxy::{hyper_client::ProxyResponse, ProxyError};
use bytes::Bytes;
use serde_json::{json, Value};
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Mutex, time::Duration};
#[derive(Default)]
struct Http {
    calls: Mutex<Vec<Request>>,
    responses: Mutex<VecDeque<ProxyResponse>>,
}
impl Transport for Http {
    fn send(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProxyError::ForwardFailed("synthetic exhausted".into()))
        })
    }
}
fn upstream(status: u16, chunks: Vec<Result<Bytes, std::io::Error>>) -> ProxyResponse {
    let mut h = http::HeaderMap::new();
    h.insert(
        "content-type",
        http::HeaderValue::from_static("text/event-stream"),
    );
    ProxyResponse::streamed(
        http::StatusCode::from_u16(status).unwrap(),
        h,
        futures::stream::iter(chunks),
    )
}
fn ok() -> ProxyResponse {
    upstream(
        200,
        vec![Ok(Bytes::from_static(
            b"event: output\ndata: {\"response\":\"hello\"}\n\nevent: done\ndata: {}\n\n",
        ))],
    )
}
fn chat() -> Value {
    json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"hello"}]})
}
#[tokio::test]
async fn selected_realms_have_distinct_hosts_and_no_fabricated_device() {
    let f = Fixture::new().await;
    let wire = Http::default();
    for realm in [Realm::Sg, Realm::Us] {
        let id = f.seed(realm).await;
        wire.responses.lock().unwrap().push_back(ok());
        let result = Lease::acquire(f.session.clone(), &id, "conversation")
            .await
            .unwrap()
            .dispatch_with(chat(), Duration::from_secs(2), &wire)
            .await
            .unwrap();
        result.fence.check().await.unwrap();
        let bytes = result.response.bytes_with_limit(4096).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("chat.completion.chunk"));
    }
    let calls = wire.calls.lock().unwrap();
    for (i, realm) in [Realm::Sg, Realm::Us].iter().enumerate() {
        assert_eq!(
            calls[i].url,
            format!("{}/api/agent/v3/llm_utils_chat", realm.api())
        );
        assert_eq!(calls[i].headers["x-device-id"], "synthetic-device");
        assert!(calls[i].headers["authorization"].is_sensitive());
        assert_eq!(
            calls[i].headers["authorization"],
            "Cloud-IDE-JWT synthetic-access"
        );
    }
    assert_ne!(calls[0].body["session_id"], calls[1].body["session_id"]);
}
#[tokio::test]
async fn unsupported_tools_and_retirement_send_zero_http() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    let lease = Lease::acquire(f.session.clone(), &id, "s").await.unwrap();
    let mut body = chat();
    body["tools"] = json!([{"function":{"name":"x"}}]);
    assert!(lease
        .dispatch_with(body, Duration::from_secs(2), &wire)
        .await
        .is_err());
    let lease = Lease::acquire(f.session.clone(), &id, "s").await.unwrap();
    retire(&f.session, &id).await.unwrap();
    assert!(lease
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .is_err());
    assert!(wire.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn first_denial_is_generic_403_without_retry() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().push_back(upstream(
        200,
        vec![Ok(Bytes::from_static(
            b"event: error\ndata: {\"code\":11004,\"message\":\"synthetic-refresh\"}\n\n",
        ))],
    ));
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 403);
    assert!(
        !String::from_utf8_lossy(&result.response.bytes_with_limit(4096).await.unwrap())
            .contains("synthetic-refresh")
    );
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn post_output_error_is_terminal_without_replay() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().push_back(upstream(
        200,
        vec![
            Ok(Bytes::from_static(
                b"event: output\ndata: {\"response\":\"first\"}\n\n",
            )),
            Ok(Bytes::from_static(
                b"event: error\ndata: {\"code\":11004,\"message\":\"private\"}\n\n",
            )),
        ],
    ));
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert!(result.response.bytes_with_limit(4096).await.is_err());
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn final_fence_rejects_buffered_bytes_after_retirement() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().push_back(ok());
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    result.response.bytes_with_limit(4096).await.unwrap();
    retire(&f.session, &id).await.unwrap();
    assert!(result.fence.check().await.is_err());
}

fn token(uid: &str) -> String {
    use base64::Engine;
    format!(
        "e30.{}.synthetic-signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({"data":{"user_id":uid},"exp":4102444800_i64})).unwrap()
        )
    )
}
fn refresh_body(uid: &str, device: &str) -> Value {
    json!({"Result":{"Token":token(uid),"RefreshToken":"synthetic-rotated-refresh","TokenExpireAt":4102444800000_i64,"BoundDeviceID":device,"DeviceBindStatus":"BOUND"}})
}
#[tokio::test]
async fn refresh_response_requires_bound_registered_device_and_unexpired_metadata() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    let s = snapshot(&f.session, &id).await.unwrap();
    let body = refresh_body("synthetic-user", "synthetic-device");
    assert_eq!(
        super::auth::accepted(&s, &body).unwrap().refresh,
        "synthetic-rotated-refresh"
    );
    assert!(super::auth::accepted(&s, &refresh_body("synthetic-user", "other")).is_err());
    let mut expired = body.clone();
    expired["Result"]["TokenExpireAt"] = json!(1);
    assert!(super::auth::accepted(&s, &expired).is_err());
    let mut wrong = body;
    wrong["Result"]["DeviceBindStatus"] = json!("UNBOUND");
    assert!(super::auth::accepted(&s, &wrong).is_err());
}

async fn keyed(f: &Fixture, expired: bool) -> String {
    let mut a = profile_material().account;
    a.id = "synthetic-keyed".into();
    if expired {
        a.grant.as_mut().unwrap().expires = now() - 1;
    }
    let id = a.id.clone();
    access(&f.session, None, |d, _| {
        d.accounts.push(a);
        Ok(((), true))
    })
    .await
    .unwrap();
    id
}
fn exchange_response(device: &str) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::OK,
        http::HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&refresh_body("synthetic-user", device)).unwrap()),
    )
}
#[tokio::test]
async fn expired_grant_refreshes_once_and_commits_before_inference() {
    let f = Fixture::new().await;
    let id = keyed(&f, true).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        exchange_response("1234567890123456"),
        profile_response("synthetic-user", "SG"),
        ok(),
    ]);
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 200);
    {
        let calls = wire.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[0].url,
            "https://growsg-normal.trae.ai/trae/api/v3/oauth/ExchangeToken"
        );
        assert_eq!(calls[0].body["DeviceInfo"]["DeviceID"], "1234567890123456");
        assert_eq!(
            calls[2].headers["authorization"],
            format!("Cloud-IDE-JWT {}", token("synthetic-user"))
        );
    }
    let row = snapshot(&f.session, &id).await.unwrap().account;
    assert_eq!(row.version, 2);
    assert!(!row.refresh_pending);
}
#[tokio::test]
async fn explicit_401_uses_one_refresh_budget_and_never_replays_twice() {
    let f = Fixture::new().await;
    let id = keyed(&f, false).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        upstream(401, vec![]),
        exchange_response("1234567890123456"),
        profile_response("synthetic-user", "SG"),
        upstream(401, vec![]),
    ]);
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 401);
    assert_eq!(wire.calls.lock().unwrap().len(), 4);
}
#[tokio::test]
async fn refresh_mismatched_bound_device_stops_without_legacy_or_chat() {
    let f = Fixture::new().await;
    let id = keyed(&f, true).await;
    let wire = Http::default();
    wire.responses
        .lock()
        .unwrap()
        .push_back(exchange_response("different-device"));
    assert!(Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .is_err());
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
    assert!(snapshot(&f.session, &id).await.is_err());
}
#[tokio::test]
async fn selected_material_activates_only_after_bound_server_proof() {
    let f = Fixture::new().await;
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        exchange_response("1234567890123456"),
        profile_response("synthetic-user", "SG"),
    ]);
    let id = super::auth::activate_with(
        f.session.clone(),
        profile_material(),
        Duration::from_secs(2),
        &wire,
    )
    .await
    .unwrap();
    let row = snapshot(&f.session, &id).await.unwrap().account;
    assert_eq!(row.uid, "synthetic-user");
    assert_eq!(row.grant.unwrap().access, token("synthetic-user"));
    assert_eq!(wire.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn simultaneous_expiry_uses_one_exchange_without_losing_either_request() {
    let f = Fixture::new().await;
    let id = keyed(&f, true).await;
    let a = Lease::acquire(f.session.clone(), &id, "a").await.unwrap();
    let b = Lease::acquire(f.session.clone(), &id, "b").await.unwrap();
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        exchange_response("1234567890123456"),
        profile_response("synthetic-user", "SG"),
        ok(),
        ok(),
    ]);
    let (ra, rb) = tokio::join!(
        a.dispatch_with(chat(), Duration::from_secs(2), &wire),
        b.dispatch_with(chat(), Duration::from_secs(2), &wire)
    );
    assert!(ra.is_ok());
    assert!(rb.is_ok());
    let calls = wire.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.url.ends_with("/ExchangeToken"))
            .count(),
        1
    );
    assert_eq!(calls.len(), 4);
}
struct RetireDuringExchange {
    session: Arc<SecretSession>,
    id: String,
    calls: Mutex<usize>,
}
impl Transport for RetireDuringExchange {
    fn send(
        &self,
        _r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            retire(&self.session, &self.id).await.unwrap();
            Ok(exchange_response("1234567890123456"))
        })
    }
}
#[tokio::test]
async fn refresh_result_cannot_resurrect_retired_account() {
    let f = Fixture::new().await;
    let id = keyed(&f, true).await;
    let wire = RetireDuringExchange {
        session: f.session.clone(),
        id: id.clone(),
        calls: Mutex::new(0),
    };
    assert!(Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .is_err());
    assert_eq!(*wire.calls.lock().unwrap(), 1);
    assert!(snapshot(&f.session, &id).await.is_err());
}
fn checkin_reply(body: Value) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::OK,
        http::HeaderMap::new(),
        Bytes::from(serde_json::to_vec(&body).unwrap()),
    )
}
#[tokio::test]
async fn cn_checkin_uses_original_device_and_persists_claim_admission() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        checkin_reply(json!({"code":0,"enable":true,"checked_in":false})),
        checkin_reply(json!({"code":0,"credits":100})),
    ]);
    let lease = Lease::checkin_fixture(f.session.clone(), &id)
        .await
        .unwrap();
    assert_eq!(
        super::checkin::run_with(lease, Duration::from_secs(2), &wire)
            .await
            .unwrap(),
        super::checkin::Outcome::Claimed
    );
    {
        let calls = wire.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].url.ends_with("/checkin_credits/status"));
        assert!(calls[1].url.ends_with("/checkin_credits/claim"));
        assert_eq!(calls[1].headers["x-device-id"], "synthetic-device");
        assert_eq!(calls[1].body, json!({"req_source":1}));
    }
    let restart =
        SecretSession::from_context(f.root.path().into(), f.session.read().unwrap().clone());
    let lease = Lease::checkin_fixture(restart, &id).await.unwrap();
    assert_eq!(
        super::checkin::run_with(lease, Duration::from_secs(2), &wire)
            .await
            .unwrap(),
        super::checkin::Outcome::AlreadyAttempted
    );
    assert_eq!(wire.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn checkin_global_done_disabled_and_invalid_status_never_claim() {
    let f = Fixture::new().await;
    let global = f.seed(Realm::Sg).await;
    let wire = Http::default();
    let lease = Lease::acquire(f.session.clone(), &global, "s")
        .await
        .unwrap();
    assert_eq!(
        super::checkin::run_with(lease, Duration::from_secs(2), &wire)
            .await
            .unwrap(),
        super::checkin::Outcome::Inactive
    );
    assert!(wire.calls.lock().unwrap().is_empty());
    let id = f.seed(Realm::Cn).await;
    for (body, outcome) in [
        (
            json!({"code":0,"enable":true,"checked_in":true}),
            Some(super::checkin::Outcome::Done),
        ),
        (
            json!({"code":0,"enable":false}),
            Some(super::checkin::Outcome::Inactive),
        ),
        (json!({"notStatus":true}), None),
        (json!({"code":11004,"message":"synthetic-refresh"}), None),
    ] {
        wire.responses
            .lock()
            .unwrap()
            .push_back(checkin_reply(body));
        let lease = Lease::checkin_fixture(f.session.clone(), &id)
            .await
            .unwrap();
        let result = super::checkin::run_with(lease, Duration::from_secs(2), &wire).await;
        if let Some(outcome) = outcome {
            assert_eq!(result.unwrap(), outcome);
        } else {
            assert!(result.is_err());
        }
    }
    assert_eq!(wire.calls.lock().unwrap().len(), 4);
    assert!(wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.url.ends_with("/status")));
}
#[tokio::test]
async fn checkin_restriction_is_terminal_without_device_or_auth_retry() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    for code in [9074, 9095] {
        access(&f.session, None, |d, _| {
            d.accounts[0].checkin_day = None;
            Ok(((), true))
        })
        .await
        .unwrap();
        let wire = Http::default();
        wire.responses.lock().unwrap().extend([
            checkin_reply(json!({"code":0,"enable":true,"checked_in":false})),
            checkin_reply(json!({"code":code,"message":"synthetic-refresh"})),
        ]);
        let lease = Lease::checkin_fixture(f.session.clone(), &id)
            .await
            .unwrap();
        assert_eq!(
            super::checkin::run_with(lease, Duration::from_secs(2), &wire)
                .await
                .unwrap(),
            super::checkin::Outcome::Ineligible
        );
        let lease = Lease::checkin_fixture(f.session.clone(), &id)
            .await
            .unwrap();
        assert_eq!(
            super::checkin::run_with(lease, Duration::from_secs(2), &wire)
                .await
                .unwrap(),
            super::checkin::Outcome::AlreadyAttempted
        );
        assert_eq!(wire.calls.lock().unwrap().len(), 2);
    }
}
#[tokio::test]
async fn original_raw_known_secret_guard_is_not_lost_during_projection() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().push_back(upstream(200,vec![Ok(Bytes::from_static(
        b": synthetic-refresh\n\nevent: output\ndata: {\"response\":\"safe visible text\"}\n\nevent: done\ndata: {}\n\n"
    ))]));
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 502);
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn structured_permission_denial_never_spends_http401_refresh_budget() {
    let f = Fixture::new().await;
    let id = keyed(&f, false).await;
    let wire = Http::default();
    wire.responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::buffered(
            http::StatusCode::UNAUTHORIZED,
            http::HeaderMap::new(),
            Bytes::from_static(b"{\"code\":11004,\"message\":\"synthetic-refresh\"}"),
        ));
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 403);
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn non_sse_structured_denial_is_sanitized_without_retry() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    wire.responses.lock().unwrap().push_back(checkin_reply(
        json!({"code":11004,"message":"synthetic-refresh"}),
    ));
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 403);
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn ambiguous_activation_cannot_replay_the_same_original_material() {
    let f = Fixture::new().await;
    let material = profile_material();
    let mut copy = super::import::Material {
        account: material.account.clone(),
    };
    copy.account.device.source_digest = "b".repeat(64); // cosmetic storage changes do not make refresh new
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        exchange_response("wrong-device"),
        exchange_response("1234567890123456"),
    ]);
    assert!(
        super::auth::activate_with(f.session.clone(), material, Duration::from_secs(2), &wire)
            .await
            .is_err()
    );
    assert!(
        super::auth::activate_with(f.session.clone(), copy, Duration::from_secs(2), &wire)
            .await
            .is_err()
    );
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn contradictory_server_realm_or_client_identity_is_never_accepted() {
    let f = Fixture::new().await;
    let id = keyed(&f, false).await;
    let s = snapshot(&f.session, &id).await.unwrap();
    for patch in [
        json!({"UserInfo":{"UserID":"synthetic-user","Host":"https://api-sg-central.trae.ai","AIRegion":"SG"}}),
        json!({"ClientID":123}),
    ] {
        let mut body = refresh_body("synthetic-user", "1234567890123456");
        body["Result"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(super::auth::accepted(&s, &body).is_err());
    }
}
async fn projected_secret_is_blocked(stream: bool, split: bool, reasoning: bool) {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Sg).await;
    let wire = Http::default();
    let raw = if split {
        b"event: output\ndata: {\"response\":\"synthetic-\"}\n\nevent: output\ndata: {\"response\":\"refresh\"}\n\nevent: done\ndata: {}\n\n".as_slice()
    } else {
        b"event: output\ndata: {\"response\":\"synthetic\\u002drefresh\"}\n\nevent: done\ndata: {}\n\n".as_slice()
    };
    let raw = if reasoning {
        std::str::from_utf8(raw)
            .unwrap()
            .replace("\"response\"", "\"reasoning_content\"")
            .into_bytes()
    } else {
        raw.to_vec()
    };
    wire.responses
        .lock()
        .unwrap()
        .push_back(upstream(200, vec![Ok(Bytes::from(raw))]));
    let mut body = chat();
    body["stream"] = json!(stream);
    let response = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(body, Duration::from_secs(2), &wire)
        .await
        .unwrap()
        .response;
    let initial_error = !response.status().is_success();
    use futures::StreamExt;
    let mut bytes = Box::pin(response.bytes_stream());
    let mut terminal_error = false;
    let mut out = Vec::new();
    while let Some(item) = bytes.next().await {
        match item {
            Ok(b) => out.extend_from_slice(&b),
            Err(_) => {
                terminal_error = true;
                break;
            }
        }
    }
    assert!(
        initial_error || terminal_error,
        "decoded credential was allowed through projection"
    );
    assert!(!String::from_utf8_lossy(&out).contains("synthetic-"));
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn streamed_json_escape_cannot_disclose_a_known_secret() {
    projected_secret_is_blocked(true, false, false).await
}
#[tokio::test]
async fn buffered_json_escape_cannot_disclose_a_known_secret() {
    projected_secret_is_blocked(false, false, false).await
}
#[tokio::test]
async fn streamed_semantic_delta_split_cannot_disclose_a_known_secret() {
    projected_secret_is_blocked(true, true, false).await
}
#[tokio::test]
async fn buffered_semantic_delta_split_cannot_disclose_a_known_secret() {
    projected_secret_is_blocked(false, true, false).await
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteFault {
    BeforePublication,
    PostRenameSync,
    Readback,
}
static WRITE_FAULT: Mutex<Option<(std::path::PathBuf, WriteFault)>> = Mutex::new(None);
pub(super) fn arm_write_fault(path: std::path::PathBuf, fault: WriteFault) {
    *WRITE_FAULT.lock().unwrap() = Some((path, fault));
}
pub(super) fn durable_write(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), crate::error::AppError> {
    let fault = {
        let mut g = WRITE_FAULT.lock().unwrap();
        if g.as_ref().is_some_and(|(p, f)| {
            p == path
                && matches!(
                    f,
                    WriteFault::PostRenameSync | WriteFault::BeforePublication
                )
        }) {
            g.take()
        } else {
            None
        }
    };
    if fault
        .as_ref()
        .is_some_and(|(_, f)| *f == WriteFault::BeforePublication)
    {
        return Err(crate::error::AppError::Config(
            "synthetic.before_publication".into(),
        ));
    }
    if fault.is_some() {
        // Real production atomic publication, then the original next fsync boundary fails.
        crate::config_file_io::atomic_write_private(path, bytes)?;
        return Err(crate::error::AppError::Config(
            "synthetic.post_rename_fsync".into(),
        ));
    }
    crate::config_file_io::write_durable(path, bytes)
}
pub(super) fn readback(path: &std::path::Path, bytes: Vec<u8>) -> Vec<u8> {
    let mut g = WRITE_FAULT.lock().unwrap();
    if g.as_ref()
        .is_some_and(|(p, f)| p == path && *f == WriteFault::Readback)
    {
        g.take();
        vec![0]
    } else {
        bytes
    }
}
struct FaultExchange {
    calls: Mutex<usize>,
    path: std::path::PathBuf,
    fault: WriteFault,
}
impl Transport for FaultExchange {
    fn send(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            if r.url.ends_with("/GetUserInfo") {
                return Ok(profile_response("synthetic-user", "SG"));
            }
            assert!(r.url.ends_with("/ExchangeToken"));
            *self.calls.lock().unwrap() += 1;
            *WRITE_FAULT.lock().unwrap() = Some((self.path.clone(), self.fault));
            Ok(exchange_response("1234567890123456"))
        })
    }
}
async fn publication_failure_recovers_without_replay(fault: WriteFault) {
    let f = Fixture::new().await;
    let id = keyed(&f, true).await;
    let before = snapshot(&f.session, &id).await.unwrap();
    let vault = f.session.read().unwrap().clone();
    let wire = FaultExchange {
        calls: Mutex::new(0),
        path: f.root.path().join("trae_accounts.json"),
        fault,
    };
    let result = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await;
    if fault == WriteFault::BeforePublication {
        assert!(result.is_err());
    } else {
        assert!(
            matches!(result,Err(ProxyError::AuthError(ref code)) if code=="trae.publication_unknown")
        );
    }

    assert!(
        WRITE_FAULT.lock().unwrap().is_none(),
        "real publication boundary must be reached"
    );
    let restart = SecretSession::from_context(f.root.path().into(), vault);
    if fault == WriteFault::BeforePublication {
        assert!(snapshot(&f.session, &id).await.is_err());
        assert!(snapshot(&restart, &id).await.is_err());
    } else {
        // The original contract allows a completely validated, already-published
        // grant to be recovered. Err never means "nothing was saved".
        for session in [&f.session, &restart] {
            let recovered = snapshot(session, &id).await.unwrap();
            assert_eq!(recovered.account.generation, before.account.generation);
            assert!(
                recovered.account.device == before.account.device,
                "full original device binding must survive exact recovery"
            );
            assert_eq!(recovered.account.version, 2);
            assert_eq!(recovered.account.uid, "synthetic-user");
            assert_eq!(recovered.account.device.id, "1234567890123456");
            assert_eq!(recovered.account.realm, Realm::Sg);
            assert_eq!(
                recovered.account.grant.as_ref().unwrap().access,
                token("synthetic-user")
            );
            assert!(!recovered.account.refresh_pending);
        }
    }
    assert_eq!(
        *wire.calls.lock().unwrap(),
        1,
        "recovery must not replay the remote exchange"
    );
}
#[tokio::test]
async fn post_rename_fsync_unknown_recovers_exact_commit_without_exchange_replay() {
    publication_failure_recovers_without_replay(WriteFault::PostRenameSync).await
}
#[tokio::test]
async fn post_publication_readback_unknown_recovers_exact_commit_without_exchange_replay() {
    publication_failure_recovers_without_replay(WriteFault::Readback).await
}

#[tokio::test]
async fn proven_prepublication_failure_stays_pending_after_restart() {
    publication_failure_recovers_without_replay(WriteFault::BeforePublication).await
}

#[tokio::test]
async fn concurrent_original_material_import_makes_one_exchange() {
    let f = Fixture::new().await;
    let material = profile_material();
    let mut copy = super::import::Material {
        account: material.account.clone(),
    };
    copy.account.device.source_digest = "c".repeat(64);
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        exchange_response("1234567890123456"),
        profile_response("synthetic-user", "SG"),
    ]);
    let (a, b) = tokio::join!(
        super::auth::activate_with(f.session.clone(), material, Duration::from_secs(2), &wire),
        super::auth::activate_with(f.session.clone(), copy, Duration::from_secs(2), &wire)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let calls = wire.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.url.ends_with("/ExchangeToken"))
            .count(),
        1
    );
}

#[tokio::test]
async fn reasoning_only_json_escape_and_delta_split_are_screened() {
    for stream in [false, true] {
        for split in [false, true] {
            projected_secret_is_blocked(stream, split, true).await;
        }
    }
}
#[tokio::test]
async fn account_projection_exposes_only_opaque_id_realm_and_local_status() {
    let f = Fixture::new().await;
    let id = f.seed(Realm::Cn).await;
    let path = f.root.path().join("trae_accounts.json");
    let before = std::fs::read(&path).unwrap();
    let views = list(&f.session).await.unwrap();
    assert_eq!(
        views,
        vec![AccountView {
            id: id.clone(),
            realm: Realm::Cn,
            status: AccountStatus::Stored
        }]
    );
    let encoded = serde_json::to_string(&views).unwrap();
    for private in [
        "synthetic-user",
        "synthetic-access",
        "synthetic-refresh",
        "private_key",
        "machine",
    ] {
        assert!(!encoded.contains(private));
    }
    assert!(lookup(&f.session, &id, Realm::Sg).await.unwrap().is_none());
    assert!(lookup(&f.session, "other", Realm::Cn)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "projection must not write credentials"
    );
}
#[tokio::test]
async fn first_activation_unknown_can_be_discovered_without_reimporting_credentials() {
    let f = Fixture::new().await;
    let vault = f.session.read().unwrap().clone();
    let wire = FaultExchange {
        calls: Mutex::new(0),
        path: f.root.path().join("trae_accounts.json"),
        fault: WriteFault::Readback,
    };
    let result = super::auth::activate_with(
        f.session.clone(),
        profile_material(),
        Duration::from_secs(2),
        &wire,
    )
    .await;
    assert!(matches!(result,Err(ProxyError::AuthError(ref c)) if c=="trae.publication_unknown"));
    let views = list(&f.session).await.unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].status, AccountStatus::Stored);
    assert_eq!(views[0].id.len(), 64);
    let restart = SecretSession::from_context(f.root.path().into(), vault);
    assert_eq!(
        lookup(&restart, &views[0].id, Realm::Sg).await.unwrap(),
        Some(views[0].clone())
    );
    assert_eq!(*wire.calls.lock().unwrap(), 1);
}
#[tokio::test]
async fn listing_pending_or_retired_authorization_cannot_enable_inference() {
    let f = Fixture::new().await;
    let wire = Http::default();
    wire.responses
        .lock()
        .unwrap()
        .push_back(exchange_response("wrong-device"));
    assert!(super::auth::activate_with(
        f.session.clone(),
        profile_material(),
        Duration::from_secs(2),
        &wire
    )
    .await
    .is_err());
    let views = list(&f.session).await.unwrap();
    assert_eq!(views[0].status, AccountStatus::RefreshPending);
    assert!(snapshot(&f.session, &views[0].id).await.is_err());
    retire(&f.session, &views[0].id).await.unwrap();
    assert_eq!(
        lookup(&f.session, &views[0].id, Realm::Sg)
            .await
            .unwrap()
            .unwrap()
            .status,
        AccountStatus::Retired
    );
    assert!(snapshot(&f.session, &views[0].id).await.is_err());
    assert_eq!(wire.calls.lock().unwrap().len(), 1);
}

// Current official I18N desktop profile contract. All inputs remain synthetic.
fn profile_material() -> super::import::Material {
    let mut material = super::import::synthetic_material();
    material.account.realm = Realm::Sg;
    material.account.device.auth_host = Realm::Sg.auth().into();
    material.account.device.version = "3.6.2".into();
    material
}
fn profile_response(uid: &str, region: &str) -> ProxyResponse {
    checkin_reply(json!({"Result":{"UserID":uid,"AIRegion":region}}))
}
#[tokio::test]
async fn fresh_profile_accepts_opaque_token_then_commits_before_chat() {
    let f = Fixture::new().await;
    let wire = Http::default();
    let mut body = refresh_body("irrelevant-cli-claim", "1234567890123456");
    body["Result"]["Token"] = json!("synthetic-opaque-fresh-token");
    wire.responses.lock().unwrap().extend([
        checkin_reply(body),
        profile_response("synthetic-user", "sg"),
        ok(),
    ]);
    let id = super::auth::activate_with(
        f.session.clone(),
        profile_material(),
        Duration::from_secs(2),
        &wire,
    )
    .await
    .unwrap();
    let restart =
        SecretSession::from_context(f.root.path().into(), f.session.read().unwrap().clone());
    let saved = snapshot(&restart, &id).await.unwrap();
    assert_eq!(
        saved.account.grant.as_ref().unwrap().access,
        "synthetic-opaque-fresh-token"
    );
    assert_eq!(saved.account.version, 2);
    assert!(!saved.account.refresh_pending);
    let result = Lease::acquire(restart, &id, "synthetic-profile")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 200);
    let calls = wire.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[0].url,
        "https://growsg-normal.trae.ai/trae/api/v3/oauth/ExchangeToken"
    );
    assert_eq!(
        calls[1].url,
        "https://growsg-normal.trae.ai/cloudide/api/v3/trae/GetUserInfo"
    );
    assert_eq!(
        calls[1].headers["x-cloudide-token"],
        "synthetic-opaque-fresh-token"
    );
    assert!(calls[1].headers["x-cloudide-token"].is_sensitive());
    assert_eq!(
        calls[1].body,
        json!({"ReqSource":"IDE","IDEVersion":"3.6.2"})
    );
    assert!(!calls[1].body.to_string().contains("synthetic-refresh"));
    assert_eq!(
        calls[2].headers["authorization"],
        "Cloud-IDE-JWT synthetic-opaque-fresh-token"
    );
}
#[tokio::test]
async fn fresh_profile_uid_realm_and_advertised_host_must_match_original_row() {
    for info in [
        json!({"UserID":"other-user","AIRegion":"SG"}),
        json!({"UserID":"synthetic-user","AIRegion":"CN"}),
        json!({"UserID":123,"AIRegion":"SG"}),
        json!({"UserID":"synthetic-user"}),
        json!({"UserID":"synthetic-user","AIRegion":"SG","Host":"https://unrelated.invalid"}),
    ] {
        let f = Fixture::new().await;
        let wire = Http::default();
        wire.responses.lock().unwrap().extend([
            exchange_response("1234567890123456"),
            checkin_reply(json!({"Result":info})),
        ]);
        assert!(super::auth::activate_with(
            f.session.clone(),
            profile_material(),
            Duration::from_secs(2),
            &wire
        )
        .await
        .is_err());
        let views = list(&f.session).await.unwrap();
        assert_eq!(views[0].status, AccountStatus::RefreshPending);
        assert!(snapshot(&f.session, &views[0].id).await.is_err());
        let restart =
            SecretSession::from_context(f.root.path().into(), f.session.read().unwrap().clone());
        assert!(Lease::acquire(restart, &views[0].id, "s").await.is_err());
        assert_eq!(wire.calls.lock().unwrap().len(), 2);
        assert!(super::auth::activate_with(
            f.session.clone(),
            profile_material(),
            Duration::from_secs(2),
            &wire
        )
        .await
        .is_err());
        assert_eq!(
            wire.calls.lock().unwrap().len(),
            2,
            "unknown attempt must not replay exchange/profile"
        );
    }
}
#[tokio::test]
async fn fresh_profile_error_empty_and_bounded_body_fail_without_replay() {
    for profile in [
        ProxyResponse::buffered(
            http::StatusCode::FORBIDDEN,
            http::HeaderMap::new(),
            Bytes::from_static(b"synthetic-refresh"),
        ),
        checkin_reply(json!({"code":11004,"message":"synthetic-refresh"})),
        checkin_reply(
            json!({"ResponseMetadata":{"Error":{"Code":"Unauthorized","Message":"synthetic-refresh"}},"Result":{"UserID":"synthetic-user","AIRegion":"SG"}}),
        ),
        checkin_reply(
            json!({"code":"invalid","Result":{"UserID":"synthetic-user","AIRegion":"SG"}}),
        ),
        checkin_reply(json!({})),
        ProxyResponse::buffered(
            http::StatusCode::OK,
            http::HeaderMap::new(),
            Bytes::from_static(b"not JSON synthetic-refresh"),
        ),
        ProxyResponse::buffered(
            http::StatusCode::OK,
            http::HeaderMap::new(),
            Bytes::from(vec![b'x'; 1024 * 1024 + 1]),
        ),
    ] {
        let f = Fixture::new().await;
        let wire = Http::default();
        wire.responses
            .lock()
            .unwrap()
            .extend([exchange_response("1234567890123456"), profile]);
        let error = super::auth::activate_with(
            f.session.clone(),
            profile_material(),
            Duration::from_secs(2),
            &wire,
        )
        .await
        .expect_err("profile error must not enable account");
        assert!(!error.to_string().contains("synthetic-refresh"));
        assert_eq!(wire.calls.lock().unwrap().len(), 2);
        let views = list(&f.session).await.unwrap();
        assert_eq!(views[0].status, AccountStatus::RefreshPending);
    }
}
struct RetireDuringProfile {
    session: Arc<SecretSession>,
    lock: bool,
    calls: Mutex<Vec<String>>,
}
impl Transport for RetireDuringProfile {
    fn send(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.url.clone());
            if r.url.ends_with("/GetUserInfo") {
                if self.lock {
                    self.session.set_blocked(true);
                } else {
                    let views = list(&self.session).await.unwrap();
                    retire(&self.session, &views[0].id).await.unwrap();
                }
                Ok(profile_response("synthetic-user", "SG"))
            } else {
                Ok(exchange_response("1234567890123456"))
            }
        })
    }
}
#[tokio::test]
async fn fresh_profile_late_result_cannot_resurrect_revoked_or_locked_owner() {
    for lock in [false, true] {
        let f = Fixture::new().await;
        let wire = RetireDuringProfile {
            session: f.session.clone(),
            lock,
            calls: Mutex::new(Vec::new()),
        };
        assert!(super::auth::activate_with(
            f.session.clone(),
            profile_material(),
            Duration::from_secs(2),
            &wire
        )
        .await
        .is_err());
        assert_eq!(wire.calls.lock().unwrap().len(), 2);
        if lock {
            assert!(list(&f.session).await.is_err());
        } else {
            let views = list(&f.session).await.unwrap();
            assert_eq!(views[0].status, AccountStatus::Retired);
        }
    }
}
#[tokio::test]
async fn fresh_profile_unverified_realm_sends_zero_http_and_saves_no_grant() {
    for realm in [Realm::Cn] {
        let f = Fixture::new().await;
        let mut material = profile_material();
        material.account.realm = realm;
        material.account.device.auth_host = realm.auth().into();
        let wire = Http::default();
        wire.responses.lock().unwrap().extend([
            exchange_response("1234567890123456"),
            profile_response("synthetic-user", "SG"),
        ]);
        let result =
            super::auth::activate_with(f.session.clone(), material, Duration::from_secs(2), &wire)
                .await;
        assert!(
            matches!(result,Err(ProxyError::AuthError(ref code)) if code=="trae.desktop_identity_unverified")
        );
        assert!(wire.calls.lock().unwrap().is_empty());
        assert!(list(&f.session).await.unwrap().is_empty());
    }
}
#[tokio::test]
async fn fresh_profile_us_uses_original_grow_host_without_changing_inference_realm() {
    let f = Fixture::new().await;
    let mut material = profile_material();
    material.account.realm = Realm::Us;
    let wire = Http::default();
    wire.responses.lock().unwrap().extend([
        exchange_response("1234567890123456"),
        profile_response("synthetic-user", "US"),
        ok(),
    ]);
    let id = super::auth::activate_with(f.session.clone(), material, Duration::from_secs(2), &wire)
        .await
        .unwrap();
    let response = Lease::acquire(f.session.clone(), &id, "s")
        .await
        .unwrap()
        .dispatch_with(chat(), Duration::from_secs(2), &wire)
        .await
        .unwrap();
    assert!(response.response.status().is_success());
    let calls = wire.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[0].url.starts_with("https://growsg-normal.trae.ai/"));
    assert!(calls[1].url.starts_with("https://growsg-normal.trae.ai/"));
    assert!(calls[2].url.starts_with("https://coreva-normal.trae.ai/"));
}
#[tokio::test]
async fn fresh_profile_missing_or_legacy_original_host_is_never_migrated() {
    let f = Fixture::new().await;
    let mut material = profile_material();
    let mut old = serde_json::to_value(&material.account).unwrap();
    old["device"].as_object_mut().unwrap().remove("auth_host");
    assert!(
        serde_json::from_value::<Account>(old).is_err(),
        "legacy saved material needs explicit original-host reimport"
    );
    material.account.device.auth_host = "https://api-sg-central.trae.ai".into();
    let wire = Http::default();
    assert!(
        super::auth::activate_with(f.session.clone(), material, Duration::from_secs(2), &wire)
            .await
            .is_err()
    );
    assert!(wire.calls.lock().unwrap().is_empty());
    assert!(list(&f.session).await.unwrap().is_empty());
}
