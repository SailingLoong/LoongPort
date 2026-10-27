//! Private protocol/security regression tests, using synthetic encrypted owners only.
//! These deliberately exercise the retained private wire/CAS/stream primitive, NOT
//! public model availability. Public acquire/discovery/dispatch/authorization are
//! unconditionally paused and separately tested by workbuddy_shared_authorization.
use super::super::model::Account;
use super::*;
use std::collections::VecDeque;

struct Fixture {
    _root: tempfile::TempDir,
    session: Arc<SecretSession>,
    ids: Vec<String>,
}
impl Fixture {
    fn new(authorized: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let session = SecretSession::from_context(
            root.path().into(),
            crate::secrets::VaultContext::generate().unwrap(),
        );
        let b = store::binding(&session).unwrap();
        let rows: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| {
                let mut row = SavedAccount::new(Account {
                    uid: format!("synthetic-{id}"),
                    domain: Domain::WorkBuddy,
                    label: format!("Synthetic {id}"),
                    token: format!("old-checkin-{id}"),
                });
                if authorized {
                    row.inference = Some(Authorization {
                        platform: PLATFORM.into(),
                        domain: Domain::WorkBuddy,
                        uid: row.account.uid.clone(),
                        grant_id: uuid::Uuid::new_v4().to_string(),
                        credential_version: 1,
                        access: format!("new-inference-{id}"),
                        refresh: format!("refresh-{id}"),
                        expires_ms: 2_000_000,
                        refresh_expires_ms: Some(9_000_000),
                    });
                }
                row
            })
            .collect();
        let ids = rows.iter().map(|r| r.account.id(b.store_id())).collect();
        store::save(&session, &b, rows).unwrap();
        Self {
            _root: root,
            session,
            ids,
        }
    }
    fn change(&self, mutate: impl FnOnce(&mut Vec<SavedAccount>)) {
        let b = store::binding(&self.session).unwrap();
        let mut rows = store::load(&self.session, &b).unwrap();
        mutate(&mut rows);
        store::save(&self.session, &b, rows).unwrap();
    }
    async fn lease(&self, index: usize) -> Lease {
        Lease::acquire_with_expected(self.session.clone(), &self.ids[index], true, None)
            .await
            .unwrap()
    }
}
#[derive(Default)]
struct Fake {
    requests: Mutex<Vec<Request>>,
    responses: Mutex<VecDeque<ProxyResponse>>,
}
impl Fake {
    fn with(responses: Vec<ProxyResponse>) -> Self {
        Self {
            requests: Default::default(),
            responses: Mutex::new(responses.into()),
        }
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}
impl Transport for Fake {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProxyError::ForwardFailed("synthetic exhausted".into()))
        })
    }
}
fn json_reply(status: u16, value: Value) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::from_u16(status).unwrap(),
        safe_headers(&http::HeaderMap::new(), false),
        Bytes::from(value.to_string()),
    )
}
fn success() -> ProxyResponse {
    ProxyResponse::streamed(
        http::StatusCode::OK,
        safe_headers(&http::HeaderMap::new(), true),
        futures::stream::iter([Ok(Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"synthetic\"}}]}\n\ndata: [DONE]\n\n",
        ))]),
    )
}
fn body() -> Value {
    json!({"model":"live-model-id","stream":true,"messages":[{"role":"system","content":"Codex Claude Code unchanged"},{"role":"user","content":"synthetic"}],"tools":[{"type":"function","function":{"name":"a.b","parameters":{"type":"object"}}}],"tool_choice":"auto"})
}
fn rotated() -> ProxyResponse {
    json_reply(
        200,
        json!({"code":0,"data":{"accessToken":"new-rotated","refreshToken":"rotated-refresh","expiresIn":3600,"domain":"www.workbuddy.cn"}}),
    )
}
fn signin_replies(uid: &str, domain: &str) -> Vec<ProxyResponse> {
    vec![
        json_reply(
            200,
            json!({"code":0,"data":{"state":"synthetic-state","authUrl":"https://copilot.tencent.com/login?state=synthetic-state"}}),
        ),
        json_reply(
            200,
            json!({"code":0,"data":{"accessToken":"new-authorized","refreshToken":"new-refresh","expiresAt":2_000_000,"refreshExpiresIn":3600,"domain":domain}}),
        ),
        json_reply(
            200,
            json!({"code":0,"data":{"uid":uid,"domain":domain,"type":"personal"}}),
        ),
    ]
}

