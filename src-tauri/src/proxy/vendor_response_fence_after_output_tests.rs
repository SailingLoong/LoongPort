//! Child of vendor_response_fence_tests; all owners and transport are reused.
//! Authorization changes occur after actual final-body answer bytes were read.
use super::*;

async fn consume_answer_prefix<S, E>(body: &mut S)
where
    S: futures::Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut delivered = Vec::new();
        loop {
            let chunk = body
                .next()
                .await
                .expect("the live stream must deliver an answer before retirement")
                .expect("unchanged authorization must admit the initial body bytes");
            delivered.extend_from_slice(&chunk);
            assert!(delivered.len() <= 65_536, "bounded synthetic prefix");
            if String::from_utf8_lossy(&delivered).contains("synthetic primed answer") {
                break;
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn emitted_answer_then_key_replacement_rejects_every_later_body_chunk() {
    let f = Fixture::new(Behavior::Complete).await;
    let (response, calls, release, _upstream) = primed_response(&f).await;
    let mut body = Box::pin(response.into_body().into_data_stream());
    consume_answer_prefix(&mut body).await;
    with_owner_conn(&f, |conn, vault, id| {
        crate::vendor::creds::set_api_key(conn, vault, id, "SYNTHETIC_AFTER_OUTPUT_KEY").unwrap();
    });
    release.add_permits(1);
    let next = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap();
    assert!(
        matches!(next, Some(Err(_))),
        "no final-body bytes after key retirement"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an already-emitted response must never replay"
    );
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "no fallback account HTTP"
    );
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn emitted_answer_then_vault_lock_rejects_every_later_body_chunk() {
    let f = Fixture::new(Behavior::Complete).await;
    let (response, calls, release, _upstream) = primed_response(&f).await;
    let mut body = Box::pin(response.into_body().into_data_stream());
    consume_answer_prefix(&mut body).await;
    let session = f.owner.state().db.secret_session();
    session.set_blocked(true);
    release.add_permits(1);
    let next = tokio::time::timeout(Duration::from_secs(5), body.next()).await;
    // Cleanup reopens only this synthetic owner. Its old epoch stays retired.
    session.set_blocked(false);
    assert!(
        matches!(next.unwrap(), Some(Err(_))),
        "no final-body bytes while locked"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an already-emitted response must never replay"
    );
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "no fallback account HTTP"
    );
    f.stop().await;
}

#[tokio::test]
#[serial_test::serial]
async fn emitted_answer_then_account_recreation_rejects_every_later_body_chunk() {
    let f = Fixture::new(Behavior::Complete).await;
    let (response, calls, release, _upstream) = primed_response(&f).await;
    let mut body = Box::pin(response.into_body().into_data_stream());
    consume_answer_prefix(&mut body).await;
    with_owner_conn(&f, |conn, vault, old_id| {
        let old = crate::vendor::creds::get(conn, vault, old_id)
            .unwrap()
            .unwrap();
        crate::vendor::creds::remove(conn, old_id).unwrap();
        let new_id = crate::vendor::creds::save_account(
            conn,
            vault,
            crate::vendor::Vendor::OpenCode,
            "SYNTHETIC_AFTER_OUTPUT_LOGIN",
            &crate::vendor::VendorAccount {
                account_id: old.account_id.unwrap(),
                label: "Synthetic account recreated after output".into(),
                login_identifier: String::new(),
            },
        )
        .unwrap();
        assert_ne!(new_id, old_id);
        crate::vendor::creds::set_api_key(conn, vault, new_id, f.owner.synthetic_key(0)).unwrap();
    });
    release.add_permits(1);
    let next = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap();
    assert!(
        matches!(next, Some(Err(_))),
        "old account cannot keep emitting after recreation"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an already-emitted response must never replay"
    );
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "no fallback account HTTP"
    );
    // The deliberately removed temporary owner cannot promise native restore.
    f.owner.state().proxy_service.stop().await.unwrap();
}
