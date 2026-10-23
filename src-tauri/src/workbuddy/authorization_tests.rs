use super::{authorization::*, model::*, protocol::*, store};
use crate::secrets::session::SecretSession;
use serde_json::{json, Value};
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Mutex};
fn token() -> Value {
    json!({"code":0,"data":{"accessToken":"SYNTHETIC_TOKEN_CANARY","domain":"www.codebuddy.cn"}})
}
fn profile() -> Value {
    json!({"code":0,"data":{"uid":"synthetic-account","nickname":"Synthetic","domain":"www.codebuddy.cn"}})
}
#[test]
fn official_account_requires_uid_token_and_matching_cn_domain() {
    assert!(parse_account(&token(), &profile()).is_ok());
    for t in [
        json!({"code":401,"data":{"accessToken":"secret"}}),
        json!({"code":0,"data":{"accessToken":{"$wbEncrypted":1,"envelope":"opaque"},"domain":"www.codebuddy.cn"}}),
        json!({"code":0,"data":{"accessToken":"secret","domain":"www.workbuddy.ai"}}),
    ] {
        assert!(parse_account(&t, &profile()).is_err());
    }
    for p in [
        json!({"code":0,"data":{"nickname":"Synthetic"}}),
        json!({"code":0,"data":{"uid":"synthetic-account","enterpriseId":"tenant-a"}}),
        json!({"code":0,"data":{"uid":"synthetic-account","enterpriseId":42}}),
        json!({"code":0,"data":{"uid":"synthetic-account","type":{}}}),
        json!({"code":0,"data":{"uid":"synthetic-account","domain":"www.workbuddy.cn"}}),
    ] {
        assert!(parse_account(&token(), &p).is_err());
    }
}
#[test]
fn authorization_url_is_bound_to_state_and_exact_official_origin() {
    assert!(verification_url(
        "https://www.codebuddy.cn/login?state=synthetic",
        "synthetic"
    )
    .is_ok());
    for url in [
        "http://www.codebuddy.cn/login?state=synthetic",
        "https://www.codebuddy.cn.evil.example/login?state=synthetic",
        "https://www.codebuddy.cn/login?state=other",
        "https://user:password@www.codebuddy.cn/login?state=synthetic",
        "https://www.workbuddy.ai/login?state=synthetic",
        "https://www.codebuddy.cn/login?state=synthetic&token=secret",
    ] {
        assert!(verification_url(url, "synthetic").is_err(), "{url}");
    }
}
struct Fake(Mutex<VecDeque<(Endpoint, Value)>>);
impl Transport for Fake {
    fn request<'a>(
        &'a self,
        e: Endpoint,
        _a: Option<&'a Account>,
        _s: Option<&'a str>,
        _n: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let (expected, v) = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected auth effect");
            assert_eq!(e, expected);
            Ok(v)
        })
    }
}
#[tokio::test]
async fn only_explicit_official_authorization_saves_account_without_reward_side_effects() {
    let session = SecretSession::ephemeral().unwrap();
    let transport=Fake(Mutex::new(vec![(Endpoint::AuthState,json!({"code":0,"data":{"state":"synthetic","authUrl":"https://www.codebuddy.cn/login?state=synthetic"}})),(Endpoint::AuthToken,token()),(Endpoint::Profile,profile())].into()));
    let login = begin(&session, &transport, 1_800_000_000_000)
        .await
        .unwrap();
    assert!(store::load(&session, &store::binding(&session).unwrap())
        .unwrap()
        .is_empty());
    let result = finish(&session, &transport, &login.flow_id, 1_800_000_000_001)
        .await
        .unwrap();
    assert_eq!(result.state, AuthorizationState::Saved);
    let rows = store::load(&session, &store::binding(&session).unwrap()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account.uid, "synthetic-account");
    assert_eq!(rows[0].credits.total_remaining, None);
    assert!(transport.0.lock().unwrap().is_empty());
    assert!(!serde_json::to_string(&login)
        .unwrap()
        .contains("SYNTHETIC_TOKEN_CANARY"));
    let bytes = std::fs::read(session.root().join(store::FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("SYNTHETIC_TOKEN_CANARY"));
}
#[tokio::test]
async fn authorization_flow_cannot_cross_owner_or_survive_expiry() {
    let a = SecretSession::ephemeral().unwrap();
    let b = SecretSession::ephemeral().unwrap();
    let f=Fake(Mutex::new(vec![(Endpoint::AuthState,json!({"code":0,"data":{"state":"synthetic","authUrl":"https://www.codebuddy.cn/login?state=synthetic"}}))].into()));
    let login = begin(&a, &f, 1_800_000_000_000).await.unwrap();
    assert!(finish(&b, &f, &login.flow_id, 1_800_000_000_001)
        .await
        .is_err());
    assert!(finish(&a, &f, &login.flow_id, 1_800_000_601_000)
        .await
        .is_err());
    assert!(f.0.lock().unwrap().is_empty());
}

const LOGIN_NOW: i64 = 1_800_000_000_000;

fn auth_state() -> Value {
    json!({"code":0,"data":{"state":"synthetic","authUrl":"https://www.codebuddy.cn/login?state=synthetic"}})
}

#[tokio::test]
async fn official_pending_codes_wait_then_save_only_after_verified_profile() {
    for as_text in [false, true] {
        let session = SecretSession::ephemeral().unwrap();
        let pending = |code: i64| {
            if as_text {
                json!({"code":code.to_string()})
            } else {
                json!({"code":code})
            }
        };
        let transport = Fake(Mutex::new(
            vec![
                (Endpoint::AuthState, auth_state()),
                (Endpoint::AuthToken, pending(11217)),
                (Endpoint::AuthToken, token()),
                (Endpoint::Profile, pending(12151)),
                (Endpoint::AuthToken, token()),
                (Endpoint::Profile, profile()),
            ]
            .into(),
        ));
        let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
        for offset in [1, 2] {
            let result = finish(&session, &transport, &login.flow_id, LOGIN_NOW + offset)
                .await
                .unwrap();
            assert_eq!(result.state, AuthorizationState::Waiting);
            assert!(store::load(&session, &store::binding(&session).unwrap())
                .unwrap()
                .is_empty());
        }
        let result = finish(&session, &transport, &login.flow_id, LOGIN_NOW + 3)
            .await
            .unwrap();
        assert_eq!(result.state, AuthorizationState::Saved);
        let rows = store::load(&session, &store::binding(&session).unwrap()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account.uid, "synthetic-account");
        assert!(rows[0].inference.is_none());
        assert!(transport.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn official_pending_codes_are_stage_specific_and_do_not_hide_rejections() {
    for code in [12151, 401, 403, 10085, 11004, 11140, 99999] {
        let session = SecretSession::ephemeral().unwrap();
        let transport = Fake(Mutex::new(
            vec![
                (Endpoint::AuthState, auth_state()),
                (Endpoint::AuthToken, json!({"code":code})),
            ]
            .into(),
        ));
        let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
        assert!(matches!(
            finish(&session, &transport, &login.flow_id, LOGIN_NOW + 1).await,
            Err(Failure::NeedsVerification)
        ));
        assert!(transport.0.lock().unwrap().is_empty());
        assert!(store::load(&session, &store::binding(&session).unwrap())
            .unwrap()
            .is_empty());
        cancel(&session, &login.flow_id).unwrap();
    }
    for code in [11217, 401, 403, 10085, 11004, 11140, 99999] {
        let session = SecretSession::ephemeral().unwrap();
        let transport = Fake(Mutex::new(
            vec![
                (Endpoint::AuthState, auth_state()),
                (Endpoint::AuthToken, token()),
                (Endpoint::Profile, json!({"code":code})),
            ]
            .into(),
        ));
        let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
        assert!(matches!(
            finish(&session, &transport, &login.flow_id, LOGIN_NOW + 1).await,
            Err(Failure::NeedsVerification)
        ));
        assert!(transport.0.lock().unwrap().is_empty());
        assert!(store::load(&session, &store::binding(&session).unwrap())
            .unwrap()
            .is_empty());
        cancel(&session, &login.flow_id).unwrap();
    }
}

#[tokio::test]
async fn pending_official_authorization_retains_cancel_expiry_and_supersession_fences() {
    for at_profile in [false, true] {
        for invalidation in ["cancel", "expiry", "supersession", "lock"] {
            let session = SecretSession::ephemeral().unwrap();
            let mut replies = vec![(Endpoint::AuthState, auth_state())];
            if at_profile {
                replies.push((Endpoint::AuthToken, token()));
                replies.push((Endpoint::Profile, json!({"code":12151})));
            } else {
                replies.push((Endpoint::AuthToken, json!({"code":11217})));
            }
            if invalidation == "supersession" {
                replies.push((Endpoint::AuthState, auth_state()));
            }
            let transport = Fake(Mutex::new(replies.into()));
            let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
            assert_eq!(
                finish(&session, &transport, &login.flow_id, LOGIN_NOW + 1)
                    .await
                    .unwrap()
                    .state,
                AuthorizationState::Waiting
            );
            let mut next_now = LOGIN_NOW + 2;
            match invalidation {
                "cancel" => cancel(&session, &login.flow_id).unwrap(),
                // begin() grants this existing ten-minute window, unlike the
                // separate inference authorization's five-minute window.
                "expiry" => next_now = LOGIN_NOW + 600_000,
                "supersession" => {
                    let next = begin(&session, &transport, LOGIN_NOW + 2).await.unwrap();
                    cancel(&session, &next.flow_id).unwrap();
                }
                "lock" => {
                    session.set_blocked(true);
                    session.set_blocked(false);
                }
                _ => unreachable!(),
            }
            assert!(matches!(
                finish(&session, &transport, &login.flow_id, next_now).await,
                Err(Failure::ExpiredAuthorization)
            ));
            assert!(transport.0.lock().unwrap().is_empty());
            assert!(store::load(&session, &store::binding(&session).unwrap())
                .unwrap()
                .is_empty());
        }
    }
}

#[tokio::test]
async fn cancelled_or_superseded_login_cannot_save_and_other_owner_cannot_cancel() {
    let session = SecretSession::ephemeral().unwrap();
    let other = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(
        vec![
            (Endpoint::AuthState, auth_state()),
            (Endpoint::AuthState, auth_state()),
        ]
        .into(),
    ));
    let old = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    assert_eq!(
        cancel(&other, &old.flow_id),
        Err(Failure::UnsupportedContext)
    );
    let new = begin(&session, &transport, LOGIN_NOW + 1).await.unwrap();
    assert!(matches!(
        finish(&session, &transport, &old.flow_id, LOGIN_NOW + 2).await,
        Err(Failure::ExpiredAuthorization)
    ));
    cancel(&session, &old.flow_id).unwrap();
    cancel(&session, &new.flow_id).unwrap();
    cancel(&session, &new.flow_id).unwrap();
    assert!(matches!(
        finish(&session, &transport, &new.flow_id, LOGIN_NOW + 3).await,
        Err(Failure::ExpiredAuthorization)
    ));
    assert!(store::load(&session, &store::binding(&session).unwrap())
        .unwrap()
        .is_empty());
    assert!(transport.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_during_profile_prevents_late_persistence() {
    let session = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(vec![(Endpoint::AuthState, auth_state())].into()));
    let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    let paused = std::sync::Arc::new(PausedProfile {
        arrived: Default::default(),
        release: Default::default(),
    });
    let finishing = {
        let session = session.clone();
        let paused = paused.clone();
        let id = login.flow_id.clone();
        tokio::spawn(async move { finish(&session, &*paused, &id, LOGIN_NOW + 1).await })
    };
    paused.arrived.notified().await;
    cancel(&session, &login.flow_id).unwrap();
    paused.release.notify_one();
    assert!(matches!(
        finishing.await.unwrap(),
        Err(Failure::ExpiredAuthorization)
    ));
    assert!(store::load(&session, &store::binding(&session).unwrap())
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn login_cannot_survive_session_epoch_retirement() {
    let session = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(vec![(Endpoint::AuthState, auth_state())].into()));
    let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    session.set_blocked(true);
    session.set_blocked(false);
    assert!(matches!(
        finish(&session, &transport, &login.flow_id, LOGIN_NOW + 1).await,
        Err(Failure::ExpiredAuthorization)
    ));
    cancel(&session, &login.flow_id).unwrap();
    assert!(transport.0.lock().unwrap().is_empty());
}

struct PausedProfile {
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Transport for PausedProfile {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        _account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            match endpoint {
                Endpoint::AuthToken => Ok(token()),
                Endpoint::Profile => {
                    self.arrived.notify_one();
                    self.release.notified().await;
                    Ok(profile())
                }
                _ => panic!("unexpected authorization effect"),
            }
        })
    }
}
#[tokio::test]
async fn second_begin_completes_while_retired_finish_is_still_awaiting_profile() {
    let session = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(
        vec![
            (Endpoint::AuthState, auth_state()),
            (Endpoint::AuthState, auth_state()),
        ]
        .into(),
    ));
    let old = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    let paused = std::sync::Arc::new(PausedProfile {
        arrived: Default::default(),
        release: Default::default(),
    });
    let finishing = {
        let session = session.clone();
        let paused = paused.clone();
        tokio::spawn(async move { finish(&session, &*paused, &old.flow_id, LOGIN_NOW + 1).await })
    };
    paused.arrived.notified().await;
    let next = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        begin(&session, &transport, LOGIN_NOW + 2),
    )
    .await
    .expect("new login must not wait for the retired provider response")
    .unwrap();
    paused.release.notify_one();
    assert!(matches!(
        finishing.await.unwrap(),
        Err(Failure::ExpiredAuthorization)
    ));
    assert!(store::load(&session, &store::binding(&session).unwrap())
        .unwrap()
        .is_empty());
    cancel(&session, &next.flow_id).unwrap();
    assert!(transport.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_after_saved_login_never_deletes_the_account() {
    let session = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(
        vec![
            (Endpoint::AuthState, auth_state()),
            (Endpoint::AuthToken, token()),
            (Endpoint::Profile, profile()),
        ]
        .into(),
    ));
    let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    assert_eq!(
        finish(&session, &transport, &login.flow_id, LOGIN_NOW + 1)
            .await
            .unwrap()
            .state,
        AuthorizationState::Saved
    );
    cancel(&session, &login.flow_id).unwrap();
    let rows = store::load(&session, &store::binding(&session).unwrap()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account.uid, "synthetic-account");
}

#[tokio::test]
async fn cancel_releases_owner_and_duplicate_finish_remains_single_flight() {
    let session = SecretSession::ephemeral().unwrap();
    let other = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(
        vec![
            (Endpoint::AuthState, auth_state()),
            (Endpoint::AuthState, auth_state()),
        ]
        .into(),
    ));
    let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    let paused = std::sync::Arc::new(PausedProfile {
        arrived: Default::default(),
        release: Default::default(),
    });
    let finishing = {
        let session = session.clone();
        let paused = paused.clone();
        let id = login.flow_id.clone();
        tokio::spawn(async move { finish(&session, &*paused, &id, LOGIN_NOW + 1).await })
    };
    paused.arrived.notified().await;
    assert!(matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            finish(&session, &transport, &login.flow_id, LOGIN_NOW + 1)
        )
        .await
        .unwrap(),
        Err(Failure::Busy)
    ));
    cancel(&session, &login.flow_id).unwrap();
    let other_login = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        begin(&other, &transport, LOGIN_NOW + 2),
    )
    .await
    .expect("another owner must not wait for cancelled HTTP")
    .unwrap();
    paused.release.notify_one();
    assert!(matches!(
        finishing.await.unwrap(),
        Err(Failure::ExpiredAuthorization)
    ));
    assert!(store::load(&session, &store::binding(&session).unwrap())
        .unwrap()
        .is_empty());
    cancel(&other, &other_login.flow_id).unwrap();
    assert!(transport.0.lock().unwrap().is_empty());
}

