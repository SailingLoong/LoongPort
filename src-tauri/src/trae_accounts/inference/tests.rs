use super::super::store::{self, Realm};
use super::*;
use crate::secrets::VaultContext;
use std::io::{Read, Write};

struct Owner {
    _serial: tokio::sync::OwnedMutexGuard<()>,
    _root: tempfile::TempDir,
    session: Arc<SecretSession>,
    seal: FixtureOwner,
}
impl Owner {
    async fn new() -> Self {
        let serial = FixtureTransport::serial().await;
        let root = tempfile::tempdir().unwrap();
        let session =
            SecretSession::from_context(root.path().into(), VaultContext::generate().unwrap());
        let seal = FixtureTransport::seed_accounts(
            &session,
            &[("synthetic-sg", Realm::Sg), ("synthetic-us", Realm::Us)],
        )
        .await
        .unwrap();
        Self {
            _serial: serial,
            _root: root,
            session,
            seal,
        }
    }
}
fn body() -> Value {
    json!({"model":"synthetic-model","messages":[{"role":"user","content":"hello"}],"stream":false})
}
fn server(status: &str, headers: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    );
    let join = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let mut stream = loop {
            if let Ok((s, _)) = listener.accept() {
                break s;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "synthetic server was never called"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        // Windows accept inherits the listener's nonblocking mode. Restore
        // blocking reads on the connected socket, still bounded by the timeout.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = stream.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                let h = String::from_utf8_lossy(&bytes[..end]);
                let size = h
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + size {
                    break;
                }
            }
            assert!(bytes.len() < 1024 * 1024);
        }
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(bytes).unwrap()
    });
    (origin, join)
}
#[tokio::test]
async fn adapter_cn_and_unverified_client_are_rejected_before_http() {
    let f = Owner::new().await;
    for (realm, client) in [
        (Realm::Cn, "ono9krqynydwx5"),
        (Realm::Sg, "unverified-client"),
    ] {
        store::access(&f.session, None, |d, _| {
            d.accounts[0].realm = realm;
            d.accounts[0].device.auth_host = realm.auth().into();
            d.accounts[0].device.client = client.into();
            Ok(((), true))
        })
        .await
        .unwrap();
        assert!(matches!(
            Lease::acquire(f.session.clone(), "synthetic-sg", "s").await,
            Err(Failure::Unsupported)
        ));
    }
}
#[tokio::test]
async fn adapter_invalidated_response_fence_is_fixed_internal_terminal() {
    let f = Owner::new().await;
    let fence = Lease::acquire(f.session.clone(), "synthetic-sg", "s")
        .await
        .unwrap()
        .fence();
    store::retire(&f.session, "synthetic-sg").await.unwrap();
    assert!(
        matches!(fence.check().await,Err(ProxyError::Internal(ref s)) if s=="trae.account_no_longer_available")
    );
}
#[tokio::test]
async fn adapter_denial_retains_original_pool_terminal_category() {
    let response = failure(403);
    let value: Value =
        serde_json::from_slice(&response.bytes_with_limit(4096).await.unwrap()).unwrap();
    assert_eq!(value["error"]["type"], "access_denied");
    let other: Value =
        serde_json::from_slice(&failure(502).bytes_with_limit(4096).await.unwrap()).unwrap();
    assert_eq!(other["error"]["type"], "api_error");
}
#[test]
fn adapter_capability_check_covers_original_three_protocols() {
    for v in [
        body(),
        json!({"messages":[{"role":"user","content":[{"type":"text","text":"hi"}]}],"system":[{"type":"text","text":"plain"}]}),
        json!({"input":"hi"}),
        json!({"input":[{"type":"message","role":"developer","content":[{"type":"input_text","text":"hi"}]}],"tools":[],"stream_options":{"include_usage":false}}),
    ] {
        assert!(validate_capabilities(&v).is_ok(), "{v}");
    }
    for v in [
        json!({"tools":[{"type":"function"}]}),
        json!({"functions":[{}]}),
        json!({"stream_options":{"include_usage":true}}),
        json!({"messages":[{"role":"tool","content":"secret"}]}),
        json!({"messages":[{"role":"assistant","content":"hi","tool_calls":[{}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"image","source":{}}]}]}),
        json!({"input":[{"type":"function_call_output","output":"x"}]}),
        json!({"input":[{"role":"user","content":[{"type":"input_audio","data":"x"}]}]}),
        json!({"system":[{"type":"tool_use"}]}),
        json!({"modalities":["audio"]}),
    ] {
        assert!(validate_capabilities(&v).is_err(), "{v}");
    }
}
#[tokio::test]
async fn adapter_fixture_only_seeds_empty_owner_and_binds_exact_account() {
    let f = Owner::new().await;
    assert_eq!(f.seal.account_ids(), ["synthetic-sg", "synthetic-us"]);
    assert!(
        FixtureTransport::seed_accounts(&f.session, &[("synthetic-next", Realm::Sg)])
            .await
            .is_err()
    );
    let fixture = FixtureTransport::loopback(&f.seal, "http://127.0.0.1:9").unwrap();
    store::access(&f.session, None, |d, _| {
        d.accounts[0].grant.as_mut().unwrap().access = "different".into();
        Ok(((), true))
    })
    .await
    .unwrap();
    let lease = Lease::acquire(f.session.clone(), "synthetic-sg", "s")
        .await
        .unwrap();
    assert!(lease
        .dispatch_fixture(body(), Duration::from_secs(1), &fixture)
        .await
        .is_err());
    assert_eq!(fixture.attempts(), 0);
}
#[tokio::test]
async fn adapter_fixture_cross_root_epoch_and_missing_transport_send_zero_http() {
    let f = Owner::new().await;
    let fixture = FixtureTransport::loopback(&f.seal, "http://127.0.0.1:9").unwrap();
    let other_root = tempfile::tempdir().unwrap();
    let other =
        SecretSession::from_context(other_root.path().into(), f.session.read().unwrap().clone());
    FixtureTransport::seed_accounts(&other, &[("synthetic-sg", Realm::Sg)])
        .await
        .unwrap();
    assert!(Lease::acquire(other, "synthetic-sg", "s")
        .await
        .unwrap()
        .dispatch_fixture(body(), Duration::from_secs(1), &fixture)
        .await
        .is_err());
    assert!(Lease::acquire(f.session.clone(), "synthetic-sg", "s")
        .await
        .unwrap()
        .dispatch(body(), Duration::from_secs(1))
        .await
        .is_err());
    f.session.set_blocked(true);
    f.session.set_blocked(false);
    assert!(Lease::acquire(f.session.clone(), "synthetic-sg", "s")
        .await
        .unwrap()
        .dispatch_fixture(body(), Duration::from_secs(1), &fixture)
        .await
        .is_err());
    assert_eq!(fixture.attempts(), 0);
}
#[tokio::test]
async fn adapter_fixture_loopback_only_and_no_redirect() {
    let f = Owner::new().await;
    for origin in [
        "https://127.0.0.1:1",
        "http://localhost:1",
        "http://example.com",
        "http://127.0.0.1/path",
        "http://user@127.0.0.1",
        "http://127.0.0.1/?x=1",
    ] {
        assert!(FixtureTransport::loopback(&f.seal, origin).is_err());
    }
    let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let (origin, join) = server(
        "302 Found",
        &format!("Location: http://{}/leak\r\n", target.local_addr().unwrap()),
        "",
    );
    let fixture = FixtureTransport::loopback(&f.seal, &origin).unwrap();
    let result = Lease::acquire(f.session.clone(), "synthetic-sg", "s")
        .await
        .unwrap()
        .dispatch_fixture(body(), Duration::from_secs(2), &fixture)
        .await
        .unwrap();
    assert_eq!(result.response.status().as_u16(), 302);
    assert_eq!(fixture.attempts(), 1);
    assert!(target.accept().is_err());
    join.join().unwrap();
}
#[tokio::test]
async fn adapter_fixture_global_wire_reaches_real_loopback_consumer() {
    let f = Owner::new().await;
    for (id, realm) in [("synthetic-sg", "sg"), ("synthetic-us", "us")] {
        let (origin, join) = server(
            "200 OK",
            "Content-Type: text/event-stream\r\n",
            "event: output\ndata: {\"response\":\"hello\"}\n\nevent: done\ndata: {}\n\n",
        );
        let fixture = FixtureTransport::loopback(&f.seal, &origin).unwrap();
        let result = Lease::acquire(f.session.clone(), id, "s")
            .await
            .unwrap()
            .dispatch_fixture(body(), Duration::from_secs(2), &fixture)
            .await
            .unwrap();
        result.fence.check().await.unwrap();
        let bytes = result.response.bytes_with_limit(4096).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("hello"));
        let wire = join.join().unwrap();
        assert!(wire.starts_with(&format!("POST /{realm}/api/agent/v3/llm_utils_chat ")));
        assert!(wire.contains(&format!("Cloud-IDE-JWT {id}-access")));
        assert_eq!(fixture.attempts(), 1);
    }
}

