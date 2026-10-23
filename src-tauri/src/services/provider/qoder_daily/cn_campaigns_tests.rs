//! Synthetic-only fixtures. Campaign windows are never interpreted as daily rounds.
use super::*;
use serde_json::json;
fn row(id: &str, status: &str) -> Value {
    json!({"campaignId":id,"campaignKey":"some_campaign","actionType":"CLAIM_BENEFIT",
        "claimStatus":status,"startAt":1767225600_i64,"endAt":1798761600_i64,
        "benefit":{"amount":12.5}})
}
fn parse(value: Value) -> Result<Vec<Campaign>, FailureCode> {
    parse_listing(&serde_json::to_vec(&value).unwrap(), "synthetic-owner")
}
#[test]
fn direct_campaigns_accept_missing_uid_and_do_not_select_a_daily_target() {
    let result =
        parse(json!({"campaigns":[row("a", "CLAIMABLE"),row("b", "CLAIMED"),row("c", "FUTURE")]}))
            .unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].claim_status, ClaimStatus::Claimable);
    assert_eq!(result[1].claim_status, ClaimStatus::Claimed);
    assert_eq!(result[2].claim_status, ClaimStatus::Unknown);
    assert_eq!(result[0].start_at, Some(1767225600000));
    assert_eq!(result[0].benefit_amount, Some(12.5));
}
#[test]
fn optional_uid_must_match_existing_authenticated_owner() {
    assert!(parse(json!({"uid":"synthetic-owner","campaigns":[]})).is_ok());
    for uid in [json!("another-owner"), json!(null), json!(123)] {
        assert_eq!(
            parse(json!({"uid":uid,"campaigns":[]})),
            Err(FailureCode::IdentityMismatch)
        );
    }
}
#[test]
fn ignores_unrelated_activities_and_does_not_require_invented_benefit_fields() {
    let mut unrelated = row("other", "CLAIMABLE");
    unrelated["actionType"] = json!("INVITE");
    let mut no_benefit = row("a", "CLAIMABLE");
    no_benefit.as_object_mut().unwrap().remove("benefit");
    no_benefit.as_object_mut().unwrap().remove("campaignKey");
    let result = parse(json!({"campaigns":[unrelated, no_benefit]})).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].benefit_amount, None);
    assert_eq!(result[0].campaign_key, None);
}
#[test]
fn duplicate_ids_bad_shapes_and_untrusted_envelopes_fail_closed() {
    for body in [
        json!({"campaigns":[row("a","CLAIMED"), row("a","CLAIMABLE")]}),
        json!({"data":{"campaigns":[]}}),
        json!({"campaigns":{}}),
        json!({"campaigns":[null]}),
    ] {
        assert_eq!(parse(body), Err(FailureCode::InvalidResponse));
    }
}
#[test]
fn service_authentication_envelopes_never_look_like_empty_success() {
    for (body, code) in [
        (
            json!({"code":401,"campaigns":[]}),
            FailureCode::AuthenticationRequired,
        ),
        (
            json!({"code":"403","campaigns":[]}),
            FailureCode::VerificationRequired,
        ),
        (json!({"code":429,"campaigns":[]}), FailureCode::RateLimited),
        (
            json!({"success":false,"message":"sensitive token","campaigns":[]}),
            FailureCode::ServiceRejected,
        ),
        (
            json!({"code":"unexpected","campaigns":[]}),
            FailureCode::ServiceRejected,
        ),
    ] {
        assert_eq!(parse(body), Err(code));
    }
}
#[test]
fn bounded_text_array_json_and_time_never_leak_unvalidated_data() {
    assert_eq!(
        parse_listing(b"not json", "synthetic-owner"),
        Err(FailureCode::InvalidResponse)
    );
    assert_eq!(
        parse_listing(&vec![b' '; MAX_BYTES + 1], "synthetic-owner"),
        Err(FailureCode::ResponseTooLarge)
    );
    assert_eq!(
        parse(json!({"campaigns":vec![row("x","CLAIMED"); MAX_CAMPAIGNS+1]})),
        Err(FailureCode::ResponseTooLarge)
    );
    for bad in [json!("a\nsecret"), json!("x".repeat(257)), json!(null)] {
        let mut item = row("a", "CLAIMABLE");
        item["campaignId"] = bad;
        assert_eq!(
            parse(json!({"campaigns":[item]})),
            Err(FailureCode::InvalidResponse)
        );
    }
    let mut item = row("a", "CLAIMABLE");
    item["startAt"] = json!("unknown");
    item["benefit"]["amount"] = json!(-1);
    let result = parse(json!({"campaigns":[item]})).unwrap();
    assert_eq!(result[0].start_at, None);
    assert_eq!(result[0].benefit_amount, None);
}