#[cfg_attr(test, tokio::test)]
async fn two_selected_accounts_keep_distinct_new_grants_and_preserve_body() {
    let f = Fixture::new(true);
    let t = Fake::with(vec![success(), success()]);
    for n in 0..2 {
        f.lease(n)
            .await
            .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
            .await
            .unwrap()
            .response
            .bytes_with_limit(4096)
            .await
            .unwrap();
    }
    let calls = t.requests.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for (n, id) in ["a", "b"].iter().enumerate() {
        assert_eq!(calls[n].endpoint.path(), "/v2/chat/completions");
        assert_eq!(
            calls[n].headers["authorization"],
            format!("Bearer new-inference-{id}")
        );
        assert!(calls[n].headers["authorization"].is_sensitive());
        assert_eq!(calls[n].headers["x-user-id"], format!("synthetic-{id}"));
        assert_eq!(
            serde_json::from_slice::<Value>(&calls[n].body).unwrap(),
            body()
        );
        assert!(!String::from_utf8_lossy(&calls[n].body).contains("old-checkin"));
    }
}
#[cfg_attr(test, tokio::test)]
async fn legacy_rows_are_not_inference_authorized_and_keep_existing_ui_dto() {
    let f = Fixture::new(false);
    assert!(
        Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
            .await
            .is_err()
    );
    let b = store::binding(&f.session).unwrap();
    let rows = store::load(&f.session, &b).unwrap();
    let encoded = serde_json::to_value(&rows[0]).unwrap();
    assert!(encoded.get("inference").is_none());
    let read: SavedAccount = serde_json::from_value(encoded).unwrap();
    assert!(read.inference.is_none());
    let ui = serde_json::to_string(&read.view(b.store_id(), "2026-10-18")).unwrap();
    assert!(!ui.contains("token"));
    assert!(!ui.contains("inference"));
}
#[cfg_attr(test, tokio::test)]
async fn explicit_new_grant_binds_only_matching_original_row_and_keeps_checkin() {
    let f = Fixture::new(false);
    let t = Fake::with(signin_replies("synthetic-a", "www.workbuddy.cn"));
    let mut pending =
        PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], &t, 1000)
            .await
            .unwrap();
    assert!(matches!(
        pending.poll_protocol_with(&t, 2000).await.unwrap(),
        AuthorizationProgress::Saved
    ));
    let b = store::binding(&f.session).unwrap();
    let rows = store::load(&f.session, &b).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].account.token.is_empty());
    assert!(rows[0].shared_authorization);
    assert!(rows[0].inference_allowed());
    assert_eq!(rows[0].account.id(b.store_id()), f.ids[0]);
    assert_eq!(rows[0].inference.as_ref().unwrap().access, "new-authorized");
    assert!(rows[1].inference.is_none());
    let calls = t.requests.lock().unwrap();
    assert_eq!(
        calls[0].query,
        Some(("platform".into(), "workbuddy".into()))
    );
    assert!(!calls[0].headers.contains_key("authorization"));
    assert!(!calls[1].headers.contains_key("authorization"));
    assert_eq!(calls[2].headers["authorization"], "Bearer new-authorized");
    let file = std::fs::read(f._root.path().join(store::FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&file).contains("new-authorized"));
    assert!(
        !serde_json::to_string(&rows[0].view(b.store_id(), "2026-10-18"))
            .unwrap()
            .contains("new-authorized")
    );
}
#[cfg_attr(test, tokio::test)]
async fn wrong_uid_or_domain_or_platform_never_binds_or_migrates() {
    for (uid, domain) in [
        ("different-user", "www.workbuddy.cn"),
        ("synthetic-a", "www.codebuddy.cn"),
        ("synthetic-a", "copilot.tencent.com"),
    ] {
        let f = Fixture::new(false);
        let t = Fake::with(signin_replies(uid, domain));
        let mut p =
            PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], &t, 1000)
                .await
                .unwrap();
        assert!(p.poll_protocol_with(&t, 2000).await.is_err());
        assert!(
            Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
                .await
                .is_err()
        );
    }
    let f = Fixture::new(false);
    let b = store::binding(&f.session).unwrap();
    let rows = store::load(&f.session, &b).unwrap();
    let owner = Owner::from_row(&f.session, b, &rows[0]);
    let t = json!({"code":0,"data":{"accessToken":"synthetic","refreshToken":"synthetic","domain":"www.workbuddy.cn","expiresIn":3600,"platform":"workbuddy-ai"}});
    assert!(new_grant(
        &owner,
        &t,
        &json!({"code":0,"data":{"uid":"synthetic-a"}}),
        1000
    )
    .is_err());
}
#[cfg_attr(test, tokio::test)]
async fn stale_login_flow_cannot_replace_a_newer_grant() {
    let f = Fixture::new(false);
    let t = Fake::with(signin_replies("synthetic-a", "www.workbuddy.cn"));
    let mut p = PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], &t, 1000)
        .await
        .unwrap();
    f.change(|rows| {
        rows[0].inference = Some(Authorization {
            platform: PLATFORM.into(),
            domain: Domain::WorkBuddy,
            uid: "synthetic-a".into(),
            grant_id: uuid::Uuid::new_v4().to_string(),
            credential_version: 1,
            access: "newer".into(),
            refresh: "newer".into(),
            expires_ms: 2_000_000,
            refresh_expires_ms: None,
        })
    });
    assert!(p.poll_protocol_with(&t, 2000).await.is_err());
    assert_eq!(t.count(), 1);
}
#[cfg_attr(test, tokio::test)]
async fn unsafe_authorization_urls_are_rejected_without_following() {
    for url in [
        "https://evil.invalid/?state=s",
        "http://copilot.tencent.com/?state=s",
        "https://copilot.tencent.com:444/?state=s",
        "https://copilot.tencent.com/?state=s&state=s",
        "https://copilot.tencent.com/?state=s&token=secret",
    ] {
        let f = Fixture::new(false);
        let t = Fake::with(vec![json_reply(
            200,
            json!({"code":0,"data":{"state":"s","authUrl":url}}),
        )]);
        assert!(
            PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], &t, 1000)
                .await
                .is_err()
        );
        assert_eq!(t.count(), 1);
    }
}
#[cfg_attr(test, tokio::test)]
async fn models_use_only_live_cli_ids_without_static_fallback_or_pricing() {
    let f = Fixture::new(true);
    let t = Fake::with(vec![json_reply(
        200,
        json!({"code":0,"data":{"agents":[{"name":"other","models":["foreign"]},{"name":"cli","models":["live","renamed","live"]}],"models":[{"id":"live","name":"Label","price":999}]}}),
    )]);
    let models = f
        .lease(0)
        .await
        .models_protocol_with(&t, 1000)
        .await
        .unwrap();
    assert_eq!(
        models,
        vec![
            Model {
                id: "live".into(),
                name: "Label".into()
            },
            Model {
                id: "renamed".into(),
                name: "renamed".into()
            }
        ]
    );
    assert!(!serde_json::to_string(&models).unwrap().contains("price"));
}
#[cfg_attr(test, tokio::test)]
async fn explicit_401_refreshes_same_grant_once_and_never_sends_old_checkin_token() {
    let f = Fixture::new(true);
    let t = Fake::with(vec![
        json_reply(401, json!({"code":401})),
        rotated(),
        success(),
    ]);
    f.lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap()
        .response
        .bytes_with_limit(4096)
        .await
        .unwrap();
    let calls = t.requests.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[1].endpoint.path(), "/v2/plugin/auth/token/refresh");
    assert_eq!(calls[1].headers["x-refresh-token"], "refresh-a");
    assert!(calls[1].headers["x-refresh-token"].is_sensitive());
    assert_eq!(calls[2].headers["authorization"], "Bearer new-rotated");
}
#[cfg_attr(test, tokio::test)]
async fn expiry_refresh_uses_the_only_replay_budget_and_preserves_second_401() {
    let f = Fixture::new(true);
    f.change(|r| r[0].inference.as_mut().unwrap().expires_ms = 1001);
    let t = Fake::with(vec![
        rotated(),
        json_reply(401, json!({"msg":"new-rotated secret echo"})),
    ]);
    let r = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    assert_eq!(r.response.status(), http::StatusCode::UNAUTHORIZED);
    assert_eq!(t.count(), 2);
    let bytes = r.response.bytes_with_limit(4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("new-rotated"));
}
#[cfg_attr(test, tokio::test)]
async fn concurrent_refreshes_spend_a_refresh_token_once() {
    let f = Fixture::new(true);
    let mut a = f.lease(0).await;
    let mut b = f.lease(0).await;
    let t = Fake::with(vec![rotated()]);
    let (a_result, b_result) = tokio::join!(a.refresh(&t, 1000), b.refresh(&t, 1000));
    a_result.unwrap();
    b_result.unwrap();
    assert_eq!(t.count(), 1);
    assert_eq!(a.grant.credential_version, 2);
    assert_eq!(b.grant.access, "new-rotated");
}
#[cfg_attr(test, tokio::test)]
async fn refresh_identity_mismatch_is_rejected_without_overwriting() {
    let f = Fixture::new(true);
    let mut a = f.lease(0).await;
    let t = Fake::with(vec![json_reply(
        200,
        json!({"code":0,"data":{"accessToken":"foreign","expiresIn":3600,"domain":"www.codebuddy.cn"}}),
    )]);
    assert!(a.refresh(&t, 1000).await.is_err());
    assert_eq!(f.lease(0).await.grant.access, "new-inference-a");
}
#[cfg_attr(test, tokio::test)]
async fn refusals_are_terminal_structured_and_never_rewrite_replay_or_rotate() {
    for (status, code) in [(403, 11004), (403, 11140), (200, 11004), (200, 11140)] {
        let f = Fixture::new(true);
        let t = Fake::with(vec![json_reply(
            status,
            json!({"code":code,"msg":"Bearer new-inference-a echo"}),
        )]);
        let out = f
            .lease(0)
            .await
            .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
            .await
            .unwrap();
        assert_eq!(out.response.status().as_u16(), 403);
        assert_eq!(t.count(), 1);
        let value: Value =
            serde_json::from_slice(&out.response.bytes_with_limit(4096).await.unwrap()).unwrap();
        assert_eq!(value["error"]["type"], "access_denied");
        assert_eq!(value["error"]["code"], format!("workbuddy_{code}"));
        assert_eq!(value["error"]["upstream_status"], status);
        assert!(!value.to_string().contains("new-inference"));
    }
}
#[cfg_attr(test, tokio::test)]
async fn status_429_and_retry_after_are_preserved_but_secret_headers_are_removed() {
    let f = Fixture::new(true);
    let mut h = http::HeaderMap::new();
    h.insert("retry-after", http::HeaderValue::from_static("42"));
    h.insert("x-echo-token", http::HeaderValue::from_static("secret"));
    let t = Fake::with(vec![ProxyResponse::buffered(
        http::StatusCode::TOO_MANY_REQUESTS,
        h,
        Bytes::from_static(b"secret"),
    )]);
    let out = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    assert_eq!(out.response.status().as_u16(), 429);
    assert_eq!(out.response.headers()["retry-after"], "42");
    assert!(!out.response.headers().contains_key("x-echo-token"));
    assert_eq!(t.count(), 1);
}
#[cfg_attr(test, tokio::test)]
async fn success_headers_are_allowlisted_and_sse_bytes_unchanged() {
    let f = Fixture::new(true);
    let bytes = Bytes::from_static(
        b"data: {\"choices\":[{\"delta\":{\"content\":\"synthetic\"}}]}\n\ndata: [DONE]\n\n",
    );
    let mut h = safe_headers(&http::HeaderMap::new(), true);
    h.insert("set-cookie", http::HeaderValue::from_static("secret"));
    h.insert("x-echo", http::HeaderValue::from_static("new-inference-a"));
    let t = Fake::with(vec![ProxyResponse::streamed(
        http::StatusCode::OK,
        h,
        futures::stream::iter([Ok(bytes.clone())]),
    )]);
    let out = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    assert_eq!(out.response.headers().len(), 1);
    assert_eq!(out.response.bytes_with_limit(4096).await.unwrap(), bytes);
}
#[cfg_attr(test, tokio::test)]
async fn final_fence_rejects_previously_primed_or_buffered_bytes_after_retirement() {
    let f = Fixture::new(true);
    let t = Fake::with(vec![success()]);
    let out = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    let bytes = out.response.bytes_with_limit(4096).await.unwrap();
    f.change(|r| r[0].inference = None);
    let mut final_body = Box::pin(
        out.fence
            .wrap_stream(futures::stream::iter([Ok::<_, std::io::Error>(bytes)])),
    );
    assert!(final_body.next().await.unwrap().is_err());
    assert!(final_body.next().await.is_none());
}
#[cfg_attr(test, tokio::test)]
async fn blocked_then_unblocked_session_never_revives_old_lease() {
    let f = Fixture::new(true);
    let lease = f.lease(0).await;
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    let t = Fake::with(vec![success()]);
    assert!(lease
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .is_err());
    assert_eq!(t.count(), 0);
}
#[cfg_attr(test, tokio::test)]
async fn post_output_stream_failure_is_sanitized_and_never_replayed() {
    let f = Fixture::new(true);
    let t = Fake::with(vec![ProxyResponse::streamed(
        http::StatusCode::OK,
        safe_headers(&http::HeaderMap::new(), true),
        futures::stream::iter([
            Ok(Bytes::from_static(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n",
            )),
            Err(std::io::Error::other("new-inference-a secret")),
        ]),
    )]);
    let out = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    let mut stream = Box::pin(out.response.bytes_stream());
    assert!(stream.next().await.unwrap().is_ok());
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.to_string(), "workbuddy.stream_interrupted");
    assert!(stream.next().await.is_none());
    assert_eq!(t.count(), 1);
}
#[cfg_attr(test, tokio::test)]
async fn nonstream_request_is_rejected_before_network_instead_of_silently_rewritten() {
    let f = Fixture::new(true);
    let t = Fake::default();
    let mut request = body();
    request["stream"] = Value::Bool(false);
    assert!(f
        .lease(0)
        .await
        .dispatch_protocol_with(request, Duration::from_secs(3), &t, 1000)
        .await
        .is_err());
    assert_eq!(t.count(), 0);
}

