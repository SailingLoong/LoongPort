use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Seen {
    head: String,
    body: Value,
}
struct Reply {
    status: u16,
    headers: String,
    body: String,
}
impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: String::new(),
            body: body.to_string(),
        }
    }
}
struct LocalServer {
    url: Url,
    seen: Arc<Mutex<Vec<Seen>>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl LocalServer {
    fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let thread = thread::spawn(move || {
            let mut replies = replies.into_iter();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                // Windows may inherit the listener's nonblocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let request = read_request(&mut stream);
                captured.lock().unwrap().push(request);
                let Some(reply) = replies.next() else {
                    return;
                };
                let response=format!("HTTP/1.1 {} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",reply.status,reply.body.len(),reply.headers,reply.body);
                // A socket timeout applies to each write, not the whole response.
                // Partial progress and interruptions must share one fixed budget.
                let write_deadline = Instant::now() + Duration::from_secs(2);
                let mut remaining_response = response.as_bytes();
                while !remaining_response.is_empty() {
                    let Some(remaining_time) =
                        write_deadline.checked_duration_since(Instant::now())
                    else {
                        break;
                    };
                    if remaining_time.is_zero() {
                        break;
                    }
                    stream.set_write_timeout(Some(remaining_time)).unwrap();
                    match stream.write(remaining_response) {
                        Ok(0) => break,
                        Ok(written) => remaining_response = &remaining_response[written..],
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                // Last expected response still leaves this listener alive briefly
                // so forbidden automatic retries/redirects are observable.
                if replies.len() == 0 {
                    let until = Instant::now() + Duration::from_millis(100);
                    while Instant::now() < until {
                        if let Ok((mut extra, _)) = listener.accept() {
                            extra.set_nonblocking(false).unwrap();
                            extra
                                .set_read_timeout(Some(Duration::from_secs(1)))
                                .unwrap();
                            captured.lock().unwrap().push(read_request(&mut extra));
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    return;
                }
            }
        });
        Self {
            url,
            seen,
            thread: Some(thread),
        }
    }
    fn transport(&self) -> FixedRegionTransport {
        FixedRegionTransport::synthetic_server(self.url.clone()).unwrap()
    }
    async fn finish(mut self) -> Vec<Seen> {
        // Let the current-thread runtime finish dropping rejected HTTP responses.
        let thread = self.thread.take().unwrap();
        tokio::task::spawn_blocking(move || thread.join().unwrap())
            .await
            .unwrap();
        Arc::try_unwrap(self.seen)
            .ok()
            .unwrap()
            .into_inner()
            .unwrap()
    }
}
fn read_request(stream: &mut TcpStream) -> Seen {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
        assert!(bytes.len() < 2 * MAX_BYTES);
        if let Some(end) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() - end < length {
        let read = stream.read(&mut chunk).unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[end..end + length]).unwrap()
    };
    Seen {
        head: head.to_ascii_lowercase(),
        body,
    }
}
fn machine() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn identity() -> Identity {
    Identity {
        uid: "synthetic-uid".into(),
        machine_id: machine(),
    }
}
fn selected_provider(db: &Database) -> Provider {
    db.get_provider_by_id("wire-fixture", "codex")
        .unwrap()
        .unwrap()
}
fn source(db: &Database) -> SourceBinding {
    SourceBinding::from_provider(&selected_provider(db))
        .unwrap()
        .0
}
fn owner() -> (Database, super::super::PatDeclaration) {
    let db = Database::memory().unwrap();
    let mut provider = Provider::with_id(
        "wire-fixture".into(),
        "Synthetic wire".into(),
        serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-pat"}}),
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some("qoder_sdk".into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let generation = db
        .get_provider_by_id(&provider.id, "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
        .generation;
    (
        db,
        super::super::PatDeclaration {
            provider_id: provider.id,
            credential_generation: generation,
            source: super::super::CredentialSource::InternationalQoderPat,
        },
    )
}
#[tokio::test]
async fn fixed_exchange_and_userinfo_match_proven_paths_methods_bodies_and_headers() {
    let server = LocalServer::start(vec![
        Reply::json(
            200,
            serde_json::json!({"token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":600000}),
        ),
        Reply::json(
            200,
            serde_json::json!({"id":1234,"name":"ignored-name","email":"ignored@example.invalid"}),
        ),
    ]);
    let transport = server.transport();
    let result = transport
        .exchange_and_identify("synthetic-pat", &machine())
        .await
        .unwrap();
    assert_eq!(result.uid, "1234");
    assert_eq!(result.tokens.access_token, "synthetic-access");
    let requests = server.finish().await;
    assert_eq!(requests.len(), 2);
    assert!(requests[0]
        .head
        .starts_with("post /api/v1/jobtoken/exchange http/1.1\r\n"));
    assert_eq!(
        requests[0].body,
        serde_json::json!({"personal_token":"synthetic-pat"})
    );
    assert!(!requests[0].head.contains("authorization:"));
    assert!(requests[0].head.contains("accept: application/json"));
    assert!(requests[0].head.contains("content-type: application/json"));
    assert!(requests[1]
        .head
        .starts_with("get /api/v1/userinfo http/1.1\r\n"));
    assert!(requests[1]
        .head
        .contains("authorization: bearer synthetic-access"));
    assert_eq!(requests[1].body, Value::Null);
    for request in &requests {
        assert!(!request.head.contains("cosy-"));
        assert!(!request.head.contains("origin:"));
    }
}
#[tokio::test]
async fn fixed_refresh_spends_exactly_one_body_without_pat_bearer_or_userinfo() {
    let server = LocalServer::start(vec![Reply::json(
        200,
        serde_json::json!({"token":"synthetic-new-access","refresh_token":"synthetic-new-refresh","expires_in":600000}),
    )]);
    let transport = server.transport();
    let result = transport
        .refresh("synthetic-old-refresh", &identity())
        .await
        .unwrap();
    assert_eq!(result.access_token, "synthetic-new-access");
    let requests = server.finish().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .head
        .starts_with("post /api/v1/jobtoken/refresh http/1.1\r\n"));
    assert!(!requests[0].head.contains("authorization:"));
    assert_eq!(
        requests[0].body,
        serde_json::json!({"refresh_token":"synthetic-old-refresh"})
    );
}
#[tokio::test]
async fn refused_refresh_remains_unknown_and_never_falls_back_to_pat_exchange() {
    let server = LocalServer::start(vec![Reply::json(
        401,
        serde_json::json!({"error":"synthetic refusal"}),
    )]);
    let transport = server.transport();
    let (db, declaration) = owner();
    let now = chrono::Utc::now().timestamp_millis();
    let request = IdentityActionRequest {
        declaration,
        action: IdentityAction::Establish,
    };
    let PreparedIdentityAction::Establish { receipt, .. } = db
        .qoder_http_prepare_identity_action(&request, now)
        .unwrap()
    else {
        panic!("initial setup must reserve")
    };
    let lease = db.qoder_http_reserve_identity_action(receipt).unwrap();
    db.qoder_http_publish_establish(
        &lease,
        Established {
            uid: "synthetic-uid".into(),
            tokens: TokenPair {
                access_token: "synthetic-access".into(),
                refresh_token: "synthetic-refresh".into(),
                expires_at_ms: now + 1,
            },
        },
        now,
    )
    .unwrap();
    assert!(acquire(&db, &selected_provider(&db), &transport, || now)
        .await
        .is_err());
    assert!(acquire(&db, &selected_provider(&db), &transport, || now)
        .await
        .is_err());
    let requests = server.finish().await;
    assert_eq!(requests.len(), 1);
    assert!(requests[0].head.contains("/jobtoken/refresh"));
}
#[tokio::test]
async fn redirect_is_not_followed_or_retried() {
    let server = LocalServer::start(vec![Reply {
        status: 302,
        headers: "Location: http://127.0.0.1:9/must-not-follow\r\n".into(),
        body: "{}".into(),
    }]);
    assert!(server
        .transport()
        .exchange_and_identify("synthetic-pat", &machine())
        .await
        .is_err());
    assert_eq!(server.finish().await.len(), 1);
}
#[tokio::test]
async fn partial_refresh_body_stays_unknown_without_replay() {
    let server = LocalServer::start(vec![Reply::json(
        200,
        serde_json::json!({"token":"synthetic-partial-access"}),
    )]);
    assert!(server
        .transport()
        .refresh("synthetic-refresh", &identity())
        .await
        .is_err());
    assert_eq!(server.finish().await.len(), 1);
}
#[tokio::test]
async fn missing_uid_does_not_publish_an_unbound_token() {
    let server = LocalServer::start(vec![
        Reply::json(
            200,
            serde_json::json!({"token":"synthetic-access","expires_in":600000}),
        ),
        Reply::json(200, serde_json::json!({"email":"ignored@example.invalid"})),
    ]);
    let (db, declaration) = owner();
    assert!(matches!(
        perform_identity_action(
            &db,
            IdentityActionRequest {
                declaration,
                action: IdentityAction::Establish
            },
            &server.transport(),
            |_| async { panic!("initial setup cannot ask twice") },
            || chrono::Utc::now().timestamp_millis()
        )
        .await,
        IdentityActionResult::Failed {
            code: IdentityFailureCode::OutcomeUnknown
        }
    ));
    assert!(db
        .qoder_http_acquire(&source(&db), chrono::Utc::now().timestamp_millis())
        .is_err());
    assert_eq!(server.finish().await.len(), 2);
}
#[tokio::test]
async fn oversized_response_is_rejected_without_identity_request() {
    let server = LocalServer::start(vec![Reply {
        status: 200,
        headers: String::new(),
        body: "x".repeat(MAX_BYTES + 1),
    }]);
    assert!(server
        .transport()
        .exchange_and_identify("synthetic-pat", &machine())
        .await
        .is_err());
    assert_eq!(server.finish().await.len(), 1);
}
#[tokio::test]
async fn unread_response_server_has_bounded_write_and_is_joined() {
    let server = LocalServer::start(vec![Reply {
        status: 200,
        headers: String::new(),
        body: "x".repeat(16 * MAX_BYTES),
    }]);
    // Keep the peer alive without reading, exercising server-side backpressure.
    let mut peer = TcpStream::connect(server.url.socket_addrs(|| None).unwrap()[0]).unwrap();
    peer.write_all(b"POST /synthetic HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(5), server.finish())
        .await
        .expect("the write deadline must let the actual server thread join");
    assert_eq!(requests.len(), 1);
    drop(peer);
}
#[test]
fn source_expiry_units_aliases_and_optional_refresh_are_explicit() {
    let now = 1000;
    let token = parse_exchange(
        &serde_json::json!({"access_token":"synthetic","expires_in":600000}),
        now,
    )
    .unwrap();
    assert_eq!(token.expires_at_ms, 601000);
    assert!(token.refresh_token.is_empty());
    assert_eq!(parse_exchange(&serde_json::json!({"token":"synthetic","expire_time":"2026-10-19T00:00:00Z","expires_in":1}),now).unwrap().expires_at_ms,1792368000000);
    assert_eq!(
        parse_refresh(
            &serde_json::json!({"token":"synthetic","refresh_token":"synthetic"}),
            now
        )
        .unwrap()
        .expires_at_ms,
        now + DAY_MS
    );
    assert!(parse_refresh(
        &serde_json::json!({"access_token":"synthetic","refresh_token":"synthetic"}),
        now
    )
    .is_err());
    assert!(parse_exchange(
        &serde_json::json!({"token":"","access_token":"must-not-fallback"}),
        now
    )
    .is_err());
}
#[test]
fn test_origin_factory_cannot_target_an_external_host_or_url_credentials() {
    for url in [
        "https://127.0.0.1/",
        "http://localhost/",
        "http://example.invalid/",
        "http://user@127.0.0.1/",
        "http://127.0.0.1/path",
        "http://127.0.0.1/?query=1",
    ] {
        assert!(FixedRegionTransport::synthetic_server(Url::parse(url).unwrap()).is_err());
    }
}

#[test]
fn cn_exchange_rejects_native_aliases_without_changing_international_compatibility() {
    let now = 1_000_000;
    for field in ["device_token", "access_token"] {
        let mut body = serde_json::json!({ "expires_in": 600000 });
        body[field] = serde_json::json!("synthetic-alias");
        assert!(
            parse_exchange_for_source(&SourceKind::ChinaQoderPat, &body, now).is_err(),
            "CN PAT exchange must not infer a native credential family from {field}"
        );
        assert!(parse_exchange_for_source(&SourceKind::InternationalQoderPat, &body, now).is_ok());
    }
    let body = serde_json::json!({"token":"synthetic-job", "refresh_token":"synthetic-job-refresh", "expires_in":600000});
    let cn = parse_exchange_for_source(&SourceKind::ChinaQoderPat, &body, now).unwrap();
    assert_eq!(cn.access_token, "synthetic-job");
    assert_eq!(cn.refresh_token, "synthetic-job-refresh");
    let alias_refresh = serde_json::json!({"token":"synthetic-job", "refreshToken":"native-alias", "expires_in":600000});
    let cn = parse_exchange_for_source(&SourceKind::ChinaQoderPat, &alias_refresh, now).unwrap();
    assert!(cn.refresh_token.is_empty());
}
