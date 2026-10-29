//! Integration fixtures: only upstream responses are synthetic; all owner/Engine calls are real.
use super::*;
use crate::workbuddy::{
    authorization,
    checkin::ClaimState,
    clock::Clock,
    engine::Engine,
    model::{today, Account, Attempt, AUTO_CLAIM_POLICY_VERSION},
    protocol::{self, DispatchFailure},
};
use std::collections::VecDeque;
const NOW: i64 = 1_800_000_000_000;
struct Fixed(i64);
impl Clock for Fixed {
    fn now_ms(&self) -> i64 {
        self.0
    }
}
struct Upstream {
    replies: Mutex<VecDeque<(Endpoint, u16, Value)>>,
    calls: Mutex<Vec<(Endpoint, String)>>,
}
impl Upstream {
    fn new(replies: Vec<(Endpoint, u16, Value)>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(vec![]),
        }
    }
}
impl Transport for Upstream {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            if request.endpoint == Endpoint::Refresh {
                tokio::task::yield_now().await;
            }
            self.calls.lock().unwrap().push((
                request.endpoint,
                request
                    .headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string(),
            ));
            let (endpoint, status, body) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected upstream request");
            assert!(request.endpoint == endpoint, "wrong upstream endpoint");
            let response = http::Response::builder()
                .status(status)
                .header(
                    "content-type",
                    if endpoint == Endpoint::Chat {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .body(if endpoint == Endpoint::Chat {
                    "data: [DONE]\n\n".into()
                } else {
                    body.to_string()
                })
                .unwrap();
            Ok(ProxyResponse::Reqwest(reqwest::Response::from(response)))
        })
    }
}
impl protocol::Transport for Upstream {
    fn request<'a>(
        &'a self,
        _endpoint: protocol::Endpoint,
        _account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async { panic!("canonical account used legacy transport") })
    }
    fn shared_request<'a>(
        &'a self,
        session: &'a Arc<SecretSession>,
        snapshot: &'a SavedAccount,
        endpoint: protocol::Endpoint,
        policy: Option<(u64, u64)>,
        day: Option<&'a str>,
        clock: &'a dyn Clock,
    ) -> Pin<Box<dyn Future<Output = Result<Value, DispatchFailure>> + Send + 'a>> {
        Box::pin(shared_account::request_with(
            session, snapshot, endpoint, policy, day, clock, self,
        ))
    }
}
struct LoginTransport(Mutex<VecDeque<(protocol::Endpoint, Value)>>);
impl protocol::Transport for LoginTransport {
    fn request<'a>(
        &'a self,
        endpoint: protocol::Endpoint,
        _account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let (expected, value) = self.0.lock().unwrap().pop_front().unwrap();
            assert_eq!(endpoint, expected);
            Ok(value)
        })
    }
}
async fn setup() -> (Arc<SecretSession>, String) {
    setup_domain(Domain::CodeBuddy).await
}
async fn setup_domain(domain: Domain) -> (Arc<SecretSession>, String) {
    let session = SecretSession::ephemeral().unwrap();
    let t=LoginTransport(Mutex::new(vec![
      (protocol::Endpoint::AuthState,json!({"code":0,"data":{"state":"synthetic","authUrl":"https://copilot.tencent.com/login?state=synthetic"}})),
      (protocol::Endpoint::AuthToken,json!({"code":0,"data":{"accessToken":"synthetic-access","refreshToken":"synthetic-refresh","expiresIn":3600,"domain":domain.host()}})),
      (protocol::Endpoint::Profile,json!({"code":0,"data":{"uid":"synthetic-account","domain":domain.host()}})),
    ].into()));
    let login = authorization::begin(&session, &t, NOW).await.unwrap();
    authorization::finish(&session, &t, &login.flow_id, NOW)
        .await
        .unwrap();
    let binding = store::binding(&session).unwrap();
    let rows = store::load(&session, &binding).unwrap();
    (session, rows[0].account.id(binding.store_id()))
}
fn reads(status: bool) -> Vec<(Endpoint, u16, Value)> {
    vec![
        (
            Endpoint::Account(protocol::Endpoint::Summary),
            200,
            json!({"code":0,"data":{"Packages":[{"PackageCode":"p","CycleCapacityRemainPrecise":10.0}]}}),
        ),
        (
            Endpoint::Account(protocol::Endpoint::Paid),
            200,
            json!({"code":0,"data":{"Accounts":[]}}),
        ),
        (
            Endpoint::Account(protocol::Endpoint::Free),
            200,
            json!({"code":0,"data":{"Accounts":[]}}),
        ),
        (
            Endpoint::Account(protocol::Endpoint::Status),
            200,
            json!({"code":0,"data":{"active":true,"today_checked_in":status}}),
        ),
    ]
}
async fn verify_paused(case: &str, domain: Domain) {
    if case == "paused_acp" {
        crate::proxy::providers::workbuddy_acp::pause_fixture::verify().await;
        return;
    }
    let (session, id) = setup_domain(domain).await;
    let binding = store::binding(&session).unwrap();
    // An existing, explicitly enabled pre-pause account, not a newly granted permission.
    if case != "paused_claim" && case != "paused_refresh" {
        let mut rows = store::load(&session, &binding).unwrap();
        rows[0].inference_consent = Some(true);
        store::save(&session, &binding, rows).unwrap();
    }
    let owned = crate::secrets::owned_file::OwnedFile::registered(store::FILE).unwrap();
    let before = std::fs::read(owned.path(&session)).unwrap();
    match case {
        "paused_lease" => {
            let result = Lease::acquire(session.clone(), &id).await;
            assert_eq!(
                serde_json::to_value(result.err()).unwrap(),
                json!("inferencePaused")
            );
        }
        "paused_consent" => {
            let result = set_inference_consent(&session, &id, true).await;
            assert_eq!(
                serde_json::to_value(result.err()).unwrap(),
                json!("inferencePaused")
            );
        }
        "paused_login" => {
            let network = Upstream::new(vec![(
                Endpoint::State,
                200,
                json!({"code":0,"data":{"state":"synthetic","authUrl":"https://copilot.tencent.com/login?state=synthetic"}}),
            )]);
            let result =
                PendingAuthorization::begin_with(session.clone(), &id, &network, NOW).await;
            assert!(
                network.calls.lock().unwrap().is_empty(),
                "paused inference login must not contact upstream"
            );
            assert_eq!(
                serde_json::to_value(result.err()).unwrap(),
                json!("inferencePaused")
            );
        }
        "paused_state" => {
            let rows = list_inference_accounts(&session).await.unwrap();
            assert_eq!(rows[0].id, id);
            assert_eq!(
                serde_json::to_value(rows[0].state).unwrap(),
                json!("paused")
            );
        }
        "paused_final_dispatch" => {
            // Account-purpose grant may refresh/check in, but is never a model capability.
            let lease = Lease::acquire_for_purpose(session.clone(), &id, false)
                .await
                .unwrap();
            let network = Upstream::new(vec![(Endpoint::Chat, 200, json!({}))]);
            let result = lease.dispatch_with(json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"synthetic"}]}), Duration::from_secs(1), &network, NOW).await;
            assert!(
                network.calls.lock().unwrap().is_empty(),
                "paused final dispatch must create no model request"
            );
            assert!(result.is_err());
        }
        "paused_models_before_refresh" => {
            let mut lease = Lease::acquire_for_purpose(session.clone(), &id, false)
                .await
                .unwrap();
            let network = Upstream::new(vec![
                (
                    Endpoint::Refresh,
                    200,
                    json!({"code":0,"data":{"accessToken":"synthetic-rotated","refreshToken":"synthetic-refresh-2","expiresIn":3600}}),
                ),
                (
                    Endpoint::Models,
                    200,
                    json!({"code":0,"data":{"models":[]}}),
                ),
            ]);
            let result = lease.models_with(&network, NOW + 3_600_000).await;
            assert!(
                network.calls.lock().unwrap().is_empty(),
                "paused model discovery must not refresh or query upstream"
            );
            assert!(result.is_err());
        }
        "paused_refresh" => {
            let mut replies = vec![(
                Endpoint::Refresh,
                200,
                json!({"code":0,"data":{"accessToken":"synthetic-rotated","refreshToken":"synthetic-refresh-2","expiresIn":3600}}),
            )];
            replies.extend(reads(false));
            let network = Upstream::new(replies);
            let clock = Fixed(NOW + 3_600_000);
            let view = Engine {
                session: &session,
                transport: &network,
                clock: &clock,
            }
            .refresh(&id)
            .await
            .unwrap();
            assert_eq!(view.credits.total_remaining, Some(10.0));
            let calls = network.calls.lock().unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|(e, _)| *e == Endpoint::Refresh)
                    .count(),
                1
            );
            assert!(calls
                .iter()
                .all(|(e, _)| matches!(e, Endpoint::Refresh | Endpoint::Account(_))));
            assert!(network.replies.lock().unwrap().is_empty());
            return;
        }
        "paused_claim" => {
            let mut replies = vec![reads(false).pop().unwrap()];
            replies.extend(reads(false).into_iter().take(3));
            replies.push((
                Endpoint::Account(protocol::Endpoint::Claim),
                200,
                json!({"code":0,"credit":7}),
            ));
            replies.push(reads(true).pop().unwrap());
            replies.extend(reads(true).into_iter().take(3));
            replies.push(reads(true).pop().unwrap());
            replies.extend(reads(true).into_iter().take(3));
            let network = Upstream::new(replies);
            let clock = Fixed(NOW);
            let engine = Engine {
                session: &session,
                transport: &network,
                clock: &clock,
            };
            engine.claim(&id).await.unwrap();
            engine.claim(&id).await.unwrap();
            let saved = store::load(&session, &binding).unwrap().remove(0);
            assert_eq!(saved.attempted_day, Some(today(NOW)));
            assert!(!saved.inference_allowed());
            let calls = network.calls.lock().unwrap();
            assert_eq!(
                calls
                    .iter()
                    .filter(|(e, _)| *e == Endpoint::Account(protocol::Endpoint::Claim))
                    .count(),
                1
            );
            assert!(calls.iter().all(|(e, _)| matches!(e, Endpoint::Account(_))));
            assert!(network.replies.lock().unwrap().is_empty());
            return;
        }
        _ => panic!("unknown pause case"),
    }
    assert_eq!(
        std::fs::read(owned.path(&session)).unwrap(),
        before,
        "pause must not rewrite account data"
    );
}