#[cfg_attr(test, tokio::test)]
async fn response_echo_across_chunks_is_withheld_then_terminated_without_replay() {
    let f = Fixture::new(true);
    let chunks = vec![
        Ok(Bytes::from_static(b"data: safe new-infer")),
        Ok(Bytes::from_static(b"ence-a suffix")),
    ];
    let t = Fake::with(vec![ProxyResponse::streamed(
        http::StatusCode::OK,
        safe_headers(&http::HeaderMap::new(), true),
        futures::stream::iter(chunks),
    )]);
    let out = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    let mut stream = Box::pin(out.response.bytes_stream());
    let mut received = vec![];
    let mut failed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(bytes) => received.extend_from_slice(&bytes),
            Err(_) => failed = true,
        }
    }
    assert!(failed);
    assert!(!String::from_utf8_lossy(&received).contains("new-infer"));
    assert_eq!(t.count(), 1);
}
#[cfg_attr(test, tokio::test)]
async fn declared_compression_is_rejected_and_requests_ask_identity() {
    let f = Fixture::new(true);
    let mut headers = safe_headers(&http::HeaderMap::new(), true);
    headers.insert("content-encoding", http::HeaderValue::from_static("gzip"));
    let t = Fake::with(vec![ProxyResponse::streamed(
        http::StatusCode::OK,
        headers,
        futures::stream::iter([Ok(Bytes::from_static(b"compressed"))]),
    )]);
    assert_eq!(
        f.lease(0)
            .await
            .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
            .await
            .unwrap()
            .response
            .status(),
        http::StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(
        t.requests.lock().unwrap()[0].headers["accept-encoding"],
        "identity"
    );
}
#[cfg_attr(test, tokio::test)]
async fn fixture_origin_and_owner_mismatch_never_fall_back_to_production() {
    let f = Fixture::new(true);
    for origin in [
        "https://127.0.0.1:8",
        "http://localhost:8",
        "http://example.com:8",
        "http://127.0.0.1:8/path",
        "http://user@127.0.0.1:8/",
    ] {
        assert!(FixtureTransport::loopback(&f.session, origin).is_err());
    }
    // Even generated but non-seam grants must not be sent to fixture origins.
    assert!(FixtureTransport::loopback(&f.session, "http://127.0.0.1:8").is_err());
    let other_root = tempfile::tempdir().unwrap();
    let other_session = SecretSession::from_context(
        other_root.path().into(),
        crate::secrets::VaultContext::generate().unwrap(),
    );
    FixtureTransport::seed_accounts(&other_session, &["synthetic-fixture"])
        .await
        .unwrap();
    let fixture = FixtureTransport::loopback(&other_session, "http://127.0.0.1:8").unwrap();
    assert!(f
        .lease(0)
        .await
        .dispatch_fixture(body(), Duration::from_secs(3), &fixture)
        .await
        .is_err());
    assert_eq!(fixture.attempts(), 0);
    assert!(
        FixtureTransport::seed_accounts(&f.session, &["synthetic-a"])
            .await
            .is_err()
    );
}

#[cfg_attr(test, tokio::test)]
async fn every_documented_denial_envelope_blocks_401_refresh_and_preserves_exact_codes() {
    for status in [401, 403, 200] {
        for value in [
            json!({"error":{"data":{"code":11004}}}),
            json!({"error":{"code":"11140"}}),
            json!({"data":{"code":"11004"}}),
            json!({"code":503,"error":{"code":11140}}),
        ] {
            let f = Fixture::new(true);
            let t = Fake::with(vec![json_reply(status, value)]);
            let out = f
                .lease(0)
                .await
                .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
                .await
                .unwrap();
            assert_eq!(t.count(), 1);
            let value: Value =
                serde_json::from_slice(&out.response.bytes_with_limit(4096).await.unwrap())
                    .unwrap();
            assert_eq!(value["error"]["type"], "access_denied");
            assert!(matches!(
                value["error"]["code"].as_str(),
                Some("workbuddy_11004" | "workbuddy_11140")
            ));
        }
    }
    assert_eq!(upstream_error_code(&json!({"code":"111400"})), Some(111400));
    assert_eq!(upstream_error_code(&json!({"msg":"code 11140"})), None);
}

struct DelayedResponse {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    attempts: std::sync::atomic::AtomicUsize,
    response: Mutex<Option<ProxyResponse>>,
}
impl DelayedResponse {
    fn new(response: ProxyResponse) -> Arc<Self> {
        Arc::new(Self {
            entered: Default::default(),
            release: Default::default(),
            attempts: Default::default(),
            response: Mutex::new(Some(response)),
        })
    }
    fn count(&self) -> usize {
        self.attempts.load(std::sync::atomic::Ordering::Acquire)
    }
}
impl Transport for DelayedResponse {
    fn request(
        &self,
        _: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.entered.notify_one();
            self.release.notified().await;
            self.response
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| ProxyError::ForwardFailed("synthetic exhausted".into()))
        })
    }
}
#[cfg_attr(test, tokio::test)]
async fn pending_refresh_is_single_flight_and_second_lease_reads_persisted_rotation() {
    let f = Fixture::new(true);
    let mut first = f.lease(0).await;
    let mut second = f.lease(0).await;
    let t = DelayedResponse::new(rotated());
    let first_t = t.clone();
    let a = tokio::spawn(async move {
        first.refresh(first_t.as_ref(), 1000).await?;
        Ok::<_, Failure>(first.grant.credential_version)
    });
    t.entered.notified().await;
    let started = Arc::new(tokio::sync::Notify::new());
    let second_started = started.clone();
    let second_t = t.clone();
    let b = tokio::spawn(async move {
        second_started.notify_one();
        second.refresh(second_t.as_ref(), 1000).await?;
        Ok::<_, Failure>(second.grant.credential_version)
    });
    started.notified().await;
    tokio::task::yield_now().await;
    assert_eq!(t.count(), 1);
    assert!(!a.is_finished());
    assert!(!b.is_finished());
    t.release.notify_one();
    let results = tokio::time::timeout(Duration::from_secs(3), async { tokio::join!(a, b) })
        .await
        .unwrap();
    assert_eq!(results.0.unwrap().unwrap(), 2);
    assert_eq!(results.1.unwrap().unwrap(), 2);
    assert_eq!(t.count(), 1);
}
#[cfg_attr(test, tokio::test)]
async fn late_refresh_response_cannot_revive_retired_original_grant() {
    let f = Fixture::new(true);
    let mut lease = f.lease(0).await;
    let t = DelayedResponse::new(rotated());
    let transport = t.clone();
    let task = tokio::spawn(async move { lease.refresh(transport.as_ref(), 1000).await });
    t.entered.notified().await;
    {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        f.change(|rows| rows[0].inference = None);
    }
    t.release.notify_one();
    assert!(task.await.unwrap().is_err());
    assert!(
        Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
            .await
            .is_err()
    );
    assert_eq!(t.count(), 1);
}
#[cfg_attr(test, tokio::test)]
async fn waiting_token_response_after_session_epoch_change_is_not_published() {
    let f = Fixture::new(false);
    let begin = Fake::with(signin_replies("synthetic-a", "www.workbuddy.cn"));
    let mut pending =
        PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], &begin, 1000)
            .await
            .unwrap();
    let t = DelayedResponse::new(json_reply(200, json!({"code":11217})));
    let transport = t.clone();
    let task =
        tokio::spawn(async move { pending.poll_protocol_with(transport.as_ref(), 2000).await });
    t.entered.notified().await;
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    t.release.notify_one();
    assert!(task.await.unwrap().is_err());
    assert!(
        Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
            .await
            .is_err()
    );
}