use crate::services::provider::qoder_http_auth::{
    self as owner, Established, IdentityAction, IdentityActionRequest, PreparedIdentityAction,
    TokenPair,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
fn original_raw(db: &Database) -> String {
    db.conn
        .lock()
        .unwrap()
        .query_row("SELECT qoder_http_secret FROM providers", [], |r| r.get(0))
        .unwrap()
}
fn failed(result: ReadResult, code: FailureCode) {
    assert!(
        matches!(result,ReadResult::Failed{code:c} if c==code),
        "expected fixed failure: {code:?}"
    );
}
struct Server {
    url: reqwest::Url,
    seen: Arc<Mutex<Vec<String>>>,
    entered: tokio::sync::oneshot::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
    join: std::thread::JoinHandle<()>,
}
impl Server {
    fn start(status: u16, extra_headers: &str, body: String, delayed: bool) -> Self {
        use std::{
            io::{Read, Write},
            net::TcpListener,
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
        let (entered, signal) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let response=format!("HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",body.len());
        let join = std::thread::spawn(move || {
            let mut entered = Some(entered);
            let mut deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
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
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0u8; 1024];
                while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buf[..n]);
                    assert!(bytes.len() < 8192);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap().to_ascii_lowercase());
                if let Some(entered) = entered.take() {
                    let _ = entered.send(());
                    if delayed {
                        let _ = gate.recv_timeout(Duration::from_secs(2));
                    }
                }
                let _ = stream.write_all(response.as_bytes());
                deadline = Instant::now() + Duration::from_millis(80);
            }
        });
        Self {
            url,
            seen,
            entered: signal,
            release,
            join,
        }
    }
    fn transport(&self) -> Transport {
        Transport::synthetic(self.url.clone(), Duration::from_secs(1))
    }
    async fn finish(self) -> Vec<String> {
        tokio::task::spawn_blocking(move || self.join.join().unwrap())
            .await
            .unwrap();
        self.seen.lock().unwrap().clone()
    }
}
#[tokio::test]
async fn full_read_uses_only_existing_access_for_one_get_and_no_runtime_identity() {
    for kind in ["qoder_cn", "qoder_cn_device"] {
        let (db, d) = account(kind, NOW + 1);
        let before = original_raw(&db);
        let server = Server::start(
            200,
            "",
            json!({"campaigns":[row("a","CLAIMABLE")]}).to_string(),
            false,
        );
        let transport = server.transport();
        let result = read_using(&db, d, &transport, || NOW).await;
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["status"], "observed");
        assert_eq!(value["runtimeIdentity"], "not_requested");
        assert_eq!(original_raw(&db), before);
        let seen = server.finish().await;
        assert_eq!(seen.len(), 1);
        let request = &seen[0];
        assert!(request.starts_with("get /sash/api/v1/me/campaigns http/1.1"));
        assert!(request.contains("authorization: bearer synthetic-read-access"));
        assert!(request.contains("cosy-clienttype: 10"));
        assert!(request.contains("user-agent: qoder"));
        assert!(request.contains("content-type: application/json"));
        for forbidden in [
            "synthetic-original-pat",
            "synthetic-never-refresh",
            "cosy-machine",
            "/reward",
            "post ",
        ] {
            assert!(!request.contains(forbidden));
        }
        let dto = value.to_string();
        for secret in [
            "synthetic-owner",
            "synthetic-read-access",
            "synthetic-never-refresh",
            "machineId",
            "receipt",
            "addedCredits",
        ] {
            assert!(!dto.contains(secret));
        }
    }
}
#[tokio::test]
async fn read_rejects_international_wrong_declaration_and_expired_before_network() {
    let transport = Transport::synthetic(
        reqwest::Url::parse("http://127.0.0.1:1/sash/api/v1/me/campaigns").unwrap(),
        Duration::from_millis(50),
    );
    let (db, d) = account("qoder_sdk", NOW + 600_000);
    failed(
        read_using(&db, d, &transport, || NOW).await,
        FailureCode::InvalidSource,
    );
    let (db, mut d) = account("qoder_cn", NOW + 1);
    let before = original_raw(&db);
    d.credential_generation = "stale".into();
    failed(
        read_using(&db, d.clone(), &transport, || NOW).await,
        FailureCode::SourceChanged,
    );
    d.credential_generation = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap()
        .meta
        .unwrap()
        .qoder_campaign_state
        .unwrap()
        .generation;
    failed(
        read_using(&db, d, &transport, || NOW + 1).await,
        FailureCode::IdentityExpired,
    );
    assert_eq!(original_raw(&db), before);
}
#[tokio::test]
async fn redirects_auth_rate_limits_service_errors_and_large_bodies_are_fixed_nonretrying_results()
{
    for (status, headers, body, code) in [
        (
            302,
            "Location: http://127.0.0.1:1/reward\r\n",
            String::new(),
            FailureCode::RedirectRejected,
        ),
        (
            401,
            "",
            "sensitive response".into(),
            FailureCode::AuthenticationRequired,
        ),
        (
            403,
            "",
            "sensitive response".into(),
            FailureCode::VerificationRequired,
        ),
        (
            429,
            "",
            "sensitive response".into(),
            FailureCode::RateLimited,
        ),
        (503, "", "sensitive response".into(), FailureCode::HttpError),
        (
            200,
            "",
            json!({"code":401,"campaigns":[]}).to_string(),
            FailureCode::AuthenticationRequired,
        ),
        (200, "", "invalid json".into(), FailureCode::InvalidResponse),
        (
            200,
            "",
            "x".repeat(MAX_BYTES + 1),
            FailureCode::ResponseTooLarge,
        ),
    ] {
        let (db, d) = account("qoder_cn", NOW + 600_000);
        let before = original_raw(&db);
        let server = Server::start(status, headers, body, false);
        let transport = server.transport();
        failed(read_using(&db, d, &transport, || NOW).await, code);
        assert_eq!(original_raw(&db), before);
        assert_eq!(server.finish().await.len(), 1);
    }
}
#[tokio::test]
async fn stale_owner_source_realm_lock_epoch_and_expiry_never_publish_inflight_results() {
    for change in ["source", "realm", "lock", "epoch", "owner", "expiry"] {
        let (db, d) = account("qoder_cn", NOW + 600_000);
        let provider_id = d.provider_id.clone();
        let mut server = Server::start(
            200,
            "",
            json!({"campaigns":[row("a","CLAIMED")]}).to_string(),
            true,
        );
        let transport = server.transport();
        let now = std::sync::atomic::AtomicI64::new(NOW);
        let action = async {
            (&mut server.entered).await.unwrap();
            match change {
                "source" | "realm" => {
                    let mut p = db
                        .get_provider_by_id(&provider_id, "codex")
                        .unwrap()
                        .unwrap();
                    if change == "source" {
                        p.settings_config["env"]["OPENAI_API_KEY"] = json!("synthetic-new-pat");
                    } else {
                        p.meta.as_mut().unwrap().provider_type = Some("qoder_sdk".into());
                    }
                    db.save_provider("codex", &p).unwrap();
                }
                "lock" => db.secrets.set_blocked(true),
                "epoch" => {
                    db.secrets.set_blocked(true);
                    db.secrets.set_blocked(false);
                }
                "owner" => {
                    db.conn
                        .lock()
                        .unwrap()
                        .execute("UPDATE providers SET qoder_http_secret=''", [])
                        .unwrap();
                }
                "expiry" => now.store(NOW + 600_000, std::sync::atomic::Ordering::SeqCst),
                _ => unreachable!(),
            }
            server.release.send(()).unwrap();
        };
        let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                read_using(&db, d, &transport, || now
                    .load(std::sync::atomic::Ordering::SeqCst)),
                action
            )
        })
        .await
        .unwrap();
        assert!(
            matches!(result, ReadResult::Failed { .. }),
            "must suppress stale response: {change}"
        );
        assert_eq!(server.finish().await.len(), 1);
    }
}
#[tokio::test]
async fn timeout_is_fixed_and_no_second_request_is_sent() {
    let (db, d) = account("qoder_cn_device", NOW + 600_000);
    let server = Server::start(200, "", json!({"campaigns":[]}).to_string(), true);
    let transport = Transport::synthetic(server.url.clone(), Duration::from_millis(30));
    failed(
        read_using(&db, d, &transport, || NOW).await,
        FailureCode::Timeout,
    );
    server.release.send(()).unwrap();
    assert_eq!(server.finish().await.len(), 1);
}

