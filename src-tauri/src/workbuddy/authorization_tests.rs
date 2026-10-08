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