#[cfg_attr(test, tokio::test)]
async fn incomplete_error_body_has_a_bounded_terminal_deadline_without_refresh() {
    let f = Fixture::new(true);
    let response = ProxyResponse::streamed(
        http::StatusCode::UNAUTHORIZED,
        safe_headers(&http::HeaderMap::new(), false),
        futures::stream::pending::<Result<Bytes, std::io::Error>>(),
    );
    let t = Fake::with(vec![response]);
    let lease = f.lease(0).await;
    assert_eq!(lease.base_url(), "https://copilot.tencent.com");
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        lease.dispatch_protocol_with(body(), Duration::from_millis(10), &t, 1000),
    )
    .await
    .unwrap();
    assert!(
        matches!(result,Err(ProxyError::Internal(ref message)) if message=="workbuddy.error_body_timeout")
    );
    assert_eq!(t.count(), 1);
}

struct DelayedProfile {
    immediate: Fake,
    delayed: Arc<DelayedResponse>,
}
impl Transport for DelayedProfile {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        if matches!(request.endpoint, Endpoint::Profile) {
            self.delayed.request(request)
        } else {
            self.immediate.request(request)
        }
    }
}
fn delayed_signin(uid: &str) -> DelayedProfile {
    let mut replies = signin_replies(uid, "www.workbuddy.cn");
    let profile = replies.pop().unwrap();
    DelayedProfile {
        immediate: Fake::with(replies),
        delayed: DelayedResponse::new(profile),
    }
}
#[cfg_attr(test, tokio::test)]
async fn cancelled_pending_profile_late_response_cannot_publish_grant() {
    let f = Fixture::new(false);
    let t = Arc::new(delayed_signin("synthetic-a"));
    let mut pending =
        PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], t.as_ref(), 1000)
            .await
            .unwrap();
    let cancellation = pending.cancel_handle();
    let transport = t.clone();
    let task =
        tokio::spawn(async move { pending.poll_protocol_with(transport.as_ref(), 2000).await });
    t.delayed.entered.notified().await;
    tokio::time::timeout(Duration::from_secs(1), cancellation.cancel())
        .await
        .unwrap();
    t.delayed.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(Failure::ExpiredAuthorization)
    ));
    assert!(
        Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
            .await
            .is_err()
    );
    assert!(
        Lease::acquire_with_expected(f.session.clone(), &f.ids[1], true, None)
            .await
            .is_err()
    );
}
#[cfg_attr(test, tokio::test)]
async fn closing_old_flow_cannot_cancel_reopened_flow_or_other_original_account() {
    let f = Fixture::new(false);
    let t = Fake::with(signin_replies("synthetic-a", "www.workbuddy.cn"));
    let mut old = PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[0], &t, 1000)
        .await
        .unwrap();
    let old_cancel = old.cancel_handle();
    old_cancel.cancel().await;
    assert!(matches!(
        old.poll_protocol_with(&t, 2000).await,
        Err(Failure::ExpiredAuthorization)
    ));
    assert_eq!(t.count(), 1);
    for (index, uid) in [(0, "synthetic-a"), (1, "synthetic-b")] {
        let t = Fake::with(signin_replies(uid, "www.workbuddy.cn"));
        let mut new =
            PendingAuthorization::begin_protocol_with(f.session.clone(), &f.ids[index], &t, 2000)
                .await
                .unwrap();
        old_cancel.cancel().await;
        assert!(matches!(
            new.poll_protocol_with(&t, 3000).await,
            Ok(AuthorizationProgress::Saved)
        ));
        assert!(
            Lease::acquire_with_expected(f.session.clone(), &f.ids[index], true, None)
                .await
                .is_ok()
        );
    }
}
#[cfg_attr(test, tokio::test)]
async fn cancel_publish_race_linearizes_and_cancel_never_revokes_published_grant() {
    // Cover both lock orders, and the contended boundary rather than timing sleeps.
    for cancel_first in [true, false] {
        let f = Fixture::new(false);
        let t = Arc::new(delayed_signin("synthetic-a"));
        let mut pending = PendingAuthorization::begin_protocol_with(
            f.session.clone(),
            &f.ids[0],
            t.as_ref(),
            1000,
        )
        .await
        .unwrap();
        let handle = pending.cancel_handle();
        let transport = t.clone();
        let task =
            tokio::spawn(async move { pending.poll_protocol_with(transport.as_ref(), 2000).await });
        t.delayed.entered.notified().await;
        if cancel_first {
            let lock = crate::services::sync_protocol::sync_mutex().lock().await;
            let cancel = tokio::spawn(async move { handle.cancel().await });
            tokio::task::yield_now().await;
            t.delayed.release.notify_one();
            drop(lock);
            cancel.await.unwrap();
            assert!(matches!(
                task.await.unwrap(),
                Err(Failure::ExpiredAuthorization)
            ));
            assert!(
                Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
                    .await
                    .is_err()
            );
        } else {
            t.delayed.release.notify_one();
            assert!(matches!(
                task.await.unwrap(),
                Ok(AuthorizationProgress::Saved)
            ));
            handle.cancel().await;
            assert!(
                Lease::acquire_with_expected(f.session.clone(), &f.ids[0], true, None)
                    .await
                    .is_ok()
            );
        }
    }
}
#[cfg_attr(test, tokio::test)]
async fn inference_status_uses_only_original_private_grant_and_safe_dto() {
    let f = Fixture::new(false);
    let states = list_inference_accounts(&f.session).await.unwrap();
    assert!(states.iter().all(|row| row.state == InferenceState::Paused));
    let f = Fixture::new(true);
    f.change(|rows| {
        rows[0].inference.as_mut().unwrap().expires_ms = now_ms() + 120_000;
        rows[1].inference.as_mut().unwrap().refresh_expires_ms = Some(now_ms() - 1);
    });
    let states = list_inference_accounts(&f.session).await.unwrap();
    assert_eq!(states[0].state, InferenceState::Paused);
    assert_eq!(states[1].state, InferenceState::Paused);
    let json = serde_json::to_value(&states).unwrap();
    assert_eq!(json[0].as_object().unwrap().len(), 2);
    assert_eq!(json[0]["id"], f.ids[0]);
    let rendered = json.to_string();
    for private in ["synthetic-a", "new-inference", "refresh-", "old-checkin"] {
        assert!(!rendered.contains(private));
    }
    f.change(|rows| rows[0].inference.as_mut().unwrap().expires_ms = 1);
    f.change(|rows| rows[0].inference.as_mut().unwrap().refresh_expires_ms = None);
    assert_eq!(
        list_inference_accounts(&f.session).await.unwrap()[0].state,
        InferenceState::Paused
    );
    f.change(|rows| rows[0].inference.as_mut().unwrap().platform = "workbuddy-ai".into());
    assert_eq!(
        list_inference_accounts(&f.session).await.unwrap()[0].state,
        InferenceState::Paused
    );
}
#[cfg_attr(test, tokio::test)]
async fn discovery_rejects_secret_echo_in_decoded_model_id_or_label_and_nonidentity() {
    for (id, name) in [
        ("new-inference-a", "Normal"),
        ("model", "prefix refresh-a suffix"),
    ] {
        let f = Fixture::new(true);
        let t = Fake::with(vec![json_reply(
            200,
            json!({"code":0,"data":{"agents":[{"name":"cli","models":[id]}],"models":[{"id":id,"name":name}]}}),
        )]);
        assert!(f
            .lease(0)
            .await
            .models_protocol_with(&t, 1000)
            .await
            .is_err());
        assert_eq!(t.count(), 1);
    }
    let f = Fixture::new(true);
    let mut h = http::HeaderMap::new();
    h.insert("content-encoding", http::HeaderValue::from_static("gzip"));
    let t = Fake::with(vec![ProxyResponse::buffered(
        http::StatusCode::OK,
        h,
        Bytes::from_static(b"{\"code\":0,\"data\":{\"agents\":[]}}"),
    )]);
    assert!(f
        .lease(0)
        .await
        .models_protocol_with(&t, 1000)
        .await
        .is_err());
}