pub(crate) async fn verify(case: &str) {
    if let Some(case) = case.strip_prefix("codebuddy_") {
        verify_paused(case, Domain::CodeBuddy).await;
        return;
    }
    if case.starts_with("paused_") {
        verify_paused(case, Domain::WorkBuddy).await;
        return;
    }
    assert!(matches!(
        case,
        "consent"
            | "refresh"
            | "pending"
            | "legacy"
            | "checkin_refusal"
            | "claim"
            | "schema"
            | "final_fence"
            | "late_revoke"
            | "wrong_identity"
    ));
    let (session, id) = setup().await;
    let binding = store::binding(&session).unwrap();
    if case == "schema" {
        let mut canonical = store::load(&session, &binding).unwrap();
        let mut legacy = SavedAccount::new(Account {
            uid: "synthetic-account".into(),
            domain: Domain::CodeBuddy,
            label: "Synthetic legacy".into(),
            credential: "synthetic-old-token".into(),
        });
        legacy.attempted_day = Some(today(NOW));
        legacy.pending = Some(Attempt {
            day: today(NOW),
            before: Some(10.0),
            expected: None,
            receipt: ClaimState::Unconfirmed,
        });
        let expected_pending = legacy.pending.clone();
        let old_document = json!({"accounts":[legacy]});
        for key in [
            "shared_authorization",
            "inference_consent",
            "inference_revision",
        ] {
            assert!(old_document["accounts"][0].get(key).is_none());
        }
        let owned = crate::secrets::owned_file::OwnedFile::registered(store::FILE).unwrap();
        let old_ciphertext = {
            let vault = session.read().unwrap();
            owned
                .encode_storage(&vault, &serde_json::to_vec(&old_document).unwrap())
                .unwrap()
        };
        crate::secrets::session::write_durable(&owned.path(&session), &old_ciphertext).unwrap();
        let loaded = store::load(&session, &binding).unwrap();
        assert!(!loaded[0].shared_authorization);
        assert!(loaded[0].pending == expected_pending);
        assert_eq!(
            loaded[0].account.api_token().unwrap(),
            "synthetic-old-token"
        );
        canonical[0].pending = expected_pending.clone();
        canonical[0].attempted_day = Some(today(NOW));
        store::save(&session, &binding, canonical).unwrap();
        assert!(store::load(&session, &binding).unwrap()[0].shared_authorization);
        // Same original owner/key can restore the exact pre-upgrade encrypted file.
        // This does NOT authorize rolling back newer pending/attempted_day records.
        crate::secrets::session::write_durable(&owned.path(&session), &old_ciphertext).unwrap();
        let restored = store::load(&session, &binding).unwrap();
        assert!(restored[0].pending == expected_pending);
        assert_eq!(restored[0].attempted_day, Some(today(NOW)));
        assert_eq!(
            restored[0].account.api_token().unwrap(),
            "synthetic-old-token"
        );
        assert_eq!(std::fs::read(owned.path(&session)).unwrap(), old_ciphertext);
        let wrong_vault = crate::secrets::VaultContext::generate().unwrap();
        assert!(owned.decode(&wrong_vault, &old_ciphertext).is_err());
        return;
    }
    if case == "final_fence" {
        struct Midnight(std::sync::atomic::AtomicUsize);
        impl Clock for Midnight {
            fn now_ms(&self) -> i64 {
                if self.0.fetch_add(1, Ordering::SeqCst) < 2 {
                    NOW
                } else {
                    NOW + 86_400_000
                }
            }
        }
        let mut rows = store::load(&session, &binding).unwrap();
        rows[0].pending = Some(Attempt {
            day: today(NOW),
            before: None,
            expected: None,
            receipt: ClaimState::Unconfirmed,
        });
        let snapshot = rows[0].clone();
        store::save(&session, &binding, rows).unwrap();
        let network = Upstream::new(vec![]);
        let result = shared_account::request_with(
            &session,
            &snapshot,
            protocol::Endpoint::Claim,
            None,
            Some(&today(NOW)),
            &Midnight(std::sync::atomic::AtomicUsize::new(0)),
            &network,
        )
        .await;
        assert!(matches!(result, Err(DispatchFailure::NotDispatched(_))));
        assert!(
            network.calls.lock().unwrap().is_empty(),
            "final fence must refuse before transport first poll"
        );
        return;
    }
    if case == "late_revoke" || case == "wrong_identity" {
        struct RevokeOnProfile {
            inner: Upstream,
            session: Arc<SecretSession>,
            id: String,
            revoke: bool,
        }
        impl Transport for RevokeOnProfile {
            fn request(
                &self,
                request: Request,
            ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>>
            {
                Box::pin(async move {
                    if request.endpoint == Endpoint::Profile && self.revoke {
                        set_inference_consent(&self.session, &self.id, false)
                            .await
                            .unwrap();
                    }
                    Transport::request(&self.inner, request).await
                })
            }
        }
        let before = store::load(&session, &binding).unwrap().remove(0);
        let network = RevokeOnProfile {
            session: session.clone(),
            id: id.clone(),
            revoke: case == "late_revoke",
            inner: Upstream::new(vec![
                (
                    Endpoint::State,
                    200,
                    json!({"code":0,"data":{"state":"synthetic-new","authUrl":"https://copilot.tencent.com/login?state=synthetic-new"}}),
                ),
                (
                    Endpoint::Token,
                    200,
                    json!({"code":0,"data":{"accessToken":"synthetic-new-access","refreshToken":"synthetic-new-refresh","expiresIn":3600,"domain":"www.codebuddy.cn"}}),
                ),
                (
                    Endpoint::Profile,
                    200,
                    json!({"code":0,"data":{"uid":if case == "wrong_identity" {"different-account"} else {"synthetic-account"},"domain":"www.codebuddy.cn"}}),
                ),
            ]),
        };
        let mut pending =
            PendingAuthorization::begin_protocol_with(session.clone(), &id, &network, NOW)
                .await
                .unwrap();
        assert!(pending.poll_protocol_with(&network, NOW + 1).await.is_err());
        let after = store::load(&session, &binding).unwrap().remove(0);
        assert!(!after.inference_allowed());
        assert_eq!(
            after.inference.as_ref().unwrap().grant_id,
            before.inference.as_ref().unwrap().grant_id
        );
        assert_eq!(after.generation, before.generation);
        return;
    }
    if case == "legacy" {
        let mut rows = store::load(&session, &binding).unwrap();
        rows[0].shared_authorization = false;
        rows[0].inference = None;
        rows[0].account.credential = "synthetic-legacy-access".into();
        store::save(&session, &binding, rows).unwrap();
        assert_eq!(
            set_inference_consent(&session, &id, true).await,
            Err(Failure::InferencePaused)
        );
        let rows = store::load(&session, &binding).unwrap();
        assert!(!rows[0].shared_authorization);
        assert_eq!(
            rows[0].account.api_token().unwrap(),
            "synthetic-legacy-access"
        );
        return;
    }
    if case == "consent" {
        // Historical enabled consent, not a new model grant through public API.
        let mut rows = store::load(&session, &binding).unwrap();
        rows[0].inference_consent = Some(true);
        store::save(&session, &binding, rows).unwrap();
        assert_eq!(
            set_inference_consent(&session, &id, true).await,
            Err(Failure::InferencePaused)
        );
        let lease = Lease::acquire_with_expected(session.clone(), &id, true, None)
            .await
            .unwrap();
        let network = Upstream::new(reads(false));
        let clock = Fixed(NOW);
        let engine = Engine {
            session: &session,
            transport: &network,
            clock: &clock,
        };
        let row = engine.list().await.unwrap().remove(0);
        engine
            .set_automatic(
                &id,
                row.generation,
                row.auto_claim_revision,
                AUTO_CLAIM_POLICY_VERSION,
                true,
            )
            .await
            .unwrap();
        let before = store::load(&session, &binding).unwrap().remove(0);
        assert!(set_inference_consent(&session, &id, false).await.unwrap());
        assert!(lease.check().await.is_err());
        let after = engine.refresh(&id).await.unwrap();
        assert_eq!(after.credits.total_remaining, Some(10.0));
        assert!(after.auto_claim_enabled);
        let persisted = store::load(&session, &binding).unwrap().remove(0);
        assert_eq!(persisted.auto_claim_revision, before.auto_claim_revision);
        assert!(persisted.inference.is_some());
        assert!(network
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, auth)| auth == "Bearer synthetic-access"));
        assert!(network.replies.lock().unwrap().is_empty());
        return;
    }
    if case == "refresh" {
        let mut lease = Lease::acquire_for_purpose(session.clone(), &id, false)
            .await
            .unwrap();
        let mut account_lease = Lease::acquire_for_purpose(session.clone(), &id, false)
            .await
            .unwrap();
        let mut replies = vec![(
            Endpoint::Refresh,
            200,
            json!({"code":0,"data":{"accessToken":"synthetic-rotated","refreshToken":"synthetic-refresh-2","expiresIn":3600}}),
        )];
        replies.extend(reads(false));
        let network = Upstream::new(replies);
        let clock = Fixed(NOW + 3_600_000);
        // Both consumers capture the old credential. The upstream yields so the
        // second consumer waits on the real owner gate, not a synthetic counter.
        let (account_result, second_account_result) = tokio::join!(
            account_lease.refresh(&network, clock.now_ms()),
            lease.refresh(&network, clock.now_ms())
        );
        account_result.unwrap();
        second_account_result.unwrap();
        Engine {
            session: &session,
            transport: &network,
            clock: &clock,
        }
        .refresh(&id)
        .await
        .unwrap();
        assert_eq!(lease.grant.access, "synthetic-rotated");
        let dispatched = lease.dispatch_with(json!({"model":"synthetic-model","stream":true,"messages":[{"role":"user","content":"synthetic"}]}), Duration::from_secs(1), &network, clock.now_ms()).await;
        assert!(
            matches!(dispatched, Err(ProxyError::InvalidRequest(reason)) if reason == "workbuddy.inference_paused")
        );
        assert!(network
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(e, _)| *e != Endpoint::Chat));
        assert_eq!(
            network
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(e, _)| *e == Endpoint::Refresh)
                .count(),
            1
        );
        assert!(network
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| *e != Endpoint::Refresh)
            .all(|(_, auth)| auth == "Bearer synthetic-rotated"));
        assert!(network.replies.lock().unwrap().is_empty());
        return;
    }
    if case == "claim" {
        let mut replies = vec![reads(false).pop().unwrap()];
        replies.extend(reads(false).into_iter().take(3));
        replies.push((
            Endpoint::Account(protocol::Endpoint::Claim),
            200,
            json!({"code":0,"credit":7}),
        ));
        replies.push(reads(true).pop().unwrap());
        replies.extend(reads(true).into_iter().take(3));
        // A repeated click can reconcile/read, but must not dispatch another claim.
        replies.push(reads(true).pop().unwrap());
        replies.extend(reads(true).into_iter().take(3));
        let network = Upstream::new(replies);
        let clock = Fixed(NOW);
        let engine = Engine {
            session: &session,
            transport: &network,
            clock: &clock,
        };
        engine.claim(&id).await.unwrap();
        engine.claim(&id).await.unwrap();
        let saved = store::load(&session, &binding).unwrap().remove(0);
        assert_eq!(saved.attempted_day, Some(today(NOW)));
        assert!(!saved.inference_allowed());
        assert_eq!(
            network
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(e, _)| *e == Endpoint::Account(protocol::Endpoint::Claim))
                .count(),
            1
        );
        assert!(network
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, auth)| auth == "Bearer synthetic-access"));
        return;
    }
    if case == "pending" {
        let mut rows = store::load(&session, &binding).unwrap();
        rows[0].pending = Some(Attempt {
            day: today(NOW),
            before: Some(10.0),
            expected: None,
            receipt: ClaimState::Unconfirmed,
        });
        rows[0].attempted_day = Some(today(NOW));
        store::save(&session, &binding, rows).unwrap();
        assert_eq!(
            set_inference_consent(&session, &id, true).await,
            Err(Failure::InferencePaused)
        );
        let mut replies = vec![reads(false).pop().unwrap()];
        replies.extend(reads(false).into_iter().take(3));
        let network = Upstream::new(replies);
        let clock = Fixed(NOW);
        Engine {
            session: &session,
            transport: &network,
            clock: &clock,
        }
        .claim(&id)
        .await
        .unwrap();
        let row = store::load(&session, &binding).unwrap().remove(0);
        assert!(row.pending.is_some());
        assert_eq!(row.attempted_day, Some(today(NOW)));
        assert!(!network
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(e, _)| *e == Endpoint::Account(protocol::Endpoint::Claim)));
        return;
    }
    if case == "checkin_refusal" {
        let network = Upstream::new(vec![
            (
                Endpoint::Account(protocol::Endpoint::Status),
                401,
                json!({}),
            ),
            reads(false).remove(0),
        ]);
        let clock = Fixed(NOW);
        let view = Engine {
            session: &session,
            transport: &network,
            clock: &clock,
        }
        .claim(&id)
        .await
        .unwrap();
        assert_eq!(view.claim_state, ClaimState::CheckinRestricted);
        let row = store::load(&session, &binding).unwrap().remove(0);
        assert!(row.inference.is_some());
        assert!(row.pending.is_none());
        assert!(network.replies.lock().unwrap().is_empty());
    }
}
