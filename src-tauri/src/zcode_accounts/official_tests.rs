use super::*;
use std::sync::Mutex;

struct Fixture {
    requests: Mutex<Vec<OfficialRequest>>,
    responses: Mutex<Vec<OfficialResponse>>,
}
impl Fixture {
    fn new(bodies: &[&str]) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(
                bodies
                    .iter()
                    .rev()
                    .map(|s| OfficialResponse {
                        status: 200,
                        body: Zeroizing::new(s.as_bytes().to_vec()),
                    })
                    .collect(),
            ),
        }
    }
}
impl OfficialTransport for Fixture {
    fn send(&self, request: OfficialRequest) -> TransportFuture<'_> {
        self.requests.lock().unwrap().push(request);
        Box::pin(async {
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or(OfficialError::Transport)
        })
    }
}
fn run<T>(future: impl Future<Output = T>) -> T {
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: std::sync::Arc<Self>) {}
    }
    let waker = Waker::from(std::sync::Arc::new(Noop));
    match std::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("synthetic transport unexpectedly blocked"),
    }
}
fn check() -> Result<(), OfficialError> {
    Ok(())
}

#[test]
fn cli_init_uses_official_endpoint_and_bearer_poll_token() {
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":0,"data":{"authorize_url":"https://zcode.z.ai/authorize?flow=test","flow_id":"synthetic-flow","expires_at":1234567,"poll_interval_sec":2}}"#,
    ]));
    let result = run(client.init(
        OAuthFamily::Zai,
        &PollToken::new("synthetic-poll-token").unwrap(),
        &check,
    ))
    .unwrap();
    assert_eq!(result.flow_id, "synthetic-flow");
    assert_eq!(result.expires_at, 1234567);
    let requests = client.transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, "https://zcode.z.ai/api/v1/oauth/cli/init");
    assert_eq!(requests[0].method, OfficialMethod::Post);
    assert_eq!(
        requests[0].authorization.as_ref().unwrap().expose(),
        "Bearer synthetic-poll-token"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(requests[0].body.as_ref().unwrap()).unwrap(),
        serde_json::json!({"provider":"zai"})
    );
}

fn project() -> PersonalProject {
    PersonalProject {
        organization_id: "org-a".into(),
        project_id: "personal-a".into(),
        organization_name: None,
        project_name: None,
    }
}
fn biz(family: OAuthFamily) -> BusinessToken {
    BusinessToken::from_stored(family, "synthetic-business-token").unwrap()
}

#[test]
fn poll_ready_keeps_start_jwt_provider_token_and_identity_separate() {
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":0,"data":{"status":"ready","token":"synthetic-start-jwt","user":{"user_id":"user-a","name":"Sample"},"zai":{"access_token":"synthetic-oauth-token","refreshToken":"synthetic-refresh"},"bigmodel":{"access_token":"wrong-family-token"}}}"#,
    ]));
    let OAuthPoll::Ready(ready) = run(client.poll(
        OAuthFamily::Zai,
        "flow/with?special#chars",
        &PollToken::new("synthetic-poll").unwrap(),
        &check,
    ))
    .unwrap() else {
        panic!("expected ready")
    };
    assert_eq!(ready.start_jwt.expose(), "synthetic-start-jwt");
    assert_eq!(
        ready.provider_access_token.expose(),
        "synthetic-oauth-token"
    );
    assert_eq!(ready.refresh_token.unwrap().expose(), "synthetic-refresh");
    assert_eq!(ready.user.id, "user-a");
    assert_eq!(
        client.transport.requests.lock().unwrap()[0].url,
        "https://zcode.z.ai/api/v1/oauth/cli/poll/flow%2Fwith%3Fspecial%23chars"
    );
}

