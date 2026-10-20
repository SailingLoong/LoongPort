//! These tests use synthetic encrypted accounts and the real MiniMax owner.
//! They are module/transport tests, not ProxyServer/pool integration evidence.
use super::super::inference::{Lease, StreamRequest, StreamTransport};
use super::*;
use crate::proxy::{hyper_client::ProxyResponse, ProxyError};
use bytes::Bytes;
use futures::StreamExt;
use std::{collections::VecDeque, time::Duration};

#[derive(Default)]
struct StreamFixture {
    calls: Mutex<Vec<StreamRequest>>,
    responses: Mutex<VecDeque<ProxyResponse>>,
}
impl StreamTransport for StreamFixture {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProxyError::ForwardFailed("synthetic transport exhausted".into()))
        })
    }
}
fn stream_response(status: u16, chunks: Vec<Result<Bytes, std::io::Error>>) -> ProxyResponse {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/event-stream"),
    );
    ProxyResponse::streamed(
        http::StatusCode::from_u16(status).unwrap(),
        headers,
        futures::stream::iter(chunks),
    )
}
fn success() -> ProxyResponse {
    stream_response(
        200,
        vec![
            Ok(Bytes::from_static(b"event: message_start\ndata: {}\n\n")),
            Ok(Bytes::from_static(b"event: message_stop\ndata: {}\n\n")),
        ],
    )
}
fn request_body() -> Value {
    json!({"model":"synthetic-model","max_tokens":32,"stream":true,"messages":[{"role":"user","content":"synthetic request"}],"tools":[{"name":"lookup","description":"test","input_schema":{"type":"object"}}]})
}
fn two_accounts(f: &Fixture) {
    f.seed(|d| {
        let mut second = account();
        second.id = "synthetic-second".into();
        second.real_user_id = "synthetic-user-second".into();
        second.region = Region::Global;
        second.credentials.as_mut().unwrap().access_token = "synthetic-access-second".into();
        second.credentials.as_mut().unwrap().refresh_token = "synthetic-refresh-second".into();
        d.accounts.push(second);
    });
}
#[tokio::test]
async fn inference_binds_two_accounts_and_preserves_anthropic_body() {
    let f = Fixture::new().await;
    two_accounts(&f);
    let transport = StreamFixture::default();
    transport
        .responses
        .lock()
        .unwrap()
        .extend([success(), success()]);
    for id in ["synthetic-account", "synthetic-second"] {
        let lease = Lease::acquire(f.session.clone(), id, "same-client-session")
            .await
            .unwrap();
        assert_eq!(
            lease.base_url(),
            if id == "synthetic-account" {
                "https://agent.minimax.cn/mavis/api/v1/llm/v1"
            } else {
                "https://agent.minimax.io/mavis/api/v1/llm/v1"
            }
        );
        let dispatch = lease
            .dispatch_with(
                request_body(),
                Duration::from_secs(3),
                &Fake::default(),
                &transport,
            )
            .await
            .unwrap();
        let response = dispatch.response;
        assert_eq!(response.status().as_u16(), 200);
        assert!(!response.bytes_with_limit(4096).await.unwrap().is_empty());
    }
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].url,
        "https://agent.minimax.cn/mavis/api/v1/llm/v1/messages"
    );
    assert_eq!(
        calls[1].url,
        "https://agent.minimax.io/mavis/api/v1/llm/v1/messages"
    );
    for (n, token) in ["Bearer synthetic-access", "Bearer synthetic-access-second"]
        .iter()
        .enumerate()
    {
        assert!(calls[n].streaming);
        assert_eq!(calls[n].timeout, Duration::from_secs(3));
        let h = &calls[n].headers;
        assert_eq!(h[http::header::AUTHORIZATION], *token);
        assert!(h[http::header::AUTHORIZATION].is_sensitive());
        assert_eq!(h[http::header::USER_AGENT], "MiniMaxAgent");
        assert_eq!(h["x-mavis-agent-id"], "main");
        assert_eq!(h["anthropic-version"], "2023-06-01");
        assert!(h["x-mavis-timezone-offset"]
            .to_str()
            .unwrap()
            .parse::<i32>()
            .is_ok());
        assert!(!h.contains_key("x-api-key"));
        assert!(!h.contains_key("bedrock-lane"));
        assert_eq!(
            serde_json::from_slice::<Value>(&calls[n].body).unwrap(),
            request_body()
        );
    }
    assert_ne!(
        calls[0].headers["x-mavis-session-id"],
        calls[1].headers["x-mavis-session-id"]
    );
}
#[derive(Default)]
struct RefreshFixture {
    calls: Mutex<Vec<Request>>,
    wrong_user: bool,
}
impl Transport for RefreshFixture {
    fn request(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            let endpoint = r.endpoint;
            self.calls.lock().unwrap().push(r);
            Ok(match endpoint {
                Endpoint::Refresh => Response {
                    status: 200,
                    body: json!({"access_token":"synthetic-rotated","refresh_token":"synthetic-refresh-rotated","token_type":"Bearer","expires_in":3600,"scope":"agent.default"}),
                },
                Endpoint::Profile => Response {
                    status: 200,
                    body: json!({"data":{"userInfo":{"realUserID":if self.wrong_user {"foreign-user"}else{"synthetic-user"},"name":"Synthetic"}}}),
                },
                _ => panic!("unexpected auth request"),
            })
        })
    }
}
#[tokio::test]
async fn inference_401_refreshes_once_and_never_changes_selected_account() {
    let f = Fixture::new().await;
    two_accounts(&f);
    let transport = StreamFixture::default();
    transport
        .responses
        .lock()
        .unwrap()
        .extend([stream_response(401, vec![]), success()]);
    let auth = RefreshFixture::default();
    let lease = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap();
    let dispatch = lease
        .dispatch_with(request_body(), Duration::from_secs(3), &auth, &transport)
        .await
        .unwrap();
    let response = dispatch.response;
    response.bytes_with_limit(4096).await.unwrap();
    {
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0].headers[http::header::AUTHORIZATION],
            "Bearer synthetic-access"
        );
        assert_eq!(
            calls[1].headers[http::header::AUTHORIZATION],
            "Bearer synthetic-rotated"
        );
        assert_eq!(
            calls[0].headers["x-mavis-session-id"],
            calls[1].headers["x-mavis-session-id"]
        );
        let auth_calls = auth.calls.lock().unwrap();
        assert_eq!(auth_calls.len(), 2);
        assert_eq!(
            auth_calls[0].body.as_ref().unwrap()["refresh_token"],
            "synthetic-refresh"
        );
        assert_eq!(
            auth_calls[1].access_token.as_deref(),
            Some("synthetic-rotated")
        );
    }
    let primary = super::super::catalog::snapshot(&f.session, "synthetic-account")
        .await
        .unwrap();
    let second = super::super::catalog::snapshot(&f.session, "synthetic-second")
        .await
        .unwrap();
    assert_eq!(primary.account.credential_version, 2);
    assert_eq!(second.account.credential_version, 1);
    assert_eq!(
        second.account.credentials.as_ref().unwrap().access_token,
        "synthetic-access-second"
    );
}
#[tokio::test]
async fn inference_repeat_401_is_returned_without_a_third_request_or_body_leak() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let transport = StreamFixture::default();
    transport.responses.lock().unwrap().extend([
        stream_response(401, vec![]),
        stream_response(
            401,
            vec![Ok(Bytes::from_static(b"synthetic-rotated secret echo"))],
        ),
    ]);
    let auth = RefreshFixture::default();
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap()
        .dispatch_with(request_body(), Duration::from_secs(3), &auth, &transport)
        .await
        .unwrap();
    let response = dispatch.response;
    assert_eq!(response.status().as_u16(), 401);
    let body = response.bytes_with_limit(4096).await.unwrap();
    assert!(!String::from_utf8_lossy(&body).contains("synthetic-rotated"));
    assert_eq!(transport.calls.lock().unwrap().len(), 2);
    assert_eq!(auth.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn inference_refresh_rejects_foreign_profile_without_replay_or_mutating_other_account() {
    let f = Fixture::new().await;
    two_accounts(&f);
    let before = f.raw();
    let transport = StreamFixture::default();
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(stream_response(401, vec![]));
    let auth = RefreshFixture {
        wrong_user: true,
        ..Default::default()
    };
    assert!(
        Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
            .await
            .unwrap()
            .dispatch_with(request_body(), Duration::from_secs(3), &auth, &transport)
            .await
            .is_err()
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    assert_eq!(before, f.raw());
}
#[tokio::test]
async fn inference_429_returns_safe_status_and_never_refreshes_or_replays() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let transport = StreamFixture::default();
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::RETRY_AFTER,
        http::HeaderValue::from_static("15"),
    );
    headers.insert(
        "x-secret",
        http::HeaderValue::from_static("synthetic-access"),
    );
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::buffered(
            http::StatusCode::TOO_MANY_REQUESTS,
            headers,
            Bytes::from_static(b"synthetic-access secret echo"),
        ));
    let auth = RefreshFixture::default();
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap()
        .dispatch_with(request_body(), Duration::from_secs(3), &auth, &transport)
        .await
        .unwrap();
    let response = dispatch.response;
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(response.headers()[http::header::RETRY_AFTER], "15");
    assert!(!response.headers().contains_key("x-secret"));
    assert!(
        !String::from_utf8_lossy(&response.bytes_with_limit(4096).await.unwrap())
            .contains("synthetic-access")
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    assert!(auth.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn inference_midstream_error_is_terminal_and_never_replays_output() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let transport = StreamFixture::default();
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(stream_response(
            200,
            vec![
                Ok(Bytes::from_static(b"first output")),
                Err(std::io::Error::other("synthetic-access upstream error")),
                Ok(Bytes::from_static(b"must not pass after error")),
            ],
        ));
    let auth = RefreshFixture::default();
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap()
        .dispatch_with(request_body(), Duration::from_secs(3), &auth, &transport)
        .await
        .unwrap();
    let response = dispatch.response;
    let mut stream = Box::pin(response.bytes_stream());
    assert_eq!(stream.next().await.unwrap().unwrap(), "first output");
    let error = stream.next().await.unwrap().unwrap_err();
    assert!(!error.to_string().contains("synthetic-access"));
    assert!(stream.next().await.is_none());
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
    assert!(auth.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn inference_revocation_and_refresh_version_fence_each_stream_chunk() {
    for change in ["retire", "version", "shutdown", "session"] {
        let f = Fixture::new().await;
        f.seed(|_| {});
        let transport = StreamFixture::default();
        transport
            .responses
            .lock()
            .unwrap()
            .push_back(stream_response(
                200,
                vec![
                    Ok(Bytes::from_static(b"first")),
                    Ok(Bytes::from_static(b"forbidden tail")),
                ],
            ));
        let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
            .await
            .unwrap()
            .dispatch_with(
                request_body(),
                Duration::from_secs(3),
                &Fake::default(),
                &transport,
            )
            .await
            .unwrap();
        let response = dispatch.response;
        let mut stream = Box::pin(response.bytes_stream());
        assert_eq!(stream.next().await.unwrap().unwrap(), "first");
        match change {
            "retire" => {
                Engine {
                    session: &f.session,
                    transport: &Fake::default(),
                }
                .retire("synthetic-account")
                .await
                .unwrap();
            }
            "version" => {
                super::super::catalog::access(&f.session, None, |d, _| {
                    d.accounts[0].credential_version += 1;
                    Ok(((), true))
                })
                .await
                .unwrap();
            }
            "shutdown" => super::super::shutdown(),
            "session" => {
                f.session.set_blocked(true);
                f.session.set_blocked(false);
            }
            _ => unreachable!(),
        }
        assert!(stream.next().await.unwrap().is_err(), "{change}");
        assert!(stream.next().await.is_none());
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }
}
#[tokio::test]
async fn inference_rejects_stale_lease_before_transport_dispatch() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let lease = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap();
    Engine {
        session: &f.session,
        transport: &Fake::default(),
    }
    .retire("synthetic-account")
    .await
    .unwrap();
    let transport = StreamFixture::default();
    assert!(lease
        .dispatch_with(
            request_body(),
            Duration::from_secs(3),
            &Fake::default(),
            &transport
        )
        .await
        .is_err());
    assert!(transport.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn inference_rejects_placeholder_and_invalid_session_without_dispatch() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    for session in ["", "request\r\nAuthorization: attack"] {
        assert!(
            Lease::acquire(f.session.clone(), "synthetic-account", session)
                .await
                .is_err()
        );
    }
    assert!(Lease::acquire(f.session.clone(), "unknown-id", "request")
        .await
        .is_err());
    f.seed(|d| d.accounts[0].credentials.as_mut().unwrap().access_token = "sk-xxx".into());
    assert!(
        Lease::acquire(f.session.clone(), "synthetic-account", "request")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn inference_final_fence_rejects_primed_and_buffered_bytes_after_revocation() {
    for mode in ["primed", "buffered"] {
        let f = Fixture::new().await;
        f.seed(|_| {});
        let transport = StreamFixture::default();
        transport.responses.lock().unwrap().push_back(success());
        let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
            .await
            .unwrap()
            .dispatch_with(
                request_body(),
                Duration::from_secs(3),
                &Fake::default(),
                &transport,
            )
            .await
            .unwrap();
        let replay = if mode == "primed" {
            let mut upstream = Box::pin(dispatch.response.bytes_stream());
            upstream.next().await.unwrap().unwrap()
        } else {
            dispatch.response.bytes_with_limit(4096).await.unwrap()
        };
        assert!(!replay.is_empty());
        Engine {
            session: &f.session,
            transport: &Fake::default(),
        }
        .retire("synthetic-account")
        .await
        .unwrap();
        let final_stream = futures::stream::once(async move { Ok::<_, std::io::Error>(replay) });
        let mut guarded = Box::pin(dispatch.fence.wrap_stream(final_stream));
        assert!(
            guarded.next().await.unwrap().is_err(),
            "{mode} bytes escaped final fence"
        );
        assert!(guarded.next().await.is_none());
    }
}

#[tokio::test]
async fn inference_success_headers_cannot_echo_upstream_credentials() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let transport = StreamFixture::default();
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(
        http::header::CONTENT_ENCODING,
        http::HeaderValue::from_static("identity"),
    );
    for name in [
        "authorization",
        "x-secret",
        "set-cookie",
        "location",
        "x-mavis-session-id",
    ] {
        headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            http::HeaderValue::from_static("synthetic-access"),
        );
    }
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(ProxyResponse::buffered(
            http::StatusCode::OK,
            headers,
            Bytes::from_static(b"synthetic bytes"),
        ));
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap()
        .dispatch_with(
            request_body(),
            Duration::from_secs(3),
            &Fake::default(),
            &transport,
        )
        .await
        .unwrap();
    assert_eq!(dispatch.response.headers().len(), 1);
    assert_eq!(
        dispatch.response.headers()[http::header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert!(!dispatch
        .response
        .headers()
        .contains_key(http::header::CONTENT_ENCODING));
}

#[tokio::test]
async fn inference_router_fixture_rejects_cross_root_and_non_loopback_without_http() {
    use super::super::inference::FixtureTransport;
    let f = Fixture::new().await;
    let owner = FixtureTransport::seed_accounts(&f.session, &[("synthetic-account", Region::Cn)])
        .await
        .unwrap();
    assert!(
        FixtureTransport::seed_accounts(&f.session, &[("synthetic-extra", Region::Cn)])
            .await
            .is_err()
    );
    for origin in [
        "https://127.0.0.1:9",
        "http://upstream.invalid",
        "http://127.0.0.1:9/override",
        "http://user@127.0.0.1:9/",
    ] {
        assert!(FixtureTransport::loopback(&owner, origin).await.is_err());
    }
    let fixture = FixtureTransport::loopback(&owner, "http://127.0.0.1:9")
        .await
        .unwrap();
    let other_root = tempfile::tempdir().unwrap();
    let other_session =
        SecretSession::from_context(other_root.path().into(), VaultContext::generate().unwrap());
    FixtureTransport::seed_accounts(&other_session, &[("synthetic-other", Region::Cn)])
        .await
        .unwrap();
    let foreign = Lease::acquire(other_session, "synthetic-other", "request-one")
        .await
        .unwrap();
    assert!(foreign
        .dispatch_fixture(request_body(), Duration::from_secs(3), &fixture)
        .await
        .is_err());
    assert_eq!(fixture.attempts(), 0);
    let same = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap();
    assert!(fixture.matches(&same));
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    let newer = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap();
    assert!(newer
        .dispatch_fixture(request_body(), Duration::from_secs(3), &fixture)
        .await
        .is_err());
    assert_eq!(fixture.attempts(), 0);
}

#[tokio::test]
async fn inference_expiry_refresh_consumes_the_single_refresh_budget() {
    let f = Fixture::new().await;
    f.seed(|document| {
        document.accounts[0]
            .credentials
            .as_mut()
            .unwrap()
            .expires_at = 1
    });
    let transport = StreamFixture::default();
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(stream_response(401, vec![]));
    let auth = RefreshFixture::default();
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "request-one")
        .await
        .unwrap()
        .dispatch_with(request_body(), Duration::from_secs(3), &auth, &transport)
        .await
        .unwrap();
    assert_eq!(dispatch.response.status().as_u16(), 401);
    assert_eq!(auth.calls.lock().unwrap().len(), 2);
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].headers[http::header::AUTHORIZATION],
        "Bearer synthetic-rotated"
    );
}

#[tokio::test]
async fn inference_blocks_known_credentials_in_success_body_across_chunks() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    for chunks in [
        vec!["event: error\ndata: synthetic-access\n\n"],
        vec!["safe ", "synt", "hetic-", "access", " trailing"],
        vec!["safe synthetic-", "access"],
        vec!["synthetic-refresh"],
    ] {
        let transport = StreamFixture::default();
        transport
            .responses
            .lock()
            .unwrap()
            .push_back(stream_response(
                200,
                chunks
                    .into_iter()
                    .map(|chunk| Ok(Bytes::from(chunk)))
                    .collect(),
            ));
        let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "body-echo")
            .await
            .unwrap()
            .dispatch_with(
                request_body(),
                Duration::from_secs(3),
                &Fake::default(),
                &transport,
            )
            .await
            .unwrap();
        let mut stream = dispatch.response.bytes_stream();
        let mut emitted = Vec::new();
        let mut rejected = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => emitted.extend_from_slice(&bytes),
                Err(error) => {
                    assert!(!error.to_string().contains("synthetic-"));
                    rejected = true;
                }
            }
        }
        assert!(
            rejected,
            "successful status must not expose a known credential"
        );
        assert!(
            !String::from_utf8(emitted).unwrap().contains("synt"),
            "a held credential prefix must not escape before the complete match"
        );
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn inference_preserves_near_matches_and_flushes_normal_eof_tail() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let transport = StreamFixture::default();
    let chunks = ["normal ", "synt", "hetic-accesX ", "synthetic-acces"];
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(stream_response(
            200,
            chunks
                .iter()
                .map(|chunk| Ok(Bytes::copy_from_slice(chunk.as_bytes())))
                .collect(),
        ));
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "body-near-match")
        .await
        .unwrap()
        .dispatch_with(
            request_body(),
            Duration::from_secs(3),
            &Fake::default(),
            &transport,
        )
        .await
        .unwrap();
    assert_eq!(
        dispatch
            .response
            .bytes_with_limit(4096)
            .await
            .unwrap()
            .as_ref(),
        chunks.concat().as_bytes()
    );
}

