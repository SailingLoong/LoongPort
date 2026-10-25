//! Synthetic upstream responses only. Real authorization, encrypted store and lease.
//! Excluded from ordinary release builds; no runtime request can select this transport.
use super::{authorization, inference::Lease, model::*, protocol::*, store};
use crate::secrets::session::SecretSession;
use serde_json::{json, Value};
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Mutex};
struct Upstream(Mutex<VecDeque<(Endpoint, Value)>>);
impl Transport for Upstream {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        _account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let (expected, response) = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected upstream request");
            assert_eq!(endpoint, expected);
            Ok(response)
        })
    }
}
pub(crate) async fn verify(case: &str) {
    if !matches!(case, "one_login" | "incomplete_login") {
        super::inference::shared_account_fixture::verify(case).await;
        return;
    }
    let session = SecretSession::ephemeral().unwrap();
    let now = 1_800_000_000_000;
    let mut token = json!({"code":0,"data":{"accessToken":"synthetic-shared-access","domain":"www.codebuddy.cn"}});
    if case == "one_login" {
        token["data"]["refreshToken"] = json!("synthetic-shared-refresh");
        token["data"]["expiresIn"] = json!(3600);
    }
    let upstream = Upstream(Mutex::new(vec![
        (Endpoint::AuthState, json!({"code":0,"data":{"state":"synthetic","authUrl":"https://www.codebuddy.cn/login?state=synthetic"}})),
        (Endpoint::AuthToken, token),
        (Endpoint::Profile, json!({"code":0,"data":{"uid":"synthetic-account","nickname":"Synthetic","domain":"www.codebuddy.cn"}})),
    ].into()));
    let login = authorization::begin(&session, &upstream, now)
        .await
        .unwrap();
    let result = authorization::finish(&session, &upstream, &login.flow_id, now + 1).await;
    let binding = store::binding(&session).unwrap();
    let rows = store::load(&session, &binding).unwrap();
    if case == "incomplete_login" {
        assert!(
            result.is_err(),
            "missing refresh must not be silently promoted to complete credentials"
        );
        assert!(rows.is_empty());
        return;
    }
    result.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].inference.is_some(),
        "one login must retain the complete shared credential instead of discarding refresh/expiry"
    );
    assert!(
        rows[0].account.token.is_empty(),
        "no duplicate access token in the legacy credential slot"
    );
    assert!(!rows[0].automatic_enabled());
    assert!(
        Lease::acquire(session.clone(), &rows[0].account.id(&binding.vault_id))
            .await
            .is_err(),
        "login alone must not opt into inference"
    );
    assert!(upstream.0.lock().unwrap().is_empty());
}