#[test]
fn poll_requires_the_selected_family_and_rejects_ambiguous_aliases() {
    for data in [
        r#"{"status":"ready","token":"jwt","user":{"user_id":"user"},"bigmodel":{"access_token":"other-family"}}"#,
        r#"{"status":"ready","token":"jwt","user":{"user_id":"user"},"zai":{"access_token":"first","accessToken":"different"}}"#,
    ] {
        let body = format!("{{\"code\":0,\"data\":{data}}}");
        let client = OfficialClient::new(Fixture::new(&[&body]));
        assert!(matches!(
            run(client.poll(
                OAuthFamily::Zai,
                "flow",
                &PollToken::new("poll").unwrap(),
                &check
            )),
            Err(OfficialError::InvalidResponse)
        ));
    }
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":0,"data":{"status":"pending"}}"#,
        r#"{"code":0,"data":{"status":"failed"}}"#,
    ]));
    assert!(matches!(
        run(client.poll(
            OAuthFamily::BigModel,
            "flow",
            &PollToken::new("poll").unwrap(),
            &check
        )),
        Ok(OAuthPoll::Pending)
    ));
    assert!(matches!(
        run(client.poll(
            OAuthFamily::BigModel,
            "flow",
            &PollToken::new("poll").unwrap(),
            &check
        )),
        Ok(OAuthPoll::Failed)
    ));
}

#[test]
fn zai_normalizes_oauth_token_once_bigmodel_never_exchanges() {
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":200,"data":{"access_token":"normalized-business"}}"#,
    ]));
    let access = ProviderAccessToken::new("synthetic-oauth").unwrap();
    let token = run(client.normalize_business_token(OAuthFamily::Zai, &access, &check)).unwrap();
    assert_eq!(token.expose(), "normalized-business");
    assert_eq!(token.family(), OAuthFamily::Zai);
    let requests = client.transport.requests.lock().unwrap();
    assert_eq!(requests[0].url, "https://api.z.ai/api/auth/z/login");
    assert!(requests[0].authorization.is_none());
    assert_eq!(
        serde_json::from_slice::<Value>(requests[0].body.as_ref().unwrap()).unwrap(),
        serde_json::json!({"token":"synthetic-oauth"})
    );
    drop(requests);
    let token =
        run(client.normalize_business_token(OAuthFamily::BigModel, &access, &check)).unwrap();
    assert_eq!(token.expose(), "synthetic-oauth");
    assert_eq!(client.transport.requests.lock().unwrap().len(), 1);
}

#[test]
fn customer_chooses_personal_project_and_uses_family_specific_auth() {
    let body = r#"{"code":200,"data":{"customerNumber":"customer-a","customerName":"Example","organizations":[{"organizationId":"org-team","projects":[{"projectId":"team","projectType":2}]},{"organizationId":"org-personal","organizationName":"默认机构","projects":[{"projectId":"team-2","projectType":"2","projectName":"默认项目"},{"projectId":"personal","projectType":1}]}]}}"#;
    for (family, host, auth) in [
        (
            OAuthFamily::BigModel,
            "https://bigmodel.cn",
            "synthetic-business-token",
        ),
        (
            OAuthFamily::Zai,
            "https://api.z.ai",
            "Bearer synthetic-business-token",
        ),
    ] {
        let client = OfficialClient::new(Fixture::new(&[body]));
        let customer = run(client.read_customer(&biz(family), &check)).unwrap();
        assert_eq!(customer.identity_id.as_deref(), Some("customer-a"));
        assert_eq!(customer.personal_project.unwrap().project_id, "personal");
        let requests = client.transport.requests.lock().unwrap();
        assert_eq!(
            requests[0].url,
            format!("{host}/api/biz/customer/getCustomerInfo")
        );
        assert_eq!(requests[0].authorization.as_ref().unwrap().expose(), auth);
    }
}