#[tokio::test]
async fn adapter_unknown_success_outcomes_are_terminal_not_synthetic_http_failures() {
    for (headers, reply) in [
        ("Content-Type: text/event-stream\r\n", ""),
        (
            "Content-Type: text/event-stream\r\n",
            "event: output\ndata: invalid-json\n\n",
        ),
        (
            "Content-Type: text/event-stream\r\nContent-Encoding: gzip\r\n",
            "opaque",
        ),
        ("Content-Type: application/json\r\n", "{}"),
        (
            "Content-Type: text/event-stream\r\n",
            "event: output\ndata: {\"response\":\"partial\"}\n\n",
        ),
        (
            "Content-Type: text/event-stream\r\n",
            "event: output\ndata: {\"response\":\"partial\"}\n\nevent: error\ndata: {\"code\":429}\n\n",
        ),
    ] {
        let f = Owner::new().await;
        let (origin, worker) = server("200 OK", headers, reply);
        let fixture = FixtureTransport::loopback(&f.seal, &origin).unwrap();
        let result = Lease::acquire(f.session.clone(), "synthetic-sg", "s")
            .await
            .unwrap()
            .dispatch_fixture(body(), Duration::from_secs(2), &fixture)
            .await;
        assert!(
            matches!(result, Err(ProxyError::ForwardFailed(_))),
            "unknown accepted response must remain a terminal transport error"
        );
        assert_eq!(fixture.attempts(), 1);
        worker.join().unwrap();
    }
}

#[tokio::test]
async fn adapter_real_http_502_keeps_explicit_response_policy() {
    let f = Owner::new().await;
    let (origin, worker) = server(
        "502 Bad Gateway",
        "Content-Type: application/json\r\n",
        "{}",
    );
    let fixture = FixtureTransport::loopback(&f.seal, &origin).unwrap();
    let result = Lease::acquire(f.session.clone(), "synthetic-sg", "s")
        .await
        .unwrap()
        .dispatch_fixture(body(), Duration::from_secs(2), &fixture)
        .await
        .unwrap();
    assert_eq!(result.response.status(), http::StatusCode::BAD_GATEWAY);
    assert_eq!(fixture.attempts(), 1);
    worker.join().unwrap();
}