#[tokio::test]
async fn inference_discards_held_secret_prefix_on_stream_error_without_replay() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    let transport = StreamFixture::default();
    transport
        .responses
        .lock()
        .unwrap()
        .push_back(stream_response(
            200,
            vec![
                Ok(Bytes::from_static(b"safe ")),
                Ok(Bytes::from_static(b"synthetic-acces")),
                Err(std::io::Error::other("synthetic-access provider error")),
            ],
        ));
    let dispatch = Lease::acquire(f.session.clone(), "synthetic-account", "body-error")
        .await
        .unwrap()
        .dispatch_with(
            request_body(),
            Duration::from_secs(3),
            &Fake::default(),
            &transport,
        )
        .await
        .unwrap();
    let mut stream = dispatch.response.bytes_stream();
    let mut emitted = Vec::new();
    let mut errors = 0;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => emitted.extend_from_slice(&bytes),
            Err(error) => {
                errors += 1;
                assert!(!error.to_string().contains("synthetic-"));
            }
        }
    }
    assert_eq!(emitted, b"safe ");
    assert_eq!(errors, 1);
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn inference_rejects_compressed_success_before_body_output() {
    let f = Fixture::new().await;
    f.seed(|_| {});
    for encoding in ["gzip", "br", "deflate", "zstd", "identity, gzip", "unknown"] {
        let transport = StreamFixture::default();
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_ENCODING,
            http::HeaderValue::from_str(encoding).unwrap(),
        );
        transport
            .responses
            .lock()
            .unwrap()
            .push_back(ProxyResponse::buffered(
                http::StatusCode::OK,
                headers,
                Bytes::from_static(b"synthetic-access"),
            ));
        let result = Lease::acquire(f.session.clone(), "synthetic-account", "body-compressed")
            .await
            .unwrap()
            .dispatch_with(
                request_body(),
                Duration::from_secs(3),
                &Fake::default(),
                &transport,
            )
            .await;
        assert!(
            result.is_err(),
            "unsupported compression must not bypass exact-secret detection"
        );
        let calls = transport.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].headers[http::header::ACCEPT_ENCODING], "identity");
    }
}