struct PausedToken {
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Transport for PausedToken {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        _account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            assert_eq!(
                endpoint,
                Endpoint::AuthToken,
                "cancelled login must not dispatch a new profile request"
            );
            self.arrived.notify_one();
            self.release.notified().await;
            Ok(token())
        })
    }
}
#[tokio::test]
async fn cancelling_pending_token_never_dispatches_profile_with_late_credential() {
    let session = SecretSession::ephemeral().unwrap();
    let transport = Fake(Mutex::new(vec![(Endpoint::AuthState, auth_state())].into()));
    let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
    let paused = std::sync::Arc::new(PausedToken {
        arrived: Default::default(),
        release: Default::default(),
    });
    let finishing = {
        let session = session.clone();
        let paused = paused.clone();
        let id = login.flow_id.clone();
        tokio::spawn(async move { finish(&session, &*paused, &id, LOGIN_NOW + 1).await })
    };
    paused.arrived.notified().await;
    cancel(&session, &login.flow_id).unwrap();
    paused.release.notify_one();
    assert!(matches!(
        finishing.await.unwrap(),
        Err(Failure::ExpiredAuthorization)
    ));
    assert!(store::load(&session, &store::binding(&session).unwrap())
        .unwrap()
        .is_empty());
}

struct PausedOfficialPending {
    endpoint: Endpoint,
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Transport for PausedOfficialPending {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        _account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            if endpoint == Endpoint::AuthToken && self.endpoint == Endpoint::Profile {
                return Ok(token());
            }
            assert_eq!(endpoint, self.endpoint);
            self.arrived.notify_one();
            self.release.notified().await;
            Ok(json!({"code":if endpoint == Endpoint::AuthToken {11217} else {12151}}))
        })
    }
}