#[tokio::test]
async fn request_fence_blocks_a_change_after_snapshot_before_any_get() {
    let transport = Transport::synthetic(
        reqwest::Url::parse("http://127.0.0.1:1/sash/api/v1/me/campaigns").unwrap(),
        Duration::from_millis(50),
    );
    for change in ["epoch", "expiry"] {
        let (db, d) = account("qoder_cn_device", NOW + 1);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = read_using(&db, d, &transport, || {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                if change == "epoch" {
                    db.secrets.set_blocked(true);
                    db.secrets.set_blocked(false);
                } else {
                    return NOW + 1;
                }
            }
            NOW
        })
        .await;
        failed(
            result,
            if change == "epoch" {
                FailureCode::OwnerChanged
            } else {
                FailureCode::IdentityExpired
            },
        );
    }
}
#[tokio::test]
async fn pending_refresh_unknown_and_missing_owner_cannot_start_get_or_change_owner() {
    let transport = Transport::synthetic(
        reqwest::Url::parse("http://127.0.0.1:1/sash/api/v1/me/campaigns").unwrap(),
        Duration::from_millis(50),
    );
    let (db, d) = account("qoder_cn", NOW + 1);
    let provider = db
        .get_provider_by_id(&d.provider_id, "codex")
        .unwrap()
        .unwrap();
    let source = owner::SourceBinding::from_provider(&provider).unwrap().0;
    let owner::Acquisition::Refresh(lease) = db.qoder_http_acquire(&source, NOW).unwrap() else {
        panic!("synthetic refresh reservation")
    };
    let pending = original_raw(&db);
    failed(
        read_using(&db, d.clone(), &transport, || NOW).await,
        FailureCode::IdentityPending,
    );
    assert_eq!(original_raw(&db), pending);
    db.qoder_http_mark_unknown(&lease.stamp, &lease.attempt_id)
        .unwrap();
    let unknown = original_raw(&db);
    failed(
        read_using(&db, d.clone(), &transport, || NOW).await,
        FailureCode::IdentityUnknown,
    );
    assert_eq!(original_raw(&db), unknown);
    db.conn
        .lock()
        .unwrap()
        .execute("UPDATE providers SET qoder_http_secret=''", [])
        .unwrap();
    failed(
        read_using(&db, d, &transport, || NOW).await,
        FailureCode::IdentityRequired,
    );
    assert!(original_raw(&db).is_empty());
}
#[test]
fn status_error_envelopes_cannot_override_an_authentication_failure() {
    for (status, code) in [
        (json!(401), FailureCode::AuthenticationRequired),
        (json!("UNAUTHORIZED"), FailureCode::AuthenticationRequired),
        (json!("FORBIDDEN"), FailureCode::VerificationRequired),
        (json!("ERROR"), FailureCode::ServiceRejected),
    ] {
        assert_eq!(parse(json!({"status":status,"campaigns":[]})), Err(code));
    }
}
#[test]
fn timestamp_contract_is_unix_seconds_only_without_daily_inference() {
    for bad in [
        json!("2026-01-01T00:00:00Z"),
        json!(1767225600000_i64),
        json!(0),
        json!(-1),
        json!(i64::MAX),
    ] {
        let mut item = row("a", "CLAIMED");
        item["startAt"] = bad;
        assert_eq!(
            parse(json!({"campaigns":[item]})).unwrap()[0].start_at,
            None
        );
    }
    let mut item = row("a", "CLAIMED");
    item["endAt"] = json!(1);
    let listing = parse(json!({"campaigns":[item]})).unwrap();
    assert_eq!(listing[0].start_at, None);
    assert_eq!(listing[0].end_at, None);
    assert_eq!(listing[0].claim_status, ClaimStatus::Claimed);
}