#[test]
fn existing_key_discovery_copies_without_creation_and_missing_key_stays_missing() {
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":200,"data":[{"name":"other","apiKey":"ignore"},{"name":"zcode-api-key","apiKey":"key/id"}]}"#,
        r#"{"data":{"secretKey":"copy-secret"}}"#,
    ]));
    let key = run(client.discover_key(&biz(OAuthFamily::Zai), &project(), &check))
        .unwrap()
        .unwrap();
    assert_eq!(key.expose(), "key/id.copy-secret");
    let requests = client.transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.method == OfficialMethod::Get));
    assert!(requests[1].url.ends_with("/api_keys/copy/key%2Fid"));
    drop(requests);
    let client = OfficialClient::new(Fixture::new(&[r#"{"data":[]}"#]));
    assert!(
        run(client.discover_key(&biz(OAuthFamily::BigModel), &project(), &check))
            .unwrap()
            .is_none()
    );
    assert_eq!(client.transport.requests.lock().unwrap().len(), 1);
}

#[test]
fn zai_requires_copy_secret_bigmodel_accepts_bare_api_key_only_after_successful_copy() {
    let key = KeySummary {
        name: KEY_NAME.into(),
        api_key: SecretValue::new("key-id").unwrap(),
    };
    for family in [OAuthFamily::Zai, OAuthFamily::BigModel] {
        let client = OfficialClient::new(Fixture::new(&[r#"{"data":{}}"#]));
        let result = run(client.copy_key(&biz(family), &project(), &key, &check));
        match family {
            OAuthFamily::Zai => assert!(matches!(result, Err(OfficialError::InvalidResponse))),
            OAuthFamily::BigModel => assert_eq!(result.unwrap().expose(), "key-id"),
        }
    }
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":401,"msg":"contains-secret","data":{}}"#,
    ]));
    assert!(matches!(
        run(client.copy_key(&biz(OAuthFamily::BigModel), &project(), &key, &check)),
        Err(OfficialError::BusinessRejected)
    ));
}

#[test]
fn cancellation_at_each_boundary_prevents_later_credentials_in_same_row() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for cancel_at in 0..4 {
        let client = OfficialClient::new(Fixture::new(&[
            r#"{"data":[{"name":"zcode-api-key","apiKey":"key-id"}]}"#,
            r#"{"data":{"secretKey":"secret"}}"#,
        ]));
        let calls = AtomicUsize::new(0);
        let check = || {
            if calls.fetch_add(1, Ordering::SeqCst) == cancel_at {
                Err(OfficialError::Cancelled)
            } else {
                Ok(())
            }
        };
        assert!(matches!(
            run(client.discover_key(&biz(OAuthFamily::Zai), &project(), &check)),
            Err(OfficialError::Cancelled)
        ));
        assert_eq!(
            client.transport.requests.lock().unwrap().len(),
            (cancel_at + 1) / 2
        );
    }
}

#[test]
fn explicit_create_is_one_post_and_keeps_uncertainty_after_lost_response() {
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":200,"data":{"name":"zcode-api-key","apiKey":"created-key"}}"#,
    ]));
    let permit = KeyCreationPermit::new("account-a", OAuthFamily::Zai, project()).unwrap();
    assert!(matches!(
        run(client.create_key_once(&biz(OAuthFamily::Zai), "account-a", permit, &check)),
        CreateKeyOutcome::Created(_)
    ));
    let requests = client.transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, OfficialMethod::Post);
    assert_eq!(
        serde_json::from_slice::<Value>(requests[0].body.as_ref().unwrap()).unwrap(),
        serde_json::json!({"name":"zcode-api-key"})
    );
    drop(requests);
    let client = OfficialClient::new(Fixture::new(&[]));
    let permit = KeyCreationPermit::new("account-a", OAuthFamily::Zai, project()).unwrap();
    assert!(matches!(
        run(client.create_key_once(&biz(OAuthFamily::Zai), "account-a", permit, &check)),
        CreateKeyOutcome::MayHaveBeenSent(OfficialError::Transport)
    ));
    assert_eq!(client.transport.requests.lock().unwrap().len(), 1);
}

#[test]
fn create_scope_mismatch_or_pre_send_cancellation_never_transmits() {
    for (family, id, cancelled) in [
        (OAuthFamily::BigModel, "account-a", false),
        (OAuthFamily::Zai, "account-b", false),
        (OAuthFamily::Zai, "account-a", true),
    ] {
        let client = OfficialClient::new(Fixture::new(&[]));
        let permit = KeyCreationPermit::new("account-a", OAuthFamily::Zai, project()).unwrap();
        let guard = || {
            if cancelled {
                Err(OfficialError::Cancelled)
            } else {
                Ok(())
            }
        };
        assert!(matches!(
            run(client.create_key_once(&biz(family), id, permit, &guard)),
            CreateKeyOutcome::NotSent(_)
        ));
        assert_eq!(client.transport.requests.lock().unwrap().len(), 0);
    }
}

