use super::*;
use owner::{
    Established, IdentityAction, IdentityActionRequest, PreparedIdentityAction, TokenPair,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
const NOW: i64 = 1_000_000;
fn account(kind: &str, expires: i64) -> (Database, PatDeclaration) {
    let db = Database::memory().unwrap();
    let mut provider = crate::provider::Provider::with_id(
        "synthetic-cn-read".into(),
        "Synthetic".into(),
        if kind == "qoder_cn_device" {
            json!({})
        } else {
            json!({"env":{"OPENAI_API_KEY":"synthetic-original-pat"}})
        },
        None,
    );
    provider.meta = Some(crate::provider::ProviderMeta {
        provider_type: Some(kind.into()),
        ..Default::default()
    });
    db.save_provider("codex", &provider).unwrap();
    let p = db
        .get_provider_by_id(&provider.id, "codex")
        .unwrap()
        .unwrap();
    let source = owner::SourceBinding::from_provider(&p).unwrap().0;
    let d = PatDeclaration {
        provider_id: p.id,
        credential_generation: source.credential_generation,
        source: source.source.declaration(),
    };
    let PreparedIdentityAction::Establish { receipt, .. } = db
        .qoder_http_prepare_identity_action(
            &IdentityActionRequest {
                declaration: d.clone(),
                action: IdentityAction::Establish,
            },
            NOW,
        )
        .unwrap()
    else {
        panic!()
    };
    let lease = db.qoder_http_reserve_identity_action(receipt).unwrap();
    db.qoder_http_publish_establish(
        &lease,
        Established {
            uid: "synthetic-owner".into(),
            tokens: TokenPair {
                access_token: "synthetic-read-access".into(),
                refresh_token: "synthetic-never-refresh".into(),
                expires_at_ms: expires,
            },
        },
        NOW,
    )
    .unwrap();
    (db, d)
}
struct Auth(owner::SourceKind);
impl owner::AuthTransport for Auth {
    fn source_kind(&self) -> owner::SourceKind {
        self.0.clone()
    }
    fn exchange_and_identify<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Established, ()>> + Send + 'a>>
    {
        Box::pin(async { panic!("must not exchange") })
    }
    fn refresh<'a>(
        &'a self,
        _: &'a str,
        _: &'a owner::Identity,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<TokenPair, ()>> + Send + 'a>>
    {
        Box::pin(async { panic!("must not refresh fresh token") })
    }
}
struct Server {
    url: reqwest::Url,
    seen: Arc<Mutex<Vec<String>>>,
    join: Option<std::thread::JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}
impl Server {
    fn start(responses: Vec<(u16, Value)>, hook: Option<Box<dyn Fn(usize) + Send>>) -> Self {
        Self::start_with_body_delay(responses, hook, std::time::Duration::ZERO)
    }
    fn start_with_body_delay(
        responses: Vec<(u16, Value)>,
        hook: Option<Box<dyn Fn(usize) + Send>>,
        delay: std::time::Duration,
    ) -> Self {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            time::Duration,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = reqwest::Url::parse(&format!(
            "http://{}/sash/api/v1/me/campaigns",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let join = std::thread::spawn(move || {
            let mut responses = responses.into_iter();
            while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0; 1024];
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(pos) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..pos]);
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|s| s.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= pos + 4 + len {
                            break;
                        }
                    }
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap());
                let count = captured.lock().unwrap().len();
                if let Some(hook) = &hook {
                    hook(count)
                }
                let (status, body) = responses.next().expect("unexpected network request");
                let body = body.to_string();
                let response=format!("HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nDate: Thu, 01 Jan 1970 00:16:40 GMT\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                if count == 1 && !delay.is_zero() {
                    let split = response.find("\r\n\r\n").unwrap() + 4;
                    let _ = stream.write_all(&response.as_bytes()[..split]);
                    std::thread::sleep(delay);
                    let _ = stream.write_all(&response.as_bytes()[split..]);
                } else {
                    let _ = stream.write_all(response.as_bytes());
                }
            }
        });
        Self {
            url,
            seen,
            join: Some(join),
            stop,
        }
    }
    async fn finish(mut self) -> Vec<String> {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let join = self.join.take().unwrap();
        tokio::task::spawn_blocking(move || join.join().unwrap())
            .await
            .unwrap();
        self.seen.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
fn row(id: &str) -> Value {
    json!({"campaignId":id,"actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMABLE"})
}
fn listed(rows: Vec<Value>) -> Value {
    json!({"campaigns":rows})
}
fn receipts(db: &Database) -> Vec<CnOperation> {
    ledger::read_cn_on(
        &db.conn.lock().unwrap(),
        &super::super::contract::Identity {
            provider_id: "synthetic-cn-read".into(),
            uid: "synthetic-owner".into(),
            region: super::super::contract::Region::China,
        },
    )
    .unwrap()
}
#[tokio::test]
async fn real_chain_posts_each_minimal_free_benefit_and_persists_unknown_delta() {
    for (kind, source) in [
        ("qoder_cn", owner::SourceKind::ChinaQoderPat),
        ("qoder_cn_device", owner::SourceKind::ChinaQoderDevice),
    ] {
        let (db, d) = account(kind, NOW + 600_000);
        let server = Server::start(
            vec![
                (
                    200,
                    listed(vec![
                        row("benefit:2026"),
                        row("a.b"),
                        row("活动"),
                        row("campaign/one"),
                    ]),
                ),
                (200, json!({"status":"CLAIMED"})),
                (200, json!({"data":{"status":"CLAIMED"}})),
                (200, json!({"status":"CLAIMED"})),
                (200, json!({"status":"CLAIMED"})),
            ],
            None,
        );
        let t = Transport::synthetic(server.url.clone());
        let result = run_using(&db, d, None, &t, &Auth(source), || NOW).await;
        let dto = serde_json::to_value(result).unwrap();
        assert_eq!(dto["status"], "completed");
        assert_eq!(dto["activities"].as_array().unwrap().len(), 4);
        assert!(dto["activities"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["outcome"] == "claimedDeltaUnknown"));
        assert_eq!(receipts(&db).len(), 4);
        assert!(receipts(&db)
            .iter()
            .all(|r| r.outcome == super::super::contract::Outcome::ClaimedDeltaUnknown));
        let seen = server.finish().await;
        assert_eq!(seen.len(), 5);
        assert!(seen[1].starts_with("POST /sash/api/v1/me/campaigns/benefit:2026/claim "));
        assert!(seen[2].starts_with("POST /sash/api/v1/me/campaigns/a.b/claim "));
        assert!(seen[4].starts_with("POST /sash/api/v1/me/campaigns/campaign%2Fone/claim "));
        assert!(seen[1].ends_with("{}"));
        assert!(!dto.to_string().contains("synthetic-read-access"));
        assert!(!dto.to_string().contains("addedCredits"));
    }
}
#[tokio::test]
async fn unknown_sent_post_reconciles_only_get_without_repeat_even_if_claimable_or_window_changed()
{
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let mut changed = row("c1");
    changed["startAt"] = json!(2000);
    changed["endAt"] = json!(3000);
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1")])),
            (500, json!({})),
            (200, listed(vec![row("c1")])),
            (200, listed(vec![changed])),
            (
                200,
                listed(vec![
                    json!({"campaignId":"c1","actionType":"CLAIM_BENEFIT","claimStatus":"CLAIMED"}),
                ]),
            ),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    for _ in 0..4 {
        let _ = run_using(
            &db,
            d.clone(),
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await;
    }
    let seen = server.finish().await;
    assert_eq!(seen.iter().filter(|r| r.starts_with("POST ")).count(), 1);
    assert_eq!(seen.iter().filter(|r| r.starts_with("GET ")).count(), 4);
    assert_eq!(
        receipts(&db)[0].outcome,
        super::super::contract::Outcome::ClaimedDeltaUnknown
    );
}
#[tokio::test]
async fn unsupported_conditions_and_failed_sibling_do_not_hide_other_free_activity() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let mut task = row("task");
    task["requiredAchievementKey"] = json!("invite");
    let server = Server::start(
        vec![
            (200, listed(vec![task, row("bad"), row("good")])),
            (500, json!({})),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    let result = serde_json::to_value(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await,
    )
    .unwrap();
    assert_eq!(result["activities"][0]["reason"], "achievement_required");
    assert_eq!(result["activities"][1]["outcome"], "unknown");
    assert_eq!(result["activities"][2]["outcome"], "claimedDeltaUnknown");
    assert_eq!(server.finish().await.len(), 3);
}
#[tokio::test]
async fn vault_lock_after_post_suppresses_receipt_and_stops_next_activity() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let db = Arc::new(db);
    let guard = db.clone();
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1"), row("c2")])),
            (200, json!({"status":"CLAIMED"})),
        ],
        Some(Box::new(move |count| {
            if count == 2 {
                guard.secrets.set_blocked(true)
            }
        })),
    );
    let t = Transport::synthetic(server.url.clone());
    assert!(matches!(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW
        )
        .await,
        RunResult::Failed { .. }
    ));
    let seen = server.finish().await;
    assert_eq!(seen.len(), 2);
    assert_eq!(
        receipts(&db)[0].outcome,
        super::super::contract::Outcome::Unknown
    );
}
#[tokio::test]
async fn policy_revision_revoked_during_listing_stops_before_any_intent() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let db = Arc::new(db);
    let guard = db.clone();
    let request = automatic::Request {
        provider_id: d.provider_id.clone(),
        credential_generation: d.credential_generation.clone(),
        expected_revision: 0,
        policy_version: automatic::CN_POLICY_VERSION,
        enabled: false,
        source: None,
    };
    let server = Server::start(
        vec![(200, listed(vec![row("c1"), row("c2")]))],
        Some(Box::new(move |_| {
            let ctx = automatic::context(&guard, &request.provider_id).unwrap();
            automatic::set(
                &guard,
                automatic::Request {
                    provider_id: request.provider_id.clone(),
                    credential_generation: request.credential_generation.clone(),
                    expected_revision: ctx.revision,
                    policy_version: automatic::CN_POLICY_VERSION,
                    enabled: false,
                    source: None,
                },
            )
            .unwrap();
        })),
    );
    let t = Transport::synthetic(server.url.clone());
    assert!(matches!(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW
        )
        .await,
        RunResult::Failed { .. }
    ));
    assert_eq!(server.finish().await.len(), 1);
    assert!(receipts(&db).is_empty());
}
#[tokio::test]
async fn provided_intervals_use_server_date_not_local_natural_day() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let mut early = row("early");
    early["startAt"] = json!(1100);
    let mut ended = row("ended");
    ended["endAt"] = json!(999);
    let mut open = row("open");
    open["startAt"] = json!(900);
    open["endAt"] = json!(1100);
    let server = Server::start(
        vec![
            (200, listed(vec![early, ended, open])),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    let result = serde_json::to_value(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await,
    )
    .unwrap();
    assert_eq!(result["activities"][0]["reason"], "campaign_not_started");
    assert_eq!(result["activities"][1]["reason"], "campaign_ended");
    assert_eq!(result["activities"][2]["outcome"], "claimedDeltaUnknown");
    assert_eq!(server.finish().await.len(), 2);
}
#[tokio::test]
async fn actual_commit_failure_rolls_back_intent_and_never_posts() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let path = std::env::temp_dir().join(format!(
        "qoder-cn-commit-test-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    {
        let mut conn = db.conn.lock().unwrap();
        conn.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        *conn = rusqlite::Connection::open(&path).unwrap();
        conn.busy_timeout(std::time::Duration::ZERO).unwrap();
    }
    let reader = rusqlite::Connection::open(&path).unwrap();
    let calls = std::cell::Cell::new(0);
    let server = Server::start(vec![(200, listed(vec![row("c1")]))], None);
    let t = Transport::synthetic(server.url.clone());
    assert!(matches!(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || {
                let n = calls.get() + 1;
                calls.set(n);
                if n == 6 {
                    reader
                        .execute_batch("BEGIN;SELECT count(*) FROM providers;")
                        .unwrap();
                }
                NOW
            }
        )
        .await,
        RunResult::Failed { .. }
    ));
    assert_eq!(server.finish().await.len(), 1);
    assert_eq!(calls.get(), 6);
    let inserted: i64 = db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT total_changes()", [], |r| r.get(0))
        .unwrap();
    assert_eq!(inserted, 1, "the insert ran before actual COMMIT failed");
    assert!(receipts(&db).is_empty());
    reader.execute_batch("ROLLBACK;").unwrap();
    drop(reader);
    drop(db);
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn persisted_unknown_survives_database_reopen_and_is_get_only() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1")])),
            (500, json!({})),
            (200, listed(vec![row("c1")])),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    let _ = run_using(
        &db,
        d.clone(),
        None,
        &t,
        &Auth(owner::SourceKind::ChinaQoderPat),
        || NOW,
    )
    .await;
    let path = std::env::temp_dir().join(format!(
        "qoder-cn-benefit-test-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    {
        let mut conn = db.conn.lock().unwrap();
        conn.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        *conn = rusqlite::Connection::open(&path).unwrap();
    }
    let result = serde_json::to_value(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await,
    )
    .unwrap();
    assert_eq!(result["activities"][0]["reason"], "previous_unknown");
    assert_eq!(
        server
            .finish()
            .await
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        1
    );
    assert_eq!(
        receipts(&db)[0].outcome,
        super::super::contract::Outcome::Unknown
    );
    drop(db);
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
async fn slow_listing_body_does_not_reset_authenticated_window_clock() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let mut expired = row("c1");
    expired["endAt"] = json!(1002);
    let server = Server::start_with_body_delay(
        vec![
            (200, listed(vec![expired])),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
        std::time::Duration::from_millis(2200),
    );
    let t = Transport::synthetic(server.url.clone());
    let result = serde_json::to_value(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await,
    )
    .unwrap();
    assert_eq!(result["activities"][0]["reason"], "campaign_ended");
    assert_eq!(server.finish().await.len(), 1);
    assert!(receipts(&db).is_empty());
}
#[tokio::test]
async fn only_post_receipt_then_independent_claimed_then_later_claimable_rearms() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let mut claimed = row("c1");
    claimed["claimStatus"] = json!("CLAIMED");
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1")])),
            (200, json!({"status":"CLAIMED"})),
            (200, listed(vec![row("c1")])),
            (200, listed(vec![claimed])),
            (200, listed(vec![row("c1")])),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    for round in 0..4 {
        let result = serde_json::to_value(
            run_using(
                &db,
                d.clone(),
                None,
                &t,
                &Auth(owner::SourceKind::ChinaQoderPat),
                || NOW,
            )
            .await,
        )
        .unwrap();
        assert_eq!(result["status"], "completed");
        if round == 2 {
            assert!(receipts(&db)[0].listing_claimed);
        } else {
            assert!(!receipts(&db)[0].listing_claimed);
        }
    }
    assert_eq!(
        server
            .finish()
            .await
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        2
    );
    assert!(receipts(&db)[0].receipt_confirmed);
}
#[tokio::test]
async fn reconciled_unknown_never_gains_no_window_replay_even_after_later_claimed() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let mut claimed = row("c1");
    claimed["claimStatus"] = json!("CLAIMED");
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1")])),
            (500, json!({})),
            (200, listed(vec![claimed.clone()])),
            (200, listed(vec![claimed])),
            (200, listed(vec![row("c1")])),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    for round in 0..4 {
        let result = serde_json::to_value(
            run_using(
                &db,
                d.clone(),
                None,
                &t,
                &Auth(owner::SourceKind::ChinaQoderPat),
                || NOW,
            )
            .await,
        )
        .unwrap();
        assert_eq!(result["status"], "completed");
        if round >= 1 {
            assert_eq!(result["activities"][0]["reason"], "reconciled_no_new_round");
            assert!(!receipts(&db)[0].receipt_confirmed);
            assert!(!receipts(&db)[0].listing_claimed);
        }
    }
    assert_eq!(
        server
            .finish()
            .await
            .iter()
            .filter(|r| r.starts_with("POST "))
            .count(),
        1
    );
}
#[tokio::test]
async fn business_blocked_http200_stops_account_with_durable_unknown() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1"), row("c2")])),
            (200, json!({"data":{"code":"SAME_PERSON_ALREADY_CLAIMED"}})),
        ],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    assert!(matches!(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW
        )
        .await,
        RunResult::Failed {
            code: FailureCode::VerificationRequired
        }
    ));
    assert_eq!(server.finish().await.len(), 2);
    assert_eq!(
        receipts(&db)[0].outcome,
        super::super::contract::Outcome::Unknown
    );
}
#[tokio::test]
async fn batch_budget_keeps_sent_unknown_stops_later_items_and_releases_next_account() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = Server::start(
        vec![
            (200, listed(vec![row("c1"), row("c2")])),
            (200, json!({"status":"CLAIMED"})),
            (200, json!({"status":"CLAIMED"})),
        ],
        Some(Box::new(move |count| {
            if count == 2 {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(std::time::Duration::from_secs(2));
            }
        })),
    );
    let t = Transport::synthetic(server.url.clone());
    let result = run_budget(
        &db,
        d.clone(),
        None,
        &t,
        &Auth(owner::SourceKind::ChinaQoderPat),
        || NOW,
        std::time::Duration::from_millis(500),
    )
    .await;
    entered_rx
        .try_recv()
        .expect("test reached durable POST, not a preflight timeout");
    release_tx.send(()).unwrap();
    assert!(matches!(
        result,
        RunResult::Failed {
            code: FailureCode::Timeout
        }
    ));
    assert_eq!(server.finish().await.len(), 2);
    assert_eq!(receipts(&db).len(), 1);
    assert_eq!(
        receipts(&db)[0].outcome,
        super::super::contract::Outcome::Unknown
    );
    let recovery = Server::start(vec![(200, listed(vec![row("c1")]))], None);
    let t = Transport::synthetic(recovery.url.clone());
    let result = serde_json::to_value(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await,
    )
    .unwrap();
    assert_eq!(result["activities"][0]["reason"], "previous_unknown");
    assert_eq!(recovery.finish().await.len(), 1);
    let (next, d) = account("qoder_cn", NOW + 600_000);
    let next_server = Server::start(
        vec![
            (200, listed(vec![row("next")])),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
    );
    let t = Transport::synthetic(next_server.url.clone());
    assert!(matches!(
        run_using(
            &next,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW
        )
        .await,
        RunResult::Completed { .. }
    ));
    assert_eq!(next_server.finish().await.len(), 2);
}
#[tokio::test]
async fn rate_limit_stops_account_without_retry_or_discarding_unknown() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let server = Server::start(
        vec![(200, listed(vec![row("c1"), row("c2")])), (429, json!({}))],
        None,
    );
    let t = Transport::synthetic(server.url.clone());
    assert!(matches!(
        run_using(
            &db,
            d,
            None,
            &t,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW
        )
        .await,
        RunResult::Failed {
            code: FailureCode::RateLimited
        }
    ));
    assert_eq!(server.finish().await.len(), 2);
    assert_eq!(
        receipts(&db)[0].outcome,
        super::super::contract::Outcome::Unknown
    );
}
fn gated_listing(
    response: Value,
) -> (
    Server,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = Server::start(
        vec![(200, response), (200, json!({"status":"CLAIMED"}))],
        Some(Box::new(move |count| {
            if count == 1 {
                entered_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
        })),
    );
    (server, entered_rx, release_tx)
}
#[tokio::test]
async fn get_started_before_post_confirmation_cannot_grant_later_observation_marker() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let db = Arc::new(db);
    let mut claimed = row("c1");
    claimed["claimStatus"] = json!("CLAIMED");
    let (a, entered, release) = gated_listing(listed(vec![claimed]));
    let at = Transport::synthetic(a.url.clone());
    let adb = db.clone();
    let ad = d.clone();
    let task = tokio::spawn(async move {
        run_using(
            &adb,
            ad,
            None,
            &at,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await
    });
    tokio::task::spawn_blocking(move || {
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    })
    .await
    .unwrap();
    let b = Server::start(
        vec![
            (200, listed(vec![row("c1")])),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
    );
    let bt = Transport::synthetic(b.url.clone());
    assert!(matches!(
        run_using(
            &db,
            d,
            None,
            &bt,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW
        )
        .await,
        RunResult::Completed { .. }
    ));
    assert!(receipts(&db)[0].receipt_confirmed);
    release.send(()).unwrap();
    let _ = task.await.unwrap();
    assert!(
        !receipts(&db)[0].listing_claimed,
        "an old in-flight GET is not evidence observed after the receipt"
    );
    assert_eq!(a.finish().await.len(), 1);
    assert_eq!(b.finish().await.len(), 2);
}
#[tokio::test]
async fn get_started_before_marker_cannot_consume_marker_written_while_inflight() {
    let (db, d) = account("qoder_cn", NOW + 600_000);
    let db = Arc::new(db);
    let first = Server::start(
        vec![
            (200, listed(vec![row("c1")])),
            (200, json!({"status":"CLAIMED"})),
        ],
        None,
    );
    let ft = Transport::synthetic(first.url.clone());
    let _ = run_using(
        &db,
        d.clone(),
        None,
        &ft,
        &Auth(owner::SourceKind::ChinaQoderPat),
        || NOW,
    )
    .await;
    first.finish().await;
    let (a, entered, release) = gated_listing(listed(vec![row("c1")]));
    let at = Transport::synthetic(a.url.clone());
    let adb = db.clone();
    let ad = d.clone();
    let task = tokio::spawn(async move {
        run_using(
            &adb,
            ad,
            None,
            &at,
            &Auth(owner::SourceKind::ChinaQoderPat),
            || NOW,
        )
        .await
    });
    tokio::task::spawn_blocking(move || {
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    })
    .await
    .unwrap();
    let mut claimed = row("c1");
    claimed["claimStatus"] = json!("CLAIMED");
    let b = Server::start(vec![(200, listed(vec![claimed]))], None);
    let bt = Transport::synthetic(b.url.clone());
    let _ = run_using(
        &db,
        d,
        None,
        &bt,
        &Auth(owner::SourceKind::ChinaQoderPat),
        || NOW,
    )
    .await;
    assert!(receipts(&db)[0].listing_claimed);
    release.send(()).unwrap();
    let _ = task.await.unwrap();
    assert_eq!(
        a.finish().await.len(),
        1,
        "a GET predating the marker cannot create another POST"
    );
    assert!(receipts(&db)[0].listing_claimed);
    assert_eq!(b.finish().await.len(), 1);
}