#[tokio::test]
async fn late_official_pending_cannot_revive_cancelled_or_retired_flow() {
    for endpoint in [Endpoint::AuthToken, Endpoint::Profile] {
        for retire in [false, true] {
            let session = SecretSession::ephemeral().unwrap();
            let transport = Fake(Mutex::new(vec![(Endpoint::AuthState, auth_state())].into()));
            let login = begin(&session, &transport, LOGIN_NOW).await.unwrap();
            let paused = std::sync::Arc::new(PausedOfficialPending {
                endpoint,
                arrived: Default::default(),
                release: Default::default(),
            });
            let finishing = {
                let session = session.clone();
                let paused = paused.clone();
                let id = login.flow_id.clone();
                tokio::spawn(async move { finish(&session, &*paused, &id, LOGIN_NOW + 1).await })
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), paused.arrived.notified())
                .await
                .unwrap();
            if retire {
                session.set_blocked(true);
                session.set_blocked(false);
            } else {
                cancel(&session, &login.flow_id).unwrap();
            }
            paused.release.notify_one();
            assert!(matches!(
                tokio::time::timeout(std::time::Duration::from_secs(5), finishing)
                    .await
                    .unwrap()
                    .unwrap(),
                Err(Failure::ExpiredAuthorization)
            ));
            assert!(store::load(&session, &store::binding(&session).unwrap())
                .unwrap()
                .is_empty());
            cancel(&session, &login.flow_id).unwrap();
        }
    }
}
