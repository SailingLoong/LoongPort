//! Register as a child of opencode_vendor_pool_tests to reuse its sealed owner.
//! These assertions exercise the original ProxyServer's final downstream Body.
use super::*;
use bytes::Bytes;
use futures::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};

fn with_owner_conn<T>(
    f: &Fixture,
    action: impl FnOnce(&rusqlite::Connection, &crate::secrets::VaultContext, i64) -> T,
) -> T {
    let db = &f.owner.state().db;
    let row = db
        .get_provider_by_id(&f.owner.ids()[0], "codex")
        .unwrap()
        .unwrap();
    let id = row
        .meta
        .unwrap()
        .auth_binding
        .unwrap()
        .account_id
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let vault = db.secret_session().read().unwrap();
    let conn = db.conn.lock().unwrap();
    action(&conn, &vault, id)
}

async fn assert_no_body_bytes(response: axum::response::Response) {
    let mut body = response.into_body().into_data_stream();
    let item = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap();
    assert!(
        matches!(item, Some(Err(_))),
        "retired authorization must fail before the first final-body byte"
    );
}

async fn assert_buffered_answer(response: axum::response::Response) {
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 65_536),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("synthetic vendor answer"));
}

#[tokio::test]
#[serial_test::serial]
async fn buffered_response_rejects_removed_vendor_account() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    // Exercise the original credential owner's deletion, without UI/native effects.
    with_owner_conn(&f, |conn, _, id| {
        crate::vendor::creds::remove(conn, id).unwrap()
    });
    assert_no_body_bytes(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.owner.state().proxy_service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn buffered_response_rejects_vendor_deleted_then_recreated_by_owner() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    with_owner_conn(&f, |conn, vault, old_id| {
        let old = crate::vendor::creds::get(conn, vault, old_id)
            .unwrap()
            .unwrap();
        crate::vendor::creds::remove(conn, old_id).unwrap();
        let new_id = crate::vendor::creds::save_account(
            conn,
            vault,
            crate::vendor::Vendor::OpenCode,
            "SYNTHETIC_RELOGIN_SIGNAL",
            &crate::vendor::VendorAccount {
                account_id: old.account_id.unwrap(),
                label: "Recreated synthetic account".into(),
                login_identifier: String::new(),
            },
        )
        .unwrap();
        assert_ne!(
            new_id, old_id,
            "the original owner allocates a new account row"
        );
        crate::vendor::creds::set_api_key(conn, vault, new_id, f.owner.synthetic_key(0)).unwrap();
    });
    assert_no_body_bytes(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.owner.state().proxy_service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn buffered_response_rejects_deleted_provider_reference() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    f.owner
        .state()
        .db
        .delete_provider("codex", &f.owner.ids()[0])
        .unwrap();
    assert_no_body_bytes(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.owner.state().proxy_service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn buffered_response_rejects_changed_account_binding() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let db = &f.owner.state().db;
    let mut first = db
        .get_provider_by_id(&f.owner.ids()[0], "codex")
        .unwrap()
        .unwrap();
    let second = db
        .get_provider_by_id(&f.owner.ids()[1], "codex")
        .unwrap()
        .unwrap();
    let other = second.meta.unwrap();
    let meta = first.meta.as_mut().unwrap();
    meta.auth_binding = other.auth_binding;
    meta.loongport_vendor_account = other.loongport_vendor_account;
    db.save_provider("codex", &first).unwrap();
    assert_no_body_bytes(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.owner.state().proxy_service.stop().await.unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn buffered_response_does_not_revive_after_vault_unlock() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let session = f.owner.state().db.secret_session();
    session.set_blocked(true);
    session.set_blocked(false);
    assert_no_body_bytes(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn buffered_response_rejects_reissued_identical_key() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    with_owner_conn(&f, |conn, vault, id| {
        crate::vendor::creds::set_api_key(conn, vault, id, f.owner.synthetic_key(0)).unwrap();
    });
    assert_no_body_bytes(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn token_expiry_display_edit_and_reorder_preserve_api_key_response() {
    let f = Fixture::new(Behavior::Complete).await;
    let response = f.request(Some(f.transport.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    with_owner_conn(&f, |conn, _, id| {
        crate::vendor::creds::clear_token(conn, id).unwrap();
        crate::vendor::creds::reorder(conn, &[id]).unwrap();
    });
    let db = &f.owner.state().db;
    let mut row = db
        .get_provider_by_id(&f.owner.ids()[0], "codex")
        .unwrap()
        .unwrap();
    row.name = "Changed synthetic display name".into();
    row.sort_index = Some(123);
    db.save_provider("codex", &row).unwrap();
    assert_buffered_answer(response).await;
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    f.stop().await;
}

struct StreamUpstream(tokio::task::JoinHandle<()>);
impl Drop for StreamUpstream {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn primed_response(
    f: &Fixture,
) -> (
    axum::response::Response,
    Arc<AtomicUsize>,
    Arc<tokio::sync::Semaphore>,
    StreamUpstream,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = calls.clone();
    let ready = Arc::new(tokio::sync::Semaphore::new(0));
    let released = ready.clone();
    let app = axum::Router::new().route("/zen/v1/responses", axum::routing::post(move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
        let calls = captured.clone();
        let ready = released.clone();
        async move {
            assert_eq!(headers["authorization"], "Bearer SYNTHETIC_OPENCODE_KEY_A");
            calls.fetch_add(1, Ordering::SeqCst);
            let prefix = format!(
                "event: response.created\ndata: {}\n\nevent: response.output_text.delta\ndata: {}\n\n",
                json!({"type":"response.created","response":{"id":"resp_fence","object":"response","status":"in_progress","model":body["model"],"output":[]}}),
                json!({"type":"response.output_text.delta","response_id":"resp_fence","item_id":"msg_fence","output_index":0,"content_index":0,"delta":"synthetic primed answer"})
            );
            let stream = async_stream::stream! {
                yield Ok::<Bytes, std::io::Error>(Bytes::from(prefix));
                ready.acquire().await.unwrap().forget();
                yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
            };
            ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transport = f.owner.transport(listener.local_addr().unwrap()).unwrap();
    let task = StreamUpstream(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let router = f.proxy.build_router().layer(axum::Extension(transport));
    let body = json!({"model":f.owner.model(),"input":"synthetic stream prompt","stream":true});
    let response = tokio::time::timeout(
        Duration::from_secs(8),
        router.oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .header("authorization", "Bearer untrusted-client-key")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    (response, calls, ready, task)
}

#[tokio::test]
#[serial_test::serial]
async fn primed_response_rejects_key_replacement_before_final_body() {
    let f = Fixture::new(Behavior::Complete).await;
    let (response, calls, release, _upstream) = primed_response(&f).await;
    with_owner_conn(&f, |conn, vault, id| {
        crate::vendor::creds::set_api_key(conn, vault, id, "SYNTHETIC_REPLACEMENT_KEY").unwrap();
    });
    release.add_permits(1);
    assert_no_body_bytes(response).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "authorization retirement must not retry another account"
    );
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn primed_response_with_unchanged_authorization_consumes_normally() {
    let f = Fixture::new(Behavior::Complete).await;
    let (response, calls, release, _upstream) = primed_response(&f).await;
    release.add_permits(1);
    let body = tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 65_536),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("synthetic primed answer"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    f.stop().await;
}

#[cfg(all(test, feature = "gui"))]
#[path = "vendor_response_fence_after_output_tests.rs"]
mod vendor_response_fence_after_output_tests;