fn event(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}
fn delta(value: Value) -> Bytes {
    event(json!({"choices":[{"index":0,"delta":value}]}))
}
fn done() -> Bytes {
    Bytes::from_static(b"data: [DONE]\n\n")
}
async fn semantic_case(chunks: Vec<Bytes>, blocked: bool) -> Vec<u8> {
    let f = Fixture::new(true);
    let t = Fake::with(vec![ProxyResponse::streamed(
        http::StatusCode::OK,
        safe_headers(&http::HeaderMap::new(), true),
        futures::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>)),
    )]);
    let out = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    let mut stream = Box::pin(out.response.bytes_stream());
    let mut output = vec![];
    let mut failed = false;
    while let Some(result) = stream.next().await {
        match result {
            Ok(bytes) => output.extend_from_slice(&bytes),
            Err(_) => failed = true,
        }
    }
    assert_eq!(failed, blocked);
    assert_eq!(
        matches!(out.fence.check().await, Err(ProxyError::Internal(_))),
        blocked
    );
    assert_eq!(t.count(), 1);
    output
}
#[cfg_attr(test, tokio::test)]
async fn decoded_unicode_first_event_sets_terminal_opaque_fence_without_output() {
    let encoded = br#"data: {"choices":[{"delta":{"content":"\u006e\u0065\u0077\u002d\u0069\u006e\u0066\u0065\u0072\u0065\u006e\u0063\u0065\u002d\u0061"}}]}

"#;
    assert!(
        semantic_case(vec![Bytes::from_static(encoded), done()], true)
            .await
            .is_empty()
    );
}
#[cfg_attr(test, tokio::test)]
async fn decoded_secret_split_across_delta_events_holds_original_prefix() {
    let bytes = [
        delta(json!({"content":"new-infer"})),
        delta(json!({"content":"ence-a"})),
        done(),
    ]
    .concat();
    for split in [1, 15, bytes.len() / 2, bytes.len() - 1] {
        assert!(semantic_case(
            vec![
                Bytes::copy_from_slice(&bytes[..split]),
                Bytes::copy_from_slice(&bytes[split..])
            ],
            true
        )
        .await
        .is_empty());
    }
}
#[cfg_attr(test, tokio::test)]
async fn reasoning_aliases_and_original_trimmed_tool_reasoning_are_screened() {
    for chunks in [
        vec![
            delta(json!({"reasoning_content":"new-infer"})),
            delta(json!({"reasoning":"ence-a"})),
            done(),
        ],
        vec![
            delta(json!({"reasoning":{"text":"new-infer"}})),
            delta(json!({"reasoning_details":[{"text":"ence-a"}]})),
            done(),
        ],
        vec![
            delta(json!({"reasoning_content":"new-infer "})),
            delta(
                json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":"{}"}}]}),
            ),
            delta(json!({"reasoning":"ence-a"})),
            done(),
        ],
        vec![
            delta(json!({"content":"<think>new-infer </think>"})),
            delta(
                json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":"{}"}}]}),
            ),
            delta(json!({"reasoning":"ence-a"})),
            done(),
        ],
    ] {
        semantic_case(chunks, true).await;
    }
}
#[cfg_attr(test, tokio::test)]
async fn nested_unicode_tool_arguments_are_held_until_complete_and_screened() {
    let args = r#"{"input":"\u006e\u0065\u0077-inference-a"}"#;
    let middle = args.len() / 2;
    let first = delta(
        json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":&args[..middle]}}]}),
    );
    let second =
        delta(json!({"tool_calls":[{"index":0,"function":{"arguments":&args[middle..]}}]}));
    assert!(semantic_case(vec![first, second, done()], true)
        .await
        .is_empty());
}
#[cfg_attr(test, tokio::test)]
async fn independent_tools_and_content_fields_never_join_into_false_secret() {
    let chunks = vec![
        delta(json!({"content":"new-infer"})),
        delta(json!({"reasoning_content":"ence-a"})),
        delta(
            json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":"{\"x\":\"new-infer\"}"}},{"index":1,"id":"call_1","function":{"name":"lookup","arguments":"{\"x\":\"ence-a\"}"}}]}),
        ),
        event(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})),
        done(),
    ];
    let expected = chunks.concat();
    assert_eq!(semantic_case(chunks, false).await, expected);
}
#[cfg_attr(test, tokio::test)]
async fn semantic_screen_preserves_normal_multibyte_and_line_endings_byte_for_byte() {
    let raw = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"你好，正常结果\"}}]}\r\n\n:data heartbeat\n\ndata: [DONE]\r\n\r\n".as_bytes().to_vec();
    for split in [1, 60, 62, raw.len() - 2] {
        assert_eq!(
            semantic_case(
                vec![
                    Bytes::copy_from_slice(&raw[..split]),
                    Bytes::copy_from_slice(&raw[split..])
                ],
                false
            )
            .await,
            raw
        );
    }
}
#[cfg_attr(test, tokio::test)]
async fn single_choice_capability_rejects_n_before_network_and_other_choice_on_response() {
    for n in [json!(0), json!(2), json!("1")] {
        let f = Fixture::new(true);
        let t = Fake::default();
        let mut request = body();
        request["n"] = n;
        assert!(matches!(
            f.lease(0)
                .await
                .dispatch_protocol_with(request, Duration::from_secs(3), &t, 1000)
                .await,
            Err(ProxyError::InvalidRequest(_))
        ));
        assert_eq!(t.count(), 0);
    }
    for choices in [
        json!([{"index":1,"delta":{"content":"other"}}]),
        json!([{"index":0,"delta":{}},{"index":1,"delta":{}}]),
    ] {
        assert!(
            semantic_case(vec![event(json!({"choices":choices})), done()], true)
                .await
                .is_empty()
        );
    }
}
#[cfg_attr(test, tokio::test)]
async fn event_and_total_holdback_limits_fail_closed_with_terminal_fence() {
    semantic_case(
        vec![Bytes::from(format!("data: {}\n\n", "x".repeat(256 * 1024)))],
        true,
    )
    .await;
    let mut held = vec![delta(json!({"content":"new-infer"}))];
    held.extend((0..20).map(|_| Bytes::from(format!(":{}\n\n", " ".repeat(64 * 1024)))));
    assert!(semantic_case(held, true).await.is_empty());
}
#[cfg_attr(test, tokio::test)]
async fn malformed_sse_or_incomplete_tool_json_never_becomes_successful_eof() {
    for chunks in [
        vec![Bytes::from_static(b"data: {broken}\n\n")],
        vec![Bytes::from_static(b"data: {\"choices\":[]}")],
        vec![
            delta(
                json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"x\":\"unfinished"}}]}),
            ),
            done(),
        ],
    ] {
        semantic_case(chunks, true).await;
    }
}