#[test]
fn create_cancellation_after_send_is_uncertain_not_not_sent() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = AtomicUsize::new(0);
    let guard = || {
        if calls.fetch_add(1, Ordering::SeqCst) > 0 {
            Err(OfficialError::Cancelled)
        } else {
            Ok(())
        }
    };
    let client = OfficialClient::new(Fixture::new(&[r#"{"data":{"apiKey":"created-key"}}"#]));
    let permit = KeyCreationPermit::new("account-a", OAuthFamily::Zai, project()).unwrap();
    assert!(matches!(
        run(client.create_key_once(&biz(OAuthFamily::Zai), "account-a", permit, &guard)),
        CreateKeyOutcome::MayHaveBeenSent(OfficialError::Cancelled)
    ));
    assert_eq!(client.transport.requests.lock().unwrap().len(), 1);
}

#[test]
fn capabilities_read_only_their_distinct_credentials_without_model_requests() {
    let client = OfficialClient::new(Fixture::new(&[
        r#"{"code":0,"data":{"plans":[],"balances":[{"remaining_units":0}]}}"#,
        r#"{"code":200,"data":[]}"#,
        r#"{"code":200,"data":{"limits":[]}}"#,
    ]));
    let start =
        run(client.read_start_balance(&StartJwt::new("synthetic-jwt").unwrap(), "3.14.4", &check))
            .unwrap();
    assert_eq!(start.value()["balances"][0]["remaining_units"], 0);
    let key = CodingKey::new("synthetic-coding-key").unwrap();
    run(client.read_coding_subscription(OAuthFamily::BigModel, &key, &check)).unwrap();
    run(client.read_coding_quota(OAuthFamily::Zai, &key, &check)).unwrap();
    let requests = client.transport.requests.lock().unwrap();
    assert_eq!(
        requests[0].url,
        "https://zcode.z.ai/api/v1/zcode-plan/billing/balance?app_version=3.14.4"
    );
    assert_eq!(
        requests[0].authorization.as_ref().unwrap().expose(),
        "Bearer synthetic-jwt"
    );
    assert_eq!(
        requests[1].url,
        "https://bigmodel.cn/api/biz/subscription/list"
    );
    assert_eq!(
        requests[2].url,
        "https://api.z.ai/api/monitor/usage/quota/limit"
    );
    assert!(requests[1..]
        .iter()
        .all(|r| r.authorization.as_ref().unwrap().expose() == "synthetic-coding-key"));
    assert!(requests.iter().all(|r| r.method == OfficialMethod::Get));
}

#[test]
fn status_errors_malformed_and_oversized_payloads_never_expose_remote_secrets() {
    for (status, body, expected) in [
        (401, "secret-body", OfficialError::Unauthorized),
        (302, "secret-body", OfficialError::Http(302)),
        (
            200,
            r#"{"code":500,"msg":"secret-body"}"#,
            OfficialError::BusinessRejected,
        ),
        (200, "secret-body", OfficialError::InvalidResponse),
    ] {
        let response = OfficialResponse {
            status,
            body: Zeroizing::new(body.as_bytes().to_vec()),
        };
        let err = parse_response(response, false).unwrap_err();
        assert_eq!(err, expected);
        assert!(!format!("{err:?} {err}").contains("secret-body"));
    }
    let response = OfficialResponse {
        status: 200,
        body: Zeroizing::new(vec![b' '; MAX_RESPONSE_BYTES + 1]),
    };
    assert_eq!(
        parse_response(response, false).unwrap_err(),
        OfficialError::ResponseTooLarge
    );
}

#[test]
fn init_rejects_insecure_urls_and_invalid_lifetime_or_poll_interval() {
    for (url, expires, interval) in [
        ("http://example.invalid", 1, 2),
        ("https://user:secret@example.invalid", 1, 2),
        ("https://example.invalid", 0, 2),
        ("https://example.invalid", 1, 0),
    ] {
        let body=serde_json::json!({"code":0,"data":{"authorize_url":url,"flow_id":"flow","expires_at":expires,"poll_interval_sec":interval}}).to_string();
        let client = OfficialClient::new(Fixture::new(&[&body]));
        assert!(
            run(client.init(OAuthFamily::Zai, &PollToken::new("poll").unwrap(), &check)).is_err()
        );
    }
}
