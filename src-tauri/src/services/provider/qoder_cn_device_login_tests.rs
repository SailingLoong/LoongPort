//! Synthetic grants only. Every service response is an in-process fixture.
use super::*;
use crate::services::provider::qoder_http_auth::{self as owner, IdentityStatus};
const NOW: i64 = 1_000_000;
fn new_request() -> StartRequest {
    StartRequest {
        provider_id: None,
        credential_generation: None,
        rebind: false,
    }
}
fn start_new(sessions: &DeviceLoginSessions, db: &Arc<Database>) -> StartResult {
    sessions.start(
        db.clone(),
        new_request(),
        |p| db.save_provider("codex", &p),
        |_| true,
        NOW,
    )
}
#[test]
fn independent_grant_creates_original_provider_without_a_pat_or_official_file() {
    let db = Arc::new(Database::memory().unwrap());
    let sessions = DeviceLoginSessions::default();
    let StartResult::Started {
        provider_id,
        credential_generation,
        auth_url,
        expires_at_ms,
        poll_interval_ms,
        ..
    } = start_new(&sessions, &db)
    else {
        panic!("new device grant must start through original Provider owner");
    };
    let provider = db
        .get_provider_by_id(&provider_id, "codex")
        .unwrap()
        .unwrap();
    assert_eq!(
        provider.meta.as_ref().unwrap().provider_type.as_deref(),
        Some("qoder_cn_device")
    );
    let (source, pat) = owner::SourceBinding::from_provider(&provider).unwrap();
    assert!(pat.is_empty());
    assert_eq!(source.origin, "https://openapi.qoder.com.cn");
    assert_eq!(source.credential_generation, credential_generation);
    assert_eq!(expires_at_ms, NOW + 300_000);
    assert_eq!(poll_interval_ms, 1000);
    assert!(auth_url.starts_with("https://qoder.cn/device/selectAccounts?"));
    assert!(!auth_url.contains("verifier="));
    assert_eq!(
        db.qoder_http_identity_context(&provider_id, NOW)
            .unwrap()
            .status,
        IdentityStatus::Pending
    );
}
#[test]
fn malformed_new_declaration_cannot_add_a_provider_or_start_grant() {
    let db = Arc::new(Database::memory().unwrap());
    let result = DeviceLoginSessions::default().start(
        db,
        StartRequest {
            credential_generation: Some("forged".into()),
            ..new_request()
        },
        |_| panic!("must reject before save"),
        |_| panic!("must not confirm"),
        NOW,
    );
    assert!(matches!(
        result,
        StartResult::Failed {
            code: "invalid_request"
        }
    ));
}

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
struct MockPoll {
    calls: AtomicUsize,
    replies: Mutex<VecDeque<Result<Option<owner::Established>, ()>>>,
    gate: Option<(tokio::sync::Notify, tokio::sync::Notify)>,
}
impl MockPoll {
    fn new(replies: Vec<Result<Option<owner::Established>, ()>>, delayed: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            replies: Mutex::new(replies.into()),
            gate: delayed.then(|| (tokio::sync::Notify::new(), tokio::sync::Notify::new())),
        }
    }
}
impl DevicePoll for MockPoll {
    fn poll<'a>(
        &'a self,
        _nonce: &'a str,
        _verifier: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<owner::Established>, ()>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some((entered, release)) = &self.gate {
                entered.notify_one();
                release.notified().await;
            }
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected repeated grant poll")
        })
    }
}
fn success(uid: &str) -> Result<Option<owner::Established>, ()> {
    Ok(Some(owner::Established {
        uid: uid.into(),
        tokens: owner::TokenPair {
            access_token: "synthetic-native-access".into(),
            refresh_token: "synthetic-native-refresh".into(),
            expires_at_ms: NOW + 900_000,
        },
    }))
}
fn session_details(result: StartResult) -> (String, String, String) {
    let StartResult::Started {
        session_id,
        provider_id,
        credential_generation,
        ..
    } = result
    else {
        panic!("expected started grant");
    };
    (session_id, provider_id, credential_generation)
}
fn fixture() -> (
    Arc<DeviceLoginSessions>,
    Arc<Database>,
    String,
    String,
    String,
) {
    let sessions = Arc::new(DeviceLoginSessions::default());
    let db = Arc::new(Database::memory().unwrap());
    let (session, provider, generation) = session_details(start_new(&sessions, &db));
    (sessions, db, session, provider, generation)
}
#[tokio::test]
async fn pending_poll_is_throttled_then_verified_grant_reaches_original_request_snapshot() {
    let (sessions, db, id, provider_id, _) = fixture();
    let transport = MockPoll::new(vec![Ok(None), success("synthetic-uid-a")], false);
    assert!(matches!(
        sessions.poll_using(&db, &id, &transport, || NOW).await,
        PollResult::Pending
    ));
    assert!(matches!(
        sessions.poll_using(&db, &id, &transport, || NOW + 1).await,
        PollResult::Pending
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        sessions
            .poll_using(&db, &id, &transport, || NOW + 1000)
            .await,
        PollResult::Completed { .. }
    ));
    assert!(matches!(
        sessions
            .poll_using(&db, &id, &transport, || NOW + 1001)
            .await,
        PollResult::Completed { .. }
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 2);
    let provider = db
        .get_provider_by_id(&provider_id, "codex")
        .unwrap()
        .unwrap();
    let (source, _) = owner::SourceBinding::from_provider(&provider).unwrap();
    let owner::Acquisition::Ready(snapshot) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!("original request snapshot must be usable");
    };
    assert_eq!(snapshot.credential_kind(), "china_qoder_device");
    assert_eq!(snapshot.uid(), "synthetic-uid-a");
    assert_eq!(snapshot.access_token(), "synthetic-native-access");
    assert!(!serde_json::to_string(&provider)
        .unwrap()
        .contains("synthetic-native"));
}
#[tokio::test]
async fn cancellation_before_poll_makes_no_request() {
    let (sessions, db, id, provider_id, _) = fixture();
    sessions.cancel(&db, &id);
    let transport = MockPoll::new(vec![], false);
    assert!(matches!(
        sessions.poll_using(&db, &id, &transport, || NOW).await,
        PollResult::Failed { code: "cancelled" }
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        db.qoder_http_identity_context(&provider_id, NOW)
            .unwrap()
            .status,
        IdentityStatus::Unknown
    );
}
#[tokio::test]
async fn opaque_session_is_scoped_to_original_database_owner() {
    let (sessions, _db, id, _, _) = fixture();
    let other_db = Arc::new(Database::memory().unwrap());
    let transport = MockPoll::new(vec![], false);
    assert!(matches!(
        sessions
            .poll_using(&other_db, &id, &transport, || NOW)
            .await,
        PollResult::Failed {
            code: "invalid_session"
        }
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

use std::sync::atomic::AtomicI64;
fn recover(
    sessions: &DeviceLoginSessions,
    db: &Arc<Database>,
    provider_id: &str,
    generation: &str,
    rebind: bool,
) -> StartResult {
    sessions.start(
        db.clone(),
        StartRequest {
            provider_id: Some(provider_id.into()),
            credential_generation: Some(generation.into()),
            rebind,
        },
        |_| panic!("existing record must not be re-added"),
        |_| true,
        NOW,
    )
}
#[tokio::test]
async fn replaced_attempt_is_rejected_before_poll_transport() {
    let (sessions, db, id, provider, generation) = fixture();
    session_details(recover(&sessions, &db, &provider, &generation, false));
    let transport = MockPoll::new(vec![success("synthetic-uid-a")], false);
    assert!(matches!(
        sessions.poll_using(&db, &id, &transport, || NOW).await,
        PollResult::Failed {
            code: "source_changed"
        }
    ));
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        0,
        "retired PKCE attempt must not reach transport"
    );
    assert_eq!(
        db.qoder_http_identity_context(&provider, NOW)
            .unwrap()
            .status,
        IdentityStatus::Pending
    );
}
#[tokio::test]
async fn vault_lock_is_rejected_before_poll_transport() {
    let (sessions, db, id, _, _) = fixture();
    db.secrets.set_blocked(true);
    let transport = MockPoll::new(vec![success("synthetic-uid-a")], false);
    assert!(matches!(
        sessions.poll_using(&db, &id, &transport, || NOW).await,
        PollResult::Failed {
            code: "source_changed"
        }
    ));
    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        0,
        "locked grant must not reach transport"
    );
}
async fn late_case_with_reply(kind: &str, pending: bool) {
    let (sessions, db, id, provider, generation) = fixture();
    let clock = Arc::new(AtomicI64::new(NOW));
    let transport = Arc::new(MockPoll::new(
        vec![if pending {
            Ok(None)
        } else {
            success("synthetic-uid-a")
        }],
        true,
    ));
    let task = {
        let (sessions, db, id, clock, transport) = (
            sessions.clone(),
            db.clone(),
            id.clone(),
            clock.clone(),
            transport.clone(),
        );
        tokio::spawn(async move {
            sessions
                .poll_using(&db, &id, transport.as_ref(), || {
                    clock.load(Ordering::SeqCst)
                })
                .await
        })
    };
    transport.gate.as_ref().unwrap().0.notified().await;
    // A second poll cannot duplicate the already in-flight request.
    assert!(matches!(
        sessions
            .poll_using(&db, &id, transport.as_ref(), || NOW)
            .await,
        PollResult::Pending
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    match kind {
        "cancel" => {
            sessions.cancel(&db, &id);
        }
        "timeout" => {
            clock.store(NOW + LOGIN_TTL_MS, Ordering::SeqCst);
        }
        "lock" => {
            db.secrets.set_blocked(true);
            db.secrets.set_blocked(false);
        }
        "replace" => {
            session_details(recover(&sessions, &db, &provider, &generation, false));
        }
        "delete" => {
            db.delete_provider("codex", &provider).unwrap();
        }
        "realm" => {
            let mut p = db.get_provider_by_id(&provider, "codex").unwrap().unwrap();
            p.meta.as_mut().unwrap().provider_type = Some("qoder_sdk".into());
            p.settings_config =
                serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-independent-pat"}});
            db.save_provider("codex", &p).unwrap();
        }
        _ => panic!("unknown synthetic action"),
    }
    transport.gate.as_ref().unwrap().1.notify_one();
    let result = task.await.unwrap();
    assert!(
        matches!(result, PollResult::Failed { .. }),
        "late response was accepted: {kind}: {result:?}"
    );
    if kind == "delete" {
        assert!(db.get_provider_by_id(&provider, "codex").unwrap().is_none());
    } else {
        assert_ne!(
            db.qoder_http_identity_context(&provider, NOW)
                .unwrap()
                .status,
            IdentityStatus::Ready
        );
    }
}
async fn late_case(kind: &str) {
    late_case_with_reply(kind, false).await;
}
#[tokio::test]
async fn pending_reply_cannot_keep_retired_or_locked_attempt_alive() {
    for kind in ["lock", "replace", "delete", "realm"] {
        late_case_with_reply(kind, true).await;
    }
}
#[tokio::test]
async fn cancelled_inflight_result_cannot_publish() {
    late_case("cancel").await;
}
#[tokio::test]
async fn timed_out_inflight_result_cannot_publish() {
    late_case("timeout").await;
}
#[tokio::test]
async fn vault_lock_unlock_epoch_rejects_late_result() {
    late_case("lock").await;
}
#[tokio::test]
async fn newer_attempt_rejects_old_inflight_result() {
    late_case("replace").await;
}
#[tokio::test]
async fn deleted_account_cannot_be_resurrected_by_late_result() {
    late_case("delete").await;
}
#[tokio::test]
async fn realm_change_cannot_receive_old_cn_grant() {
    late_case("realm").await;
}
#[tokio::test]
async fn recover_preserves_identity_and_machine_but_explicit_rebind_can_replace_both() {
    let (sessions, db, first_id, provider, generation) = fixture();
    let first_machine = sessions
        .lookup(&db, &first_id)
        .unwrap()
        .lease
        .machine_id
        .clone();
    let transport = MockPoll::new(vec![success("synthetic-uid-a")], false);
    assert!(matches!(
        sessions
            .poll_using(&db, &first_id, &transport, || NOW)
            .await,
        PollResult::Completed { .. }
    ));
    let p = db.get_provider_by_id(&provider, "codex").unwrap().unwrap();
    let (source, _) = owner::SourceBinding::from_provider(&p).unwrap();
    let owner::Acquisition::Ready(snapshot) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!("missing ready snapshot");
    };
    db.qoder_http_reject_device_snapshot(&snapshot).unwrap();
    let (same_id, _, _) = session_details(recover(&sessions, &db, &provider, &generation, false));
    assert_eq!(
        sessions.lookup(&db, &same_id).unwrap().lease.machine_id,
        first_machine
    );
    let wrong_uid = MockPoll::new(
        vec![success("synthetic-uid-b"), success("synthetic-uid-b")],
        false,
    );
    assert!(matches!(
        sessions.poll_using(&db, &same_id, &wrong_uid, || NOW).await,
        PollResult::Failed {
            code: "identity_rebind_required"
        }
    ));
    let (new_id, _, _) = session_details(recover(&sessions, &db, &provider, &generation, true));
    assert_ne!(
        sessions.lookup(&db, &new_id).unwrap().lease.machine_id,
        first_machine
    );
    assert!(matches!(
        sessions.poll_using(&db, &new_id, &wrong_uid, || NOW).await,
        PollResult::Completed { .. }
    ));
}
#[test]
fn pkce_uses_production_public_client_and_never_exposes_verifier_in_ipc() {
    let sessions = DeviceLoginSessions::default();
    let db = Arc::new(Database::memory().unwrap());
    let result = start_new(&sessions, &db);
    let wire = serde_json::to_string(&result).unwrap();
    let StartResult::Started {
        session_id,
        auth_url,
        ..
    } = result
    else {
        panic!("expected grant");
    };
    let session = sessions.lookup(&db, &session_id).unwrap();
    let url = reqwest::Url::parse(&auth_url).unwrap();
    let query: HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(query.len(), 5);
    assert_eq!(query.get("client_id").unwrap(), PRODUCTION_CLIENT_ID);
    assert_eq!(query.get("machine_id").unwrap(), &session.lease.machine_id);
    assert_eq!(query.get("challenge_method").unwrap(), "S256");
    assert_eq!(
        query.get("challenge").unwrap(),
        &URL_SAFE_NO_PAD.encode(Sha256::digest(session.verifier.as_bytes()))
    );
    assert_eq!(session.verifier.len(), 43);
    assert!(!wire.contains(session.verifier.as_str()));
    assert!(!wire.contains("refresh_token"));
}

#[tokio::test]
async fn preflight_rejects_deleted_realm_changed_or_locked_epoch_without_transport() {
    for action in ["delete", "realm", "epoch"] {
        let (sessions, db, id, provider, _) = fixture();
        match action {
            "delete" => {
                db.delete_provider("codex", &provider).unwrap();
            }
            "realm" => {
                let mut p = db.get_provider_by_id(&provider, "codex").unwrap().unwrap();
                p.meta.as_mut().unwrap().provider_type = Some("qoder_sdk".into());
                p.settings_config =
                    serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-independent-pat"}});
                db.save_provider("codex", &p).unwrap();
            }
            _ => {
                db.secrets.set_blocked(true);
                db.secrets.set_blocked(false);
            }
        }
        let transport = MockPoll::new(vec![], false);
        assert!(matches!(
            sessions.poll_using(&db, &id, &transport, || NOW).await,
            PollResult::Failed {
                code: "source_changed"
            }
        ));
        assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn failed_poll_is_terminal_and_never_replays_ambiguous_grant() {
    let (sessions, db, id, provider, _) = fixture();
    let transport = MockPoll::new(vec![Err(())], false);
    for _ in 0..2 {
        assert!(matches!(
            sessions.poll_using(&db, &id, &transport, || NOW).await,
            PollResult::Failed {
                code: "login_failed"
            }
        ));
    }
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.qoder_http_identity_context(&provider, NOW)
            .unwrap()
            .status,
        IdentityStatus::Unknown
    );
}
#[tokio::test]
async fn expired_grant_makes_no_request_and_recovery_keeps_own_machine() {
    let (sessions, db, id, provider, generation) = fixture();
    let machine = sessions.lookup(&db, &id).unwrap().lease.machine_id.clone();
    let transport = MockPoll::new(vec![], false);
    assert!(matches!(
        sessions
            .poll_using(&db, &id, &transport, || NOW + LOGIN_TTL_MS)
            .await,
        PollResult::Failed { code: "expired" }
    ));
    let (new_id, _, _) = session_details(recover(&sessions, &db, &provider, &generation, false));
    assert_eq!(
        sessions.lookup(&db, &new_id).unwrap().lease.machine_id,
        machine
    );
}
#[tokio::test]
async fn declined_rebind_leaves_ready_credential_and_does_not_generate_another_grant() {
    let (sessions, db, id, provider, generation) = fixture();
    let transport = MockPoll::new(vec![success("synthetic-uid-a")], false);
    assert!(matches!(
        sessions.poll_using(&db, &id, &transport, || NOW).await,
        PollResult::Completed { .. }
    ));
    let before = sessions.sessions.lock().unwrap().len();
    let result = sessions.start(
        db.clone(),
        StartRequest {
            provider_id: Some(provider.clone()),
            credential_generation: Some(generation),
            rebind: true,
        },
        |_| panic!("must not save existing record"),
        |_| false,
        NOW,
    );
    assert!(matches!(result, StartResult::Failed { code: "cancelled" }));
    assert_eq!(sessions.sessions.lock().unwrap().len(), before);
    assert_eq!(
        db.qoder_http_identity_context(&provider, NOW)
            .unwrap()
            .status,
        IdentityStatus::Ready
    );
    // Cancel is not a claim that an already committed server grant was revoked.
    sessions.cancel(&db, &id);
    assert_eq!(
        db.qoder_http_identity_context(&provider, NOW)
            .unwrap()
            .status,
        IdentityStatus::Ready
    );
}
#[test]
fn native_entry_cannot_adopt_a_pat_provider_or_stale_source_generation() {
    let (sessions, db, _id, provider, generation) = fixture();
    let result = sessions.start(
        db.clone(),
        StartRequest {
            provider_id: Some(provider.clone()),
            credential_generation: Some(format!("stale-{generation}")),
            rebind: false,
        },
        |_| panic!("must not save"),
        |_| panic!("must not confirm stale source"),
        NOW,
    );
    assert!(matches!(
        result,
        StartResult::Failed {
            code: "source_changed"
        }
    ));
    let mut p = db.get_provider_by_id(&provider, "codex").unwrap().unwrap();
    p.meta.as_mut().unwrap().provider_type = Some("qoder_cn".into());
    p.settings_config = serde_json::json!({"env":{"OPENAI_API_KEY":"synthetic-cn-pat"}});
    db.save_provider("codex", &p).unwrap();
    let generation = db
        .get_provider_by_id(&provider, "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
        .generation;
    let result = sessions.start(
        db,
        StartRequest {
            provider_id: Some(provider),
            credential_generation: Some(generation),
            rebind: false,
        },
        |_| panic!("must not save"),
        |_| panic!("must not confirm a PAT source"),
        NOW,
    );
    assert!(matches!(
        result,
        StartResult::Failed {
            code: "source_changed"
        }
    ));
}