#[cfg_attr(test, tokio::test)]
async fn clean_eof_without_a_terminal_signal_is_not_success() {
    for chunks in [
        vec![],
        vec![Bytes::from_static(b": heartbeat\n\n")],
        vec![delta(json!({"role":"assistant"}))],
        vec![delta(json!({"content":"partial response"}))],
        vec![delta(json!({"reasoning_content":"partial reasoning"}))],
        vec![event(json!({"choices":[],"usage":{"total_tokens":1}}))],
        vec![event(
            json!({"choices":[{"delta":{},"finish_reason":false}]}),
        )],
        vec![event(json!({"choices":[{"delta":{},"finish_reason":""}]}))],
    ] {
        semantic_case(chunks, true).await;
    }
}

#[cfg_attr(test, tokio::test)]
async fn explicit_terminal_signals_preserve_clean_eof_wire_bytes() {
    for terminal in [
        done(),
        event(json!({"choices":[{"delta":{},"finish_reason":"stop"}]})),
        event(json!({"choices":[{"delta":{},"finish_reason":"length"}]})),
    ] {
        let chunks = vec![delta(json!({"content":"complete response"})), terminal];
        let expected = chunks.concat();
        assert_eq!(semantic_case(chunks, false).await, expected);
    }
}

#[cfg_attr(test, tokio::test)]
async fn finish_reason_only_keeps_usage_tail_and_completed_tools() {
    for chunks in [
        vec![
            delta(json!({"content":"complete response"})),
            event(json!({"choices":[{"delta":{},"finish_reason":"stop"}]})),
            event(json!({"choices":[],"usage":{"total_tokens":2}})),
        ],
        vec![
            delta(
                json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":"{}"}}]}),
            ),
            event(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})),
        ],
    ] {
        let expected = chunks.concat();
        assert_eq!(semantic_case(chunks, false).await, expected);
    }
}

#[cfg_attr(test, tokio::test)]
async fn split_leading_think_and_reasoning_alias_follow_original_late_parse_order() {
    let blocked = vec![
        delta(json!({"reasoning_content":"new-infer"})),
        delta(json!({"content":"<thi"})),
        delta(json!({"content":"nk>ence-a</think>normal"})),
        done(),
    ];
    assert!(semantic_case(blocked, true).await.is_empty());
    // Inline text is parsed only on close; an intervening alias comes FIRST in
    // the real converter's reasoning string, not the wire arrival of the prefix.
    let safe = vec![
        delta(json!({"content":"<think>new-infer"})),
        delta(json!({"reasoning_content":"ence-a"})),
        delta(json!({"content":"</think>normal"})),
        done(),
    ];
    let expected = safe.concat();
    assert_eq!(semantic_case(safe, false).await, expected);
}
#[cfg_attr(test, tokio::test)]
async fn completed_tool_rejects_late_argument_fragments_without_reinterpretation() {
    semantic_case(vec![
        delta(json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":"{}"}}]})),
        event(json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]})),
        delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"late"}}]})), done(),
    ], true).await;
}

#[cfg_attr(test, tokio::test)]
async fn structured_sse_denials_preserve_safe_terminal_meaning_before_and_after_output() {
    for code in [11004, 11140] {
        for envelope in [
            json!({"code":code,"msg":"untrusted-upstream-echo"}),
            json!({"data":{"code":code.to_string(),"msg":"untrusted-upstream-echo"}}),
            json!({"error":{"code":code,"message":"untrusted-upstream-echo"}}),
            json!({"code":503,"error":{"data":{"code":code.to_string(),"msg":"untrusted-upstream-echo"}}}),
        ] {
            for late in [false, true] {
                let f = Fixture::new(true);
                let first = delta(json!({"content":"normal visible output"}));
                let mut chunks = Vec::new();
                if late {
                    chunks.push(first.clone());
                }
                chunks.extend([
                    event(envelope.clone()),
                    delta(json!({"content":"must not follow refusal"})),
                    done(),
                ]);
                let t = Fake::with(vec![ProxyResponse::streamed(
                    http::StatusCode::OK,
                    safe_headers(&http::HeaderMap::new(), true),
                    futures::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>)),
                )]);
                let result = f
                    .lease(0)
                    .await
                    .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
                    .await
                    .unwrap();
                let mut stream = result.response.bytes_stream();
                let mut output = Vec::new();
                let failure = loop {
                    match stream
                        .next()
                        .await
                        .expect("refusal must not become successful EOF")
                    {
                        Ok(chunk) => output.extend_from_slice(&chunk),
                        Err(error) => break error.to_string(),
                    }
                };
                assert!(stream.next().await.is_none());
                assert_eq!(output, if late { first.to_vec() } else { vec![] });
                assert_eq!(t.count(), 1);
                let error = result.fence.check().await.unwrap_err();
                let ProxyError::UpstreamError {
                    status,
                    body: Some(body),
                } = error
                else {
                    panic!("refusal lost its safe type");
                };
                assert_eq!(status, 403);
                let value: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(value["error"]["type"], "access_denied");
                assert_eq!(value["error"]["code"], format!("workbuddy_{code}"));
                assert_eq!(value["error"]["upstream_status"], 200);
                let meaning = if code == 11004 {
                    "unapproved channel"
                } else {
                    "unusual activity"
                };
                assert!(body.contains(meaning));
                assert!(failure.contains(meaning));
                for text in [body, failure] {
                    assert!(!text.contains("untrusted-upstream-echo"));
                }
            }
        }
    }
}

#[cfg_attr(test, tokio::test)]
async fn structured_sse_denial_detection_never_matches_messages_or_code_prefixes() {
    let chunks = vec![
        event(json!({"code":111400,"msg":"11140 unapproved channel"})),
        delta(json!({"content":"11004 unusual activity are literal model text"})),
        done(),
    ];
    let expected = chunks.concat();
    assert_eq!(semantic_case(chunks, false).await, expected);
}

#[cfg_attr(test, tokio::test)]
async fn structured_sse_typed_fence_keeps_retirement_terminal_not_auth_retry() {
    let f = Fixture::new(true);
    let t = Fake::with(vec![success()]);
    let result = f
        .lease(0)
        .await
        .dispatch_protocol_with(body(), Duration::from_secs(3), &t, 1000)
        .await
        .unwrap();
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    assert!(matches!(
        result.fence.check().await,
        Err(ProxyError::Internal(_))
    ));
    assert!(result.response.bytes_with_limit(4096).await.is_err());
    assert_eq!(t.count(), 1);
}

#[cfg_attr(test, tokio::test)]
async fn workbuddy_finish_reason_rejects_late_content_reasoning_role_or_new_tool() {
    let first = delta(json!({"content":"visible partial output"}));
    let finish = event(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}));
    for late in [
        delta(json!({"content":"late content"})),
        delta(json!({"reasoning_content":"late reasoning"})),
        delta(json!({"role":"assistant"})),
        delta(
            json!({"tool_calls":[{"index":1,"id":"call_late","function":{"name":"lookup","arguments":"{}"}}]}),
        ),
    ] {
        let emitted = semantic_case(vec![first.clone(), finish.clone(), late], true).await;
        assert_eq!(emitted, first);
    }
}
#[cfg_attr(test, tokio::test)]
async fn workbuddy_finish_reason_rejects_general_error_frames() {
    let finish = event(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}));
    for error in [
        event(json!({"error":{"type":"server_error","code":503,"message":"synthetic failure"}})),
        Bytes::from_static(b"event: error\ndata: {\"message\":\"synthetic failure\"}\n\n"),
        event(json!({"error":{"type":"server_error"},"choices":[],"usage":{"total_tokens":1}})),
    ] {
        assert!(semantic_case(vec![finish.clone(), error], true)
            .await
            .is_empty());
    }
}
#[cfg_attr(test, tokio::test)]
async fn workbuddy_done_is_withheld_until_the_following_tail_is_checked() {
    let first = delta(json!({"content":"visible partial output"}));
    let emitted = semantic_case(
        vec![
            first.clone(),
            done(),
            event(json!({"error":{"type":"server_error","message":"late failure"}})),
        ],
        true,
    )
    .await;
    assert_eq!(
        emitted, first,
        "DONE must not escape before the native EOF is validated"
    );
}
#[cfg_attr(test, tokio::test)]
async fn workbuddy_finished_tools_usage_and_done_keep_their_original_wire_order() {
    for (first, reason) in [
        (delta(json!({"content":"complete answer"})), "stop"),
        (
            delta(
                json!({"tool_calls":[{"index":0,"id":"call_0","function":{"name":"lookup","arguments":"{}"}}]}),
            ),
            "tool_calls",
        ),
    ] {
        let chunks = vec![
            first,
            event(json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]})),
            event(
                json!({"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}),
            ),
            done(),
            Bytes::from_static(b": inert comment\n\n"),
        ];
        let wire = chunks.concat();
        assert_eq!(semantic_case(chunks, false).await, wire);
        assert_eq!(
            semantic_case(wire.chunks(1).map(Bytes::copy_from_slice).collect(), false).await,
            wire
        );
    }
}

#[cfg_attr(test, tokio::test)]
async fn expiry_owner_model_discovery_and_meter_join_require_same_credential_generation() {
    use crate::relay::entitlement_expiry::*;
    let f = Fixture::new(true);
    let t = Fake::with(vec![json_reply(
        200,
        json!({"code":0,"data":{
        "agents":[{"name":"cli","models":["m","free"]}],
        "models":[{"id":"m","credits":"x0.03"},{"id":"free","credits":"0"}],"modelPromotions":[]}}),
    )]);
    f.lease(0)
        .await
        .models_protocol_with(&t, 1000)
        .await
        .unwrap();
    f.change(|rows| {
        let empty=json!({"code":0,"data":{"Accounts":[],"TotalCount":0}});
        let summary=json!({"code":0,"data":{"Packages":[{"PackageCode":"p","CapacityUnit":"credits"}]}});
        let paid=json!({"code":0,"data":{"Accounts":[
            {"ResourceId":"later","PackageCode":"p","CapacityRemainPrecise":4,"DeductionEndTime":"2026-10-26T00:00:00Z"},
            {"ResourceId":"earlier","PackageCode":"p","CapacityRemainPrecise":2,"DeductionEndTime":"2026-10-25T00:00:00Z"}],"TotalCount":100}});
        rows[0].credits=super::super::credits::normalize(&summary,&paid,&empty,1000);
        rows[0].credits.observed_generation=Some(rows[0].generation);
    });
    let rank = |candidates| {
        project_and_order(
            candidates,
            &["m".into()],
            Policy::EarliestExpiry,
            1000,
            5000,
        )
    };
    let candidates = super::super::credits::expiry_candidates(&f.session, &f.ids[0], "m", 1000)
        .await
        .unwrap();
    let ranked = rank(candidates);
    assert_eq!(ranked[0].candidate.bucket_id, "earlier");
    assert!(matches!(ranked[0].priority, Priority::KnownExpiryMs(_)));
    let free = super::super::credits::expiry_candidates(&f.session, &f.ids[0], "free", 1000)
        .await
        .unwrap();
    assert!(
        project_and_order(free, &["free".into()], Policy::EarliestExpiry, 1000, 5000)
            .iter()
            .all(|c| matches!(c.priority, Priority::Unknown(_)))
    );
    assert!(
        super::super::credits::expiry_candidates(&f.session, &f.ids[1], "m", 1000)
            .await
            .unwrap()
            .is_empty()
    );
    let binding = store::binding(&f.session).unwrap();
    let original = store::load(&f.session, &binding).unwrap()[0]
        .credits
        .clone();
    for exclusion in ["unit", "daily", "slice", "unreported"] {
        f.change(|rows| {
            rows[0].credits = original.clone();
            for observation in &mut rows[0].credits.expiry_observations {
                match exclusion {
                    "unit" => observation.capacity_unit = Some(json!("tokens")),
                    "daily" => observation.package_code = Some("TCACA_code_001_PqouKr6QWV".into()),
                    "slice" => observation.capacity_type = Some(json!(4)),
                    _ if observation.source != "summary" => {
                        observation.package_code = Some("unreported".into());
                        observation.capacity_unit = Some(json!("credits"));
                    }
                    _ => {}
                }
            }
        });
        let candidates = super::super::credits::expiry_candidates(&f.session, &f.ids[0], "m", 1000)
            .await
            .unwrap();
        assert!(
            rank(candidates)
                .iter()
                .all(|c| matches!(c.priority, Priority::Unknown(_))),
            "{exclusion}"
        );
    }
    f.change(|rows| rows[0].credits = original);
    f.change(|rows| rows[0].generation += 1);
    assert!(rank(
        super::super::credits::expiry_candidates(&f.session, &f.ids[0], "m", 1000)
            .await
            .unwrap()
    )
    .iter()
    .all(|c| matches!(c.priority, Priority::Unknown(_))));
}

async fn expiry_review_snapshot_case(future: bool) {
    use crate::relay::entitlement_expiry::*;
    let f = Fixture::new(true);
    let t = Fake::with(vec![json_reply(
        200,
        json!({"code":0,"data":{
        "agents":[{"name":"cli","models":["m"]}],"models":[{"id":"m","credits":"x0.03"}],"modelPromotions":[]}}),
    )]);
    f.lease(0)
        .await
        .models_protocol_with(&t, 1000)
        .await
        .unwrap();
    f.change(|rows| {
        rows[0].credits=super::super::credits::normalize(
            &json!({"code":0,"data":{"Packages":[{"PackageCode":"p","CapacityUnit":"credits"}]}}),
            &json!({"code":0,"data":{"Accounts":[{"ResourceId":"r","PackageCode":"p","CapacityRemainPrecise":4,"DeductionEndTime":"2026-10-26T00:00:00Z"}]}}),&json!({}),1000);
        if future { rows[0].credit_models.as_mut().unwrap().observed_at_ms=2000; }
        else { rows[0].generation+=1; }
        rows[0].credits.observed_generation=Some(rows[0].generation);
    });
    let candidates = super::super::credits::expiry_candidates(&f.session, &f.ids[0], "m", 1000)
        .await
        .unwrap();
    let ranked = project_and_order(
        candidates,
        &["m".into()],
        Policy::EarliestExpiry,
        1000,
        5000,
    );
    assert!(ranked
        .iter()
        .all(|c| matches!(c.priority, Priority::Unknown(_))));
}
#[cfg_attr(test, tokio::test)]
async fn expiry_review_future_model_observation_cannot_hide_behind_older_credits() {
    expiry_review_snapshot_case(true).await;
}
#[cfg_attr(test, tokio::test)]
async fn expiry_review_new_meter_generation_does_not_relabel_old_model_snapshot() {
    expiry_review_snapshot_case(false).await;
}

// Only the external small target calls this; cfg(test) still registers every
// original tokio test unchanged, without nested test-runtime invocation.
#[cfg(all(feature = "test-hooks", not(test)))]
pub(crate) async fn verify_private_protocol(case: &str) {
    match case {
        "two_selected_accounts_keep_distinct_new_grants_and_preserve_body" => {
            two_selected_accounts_keep_distinct_new_grants_and_preserve_body().await
        }
        "legacy_rows_are_not_inference_authorized_and_keep_existing_ui_dto" => {
            legacy_rows_are_not_inference_authorized_and_keep_existing_ui_dto().await
        }
        "explicit_new_grant_binds_only_matching_original_row_and_keeps_checkin" => {
            explicit_new_grant_binds_only_matching_original_row_and_keeps_checkin().await
        }
        "wrong_uid_or_domain_or_platform_never_binds_or_migrates" => {
            wrong_uid_or_domain_or_platform_never_binds_or_migrates().await
        }
        "stale_login_flow_cannot_replace_a_newer_grant" => {
            stale_login_flow_cannot_replace_a_newer_grant().await
        }
        "unsafe_authorization_urls_are_rejected_without_following" => {
            unsafe_authorization_urls_are_rejected_without_following().await
        }
        "models_use_only_live_cli_ids_without_static_fallback_or_pricing" => {
            models_use_only_live_cli_ids_without_static_fallback_or_pricing().await
        }
        "explicit_401_refreshes_same_grant_once_and_never_sends_old_checkin_token" => {
            explicit_401_refreshes_same_grant_once_and_never_sends_old_checkin_token().await
        }
        "expiry_refresh_uses_the_only_replay_budget_and_preserves_second_401" => {
            expiry_refresh_uses_the_only_replay_budget_and_preserves_second_401().await
        }
        "concurrent_refreshes_spend_a_refresh_token_once" => {
            concurrent_refreshes_spend_a_refresh_token_once().await
        }
        "refresh_identity_mismatch_is_rejected_without_overwriting" => {
            refresh_identity_mismatch_is_rejected_without_overwriting().await
        }
        "refusals_are_terminal_structured_and_never_rewrite_replay_or_rotate" => {
            refusals_are_terminal_structured_and_never_rewrite_replay_or_rotate().await
        }
        "status_429_and_retry_after_are_preserved_but_secret_headers_are_removed" => {
            status_429_and_retry_after_are_preserved_but_secret_headers_are_removed().await
        }
        "success_headers_are_allowlisted_and_sse_bytes_unchanged" => {
            success_headers_are_allowlisted_and_sse_bytes_unchanged().await
        }
        "final_fence_rejects_previously_primed_or_buffered_bytes_after_retirement" => {
            final_fence_rejects_previously_primed_or_buffered_bytes_after_retirement().await
        }
        "blocked_then_unblocked_session_never_revives_old_lease" => {
            blocked_then_unblocked_session_never_revives_old_lease().await
        }
        "post_output_stream_failure_is_sanitized_and_never_replayed" => {
            post_output_stream_failure_is_sanitized_and_never_replayed().await
        }
        "nonstream_request_is_rejected_before_network_instead_of_silently_rewritten" => {
            nonstream_request_is_rejected_before_network_instead_of_silently_rewritten().await
        }
        "response_echo_across_chunks_is_withheld_then_terminated_without_replay" => {
            response_echo_across_chunks_is_withheld_then_terminated_without_replay().await
        }
        "declared_compression_is_rejected_and_requests_ask_identity" => {
            declared_compression_is_rejected_and_requests_ask_identity().await
        }
        "fixture_origin_and_owner_mismatch_never_fall_back_to_production" => {
            fixture_origin_and_owner_mismatch_never_fall_back_to_production().await
        }
        "every_documented_denial_envelope_blocks_401_refresh_and_preserves_exact_codes" => {
            every_documented_denial_envelope_blocks_401_refresh_and_preserves_exact_codes().await
        }
        "pending_refresh_is_single_flight_and_second_lease_reads_persisted_rotation" => {
            pending_refresh_is_single_flight_and_second_lease_reads_persisted_rotation().await
        }
        "late_refresh_response_cannot_revive_retired_original_grant" => {
            late_refresh_response_cannot_revive_retired_original_grant().await
        }
        "waiting_token_response_after_session_epoch_change_is_not_published" => {
            waiting_token_response_after_session_epoch_change_is_not_published().await
        }
        "incomplete_error_body_has_a_bounded_terminal_deadline_without_refresh" => {
            incomplete_error_body_has_a_bounded_terminal_deadline_without_refresh().await
        }
        "cancelled_pending_profile_late_response_cannot_publish_grant" => {
            cancelled_pending_profile_late_response_cannot_publish_grant().await
        }
        "closing_old_flow_cannot_cancel_reopened_flow_or_other_original_account" => {
            closing_old_flow_cannot_cancel_reopened_flow_or_other_original_account().await
        }
        "cancel_publish_race_linearizes_and_cancel_never_revokes_published_grant" => {
            cancel_publish_race_linearizes_and_cancel_never_revokes_published_grant().await
        }
        "inference_status_uses_only_original_private_grant_and_safe_dto" => {
            inference_status_uses_only_original_private_grant_and_safe_dto().await
        }
        "discovery_rejects_secret_echo_in_decoded_model_id_or_label_and_nonidentity" => {
            discovery_rejects_secret_echo_in_decoded_model_id_or_label_and_nonidentity().await
        }
        "decoded_unicode_first_event_sets_terminal_opaque_fence_without_output" => {
            decoded_unicode_first_event_sets_terminal_opaque_fence_without_output().await
        }
        "decoded_secret_split_across_delta_events_holds_original_prefix" => {
            decoded_secret_split_across_delta_events_holds_original_prefix().await
        }
        "reasoning_aliases_and_original_trimmed_tool_reasoning_are_screened" => {
            reasoning_aliases_and_original_trimmed_tool_reasoning_are_screened().await
        }
        "nested_unicode_tool_arguments_are_held_until_complete_and_screened" => {
            nested_unicode_tool_arguments_are_held_until_complete_and_screened().await
        }
        "independent_tools_and_content_fields_never_join_into_false_secret" => {
            independent_tools_and_content_fields_never_join_into_false_secret().await
        }
        "semantic_screen_preserves_normal_multibyte_and_line_endings_byte_for_byte" => {
            semantic_screen_preserves_normal_multibyte_and_line_endings_byte_for_byte().await
        }
        "single_choice_capability_rejects_n_before_network_and_other_choice_on_response" => {
            single_choice_capability_rejects_n_before_network_and_other_choice_on_response().await
        }
        "event_and_total_holdback_limits_fail_closed_with_terminal_fence" => {
            event_and_total_holdback_limits_fail_closed_with_terminal_fence().await
        }
        "malformed_sse_or_incomplete_tool_json_never_becomes_successful_eof" => {
            malformed_sse_or_incomplete_tool_json_never_becomes_successful_eof().await
        }
        "clean_eof_without_a_terminal_signal_is_not_success" => {
            clean_eof_without_a_terminal_signal_is_not_success().await
        }
        "explicit_terminal_signals_preserve_clean_eof_wire_bytes" => {
            explicit_terminal_signals_preserve_clean_eof_wire_bytes().await
        }
        "finish_reason_only_keeps_usage_tail_and_completed_tools" => {
            finish_reason_only_keeps_usage_tail_and_completed_tools().await
        }
        "split_leading_think_and_reasoning_alias_follow_original_late_parse_order" => {
            split_leading_think_and_reasoning_alias_follow_original_late_parse_order().await
        }
        "completed_tool_rejects_late_argument_fragments_without_reinterpretation" => {
            completed_tool_rejects_late_argument_fragments_without_reinterpretation().await
        }
        "structured_sse_denials_preserve_safe_terminal_meaning_before_and_after_output" => {
            structured_sse_denials_preserve_safe_terminal_meaning_before_and_after_output().await
        }
        "structured_sse_denial_detection_never_matches_messages_or_code_prefixes" => {
            structured_sse_denial_detection_never_matches_messages_or_code_prefixes().await
        }
        "structured_sse_typed_fence_keeps_retirement_terminal_not_auth_retry" => {
            structured_sse_typed_fence_keeps_retirement_terminal_not_auth_retry().await
        }
        "workbuddy_finish_reason_rejects_late_content_reasoning_role_or_new_tool" => {
            workbuddy_finish_reason_rejects_late_content_reasoning_role_or_new_tool().await
        }
        "workbuddy_finish_reason_rejects_general_error_frames" => {
            workbuddy_finish_reason_rejects_general_error_frames().await
        }
        "workbuddy_done_is_withheld_until_the_following_tail_is_checked" => {
            workbuddy_done_is_withheld_until_the_following_tail_is_checked().await
        }
        "workbuddy_finished_tools_usage_and_done_keep_their_original_wire_order" => {
            workbuddy_finished_tools_usage_and_done_keep_their_original_wire_order().await
        }
        "expiry_owner_model_discovery_and_meter_join_require_same_credential_generation" => {
            expiry_owner_model_discovery_and_meter_join_require_same_credential_generation().await
        }
        "expiry_review_future_model_observation_cannot_hide_behind_older_credits" => {
            expiry_review_future_model_observation_cannot_hide_behind_older_credits().await
        }
        "expiry_review_new_meter_generation_does_not_relabel_old_model_snapshot" => {
            expiry_review_new_meter_generation_does_not_relabel_old_model_snapshot().await
        }
        _ => panic!("unknown private protocol regression"),
    }
}
