use super::{checkin::ClaimState, engine::Engine, model::*, protocol::*, store};
use crate::secrets::session::SecretSession;
use serde_json::{json, Value};
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Mutex};
const NOW: i64 = 1_800_000_000_000;
struct Fake {
    queue: Mutex<VecDeque<(Endpoint, Result<Value, Failure>)>>,
    calls: Mutex<Vec<Endpoint>>,
}
impl Fake {
    fn new(items: Vec<(Endpoint, Result<Value, Failure>)>) -> Self {
        Self {
            queue: Mutex::new(items.into()),
            calls: Mutex::new(vec![]),
        }
    }
}
impl Transport for Fake {
    fn request<'a>(
        &'a self,
        e: Endpoint,
        _a: Option<&'a Account>,
        _s: Option<&'a str>,
        _n: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(e);
            let (expected, result) = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected network effect");
            assert_eq!(e, expected);
            result
        })
    }
}
fn resource(balance: f64) -> Vec<(Endpoint, Result<Value, Failure>)> {
    vec![
        (
            Endpoint::Summary,
            Ok(
                json!({"code":0,"data":{"Packages":[{"PackageCode":"p","CycleCapacityRemainPrecise":balance}]}}),
            ),
        ),
        (Endpoint::Paid, Ok(json!({"code":0,"data":{"Accounts":[]}}))),
        (Endpoint::Free, Ok(json!({"code":0,"data":{"Accounts":[]}}))),
    ]
}
fn status(checked: bool) -> (Endpoint, Result<Value, Failure>) {
    (
        Endpoint::Status,
        Ok(json!({"code":0,"data":{"active":true,"today_checked_in":checked}})),
    )
}
fn setup() -> (std::sync::Arc<SecretSession>, String) {
    let s = SecretSession::ephemeral().unwrap();
    let b = store::binding(&s).unwrap();
    let account = Account {
        uid: "synthetic-uid".into(),
        domain: Domain::CodeBuddy,
        label: "Synthetic account".into(),
        token: "SYNTHETIC_TOKEN_CANARY".into(),
    };
    let id = account.id(&b.vault_id);
    store::save(&s, &b, vec![SavedAccount::new(account)]).unwrap();
    (s, id)
}
#[tokio::test]
async fn listing_and_refresh_are_read_only_for_rewards_and_redacted() {
    let (s, id) = setup();
    let mut q = resource(10.0);
    q.push(status(false));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    let rows = e.list().await.unwrap();
    assert!(f.calls.lock().unwrap().is_empty());
    assert_eq!(rows[0].credits.total_remaining, None);
    let row = e.refresh(&id).await.unwrap();
    assert_eq!(row.credits.total_remaining, Some(10.0));
    assert_eq!(row.claim_state, ClaimState::Available);
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
    let dto = serde_json::to_string(&row).unwrap();
    assert!(!dto.contains("SYNTHETIC_TOKEN_CANARY"));
    assert!(!dto.contains("synthetic-uid"));
    let bytes = std::fs::read(s.root().join(store::FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("SYNTHETIC_TOKEN_CANARY"));
}
#[tokio::test]
async fn explicit_claim_reads_back_status_and_actual_credits() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    q.push((Endpoint::Claim, Ok(json!({"code":0,"credit":20}))));
    q.push(status(true));
    q.extend(resource(30.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    let row = e.claim(&id).await.unwrap();
    assert_eq!(row.claim_state, ClaimState::Claimed);
    assert_eq!(row.credited, Some(20.0));
    assert!(!row.can_claim);
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
    assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
        .pending
        .is_none());
}
#[tokio::test]
async fn lost_receipt_survives_restart_and_next_click_queries_without_resubmitting() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    q.push((Endpoint::Claim, Err(Failure::Unconfirmed)));
    q.push(status(true));
    q.extend(resource(30.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    assert_eq!(
        e.claim(&id).await.unwrap().claim_state,
        ClaimState::Unconfirmed
    );
    assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
        .pending
        .is_some());
    let mut q = vec![status(true)];
    q.extend(resource(30.0));
    let after_restart = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &after_restart,
        clock: &super::clock::FixedClock(NOW),
    };
    assert_eq!(
        e.claim(&id).await.unwrap().claim_state,
        ClaimState::Unconfirmed
    );
    assert!(!after_restart
        .calls
        .lock()
        .unwrap()
        .contains(&Endpoint::Claim));
}
#[tokio::test]
async fn already_or_inactive_or_verification_does_not_submit() {
    for response in [
        json!({"code":0,"data":{"today_checked_in":true}}),
        json!({"code":10085}),
        json!({"code":0,"data":{"active":false,"today_checked_in":false}}),
        json!({"code":0,"data":{}}),
    ] {
        let (s, id) = setup();
        let f = Fake::new(vec![(Endpoint::Status, Ok(response))]);
        let e = Engine {
            session: &s,
            transport: &f,
            clock: &super::clock::FixedClock(NOW),
        };
        let row = e.claim(&id).await.unwrap();
        assert_ne!(row.claim_state, ClaimState::Claimed);
        assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
    }
}
#[tokio::test]
async fn next_cn_day_does_not_show_yesterdays_claim_as_today() {
    let (s, id) = setup();
    let b = store::binding(&s).unwrap();
    let mut rows = store::load(&s, &b).unwrap();
    rows[0].claim_day = today(NOW);
    rows[0].claim_state = ClaimState::Claimed;
    rows[0].credited = Some(20.0);
    store::save(&s, &b, rows).unwrap();
    let f = Fake::new(vec![]);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW + 86400000),
    };
    let rows = e.list().await.unwrap();
    assert_eq!(rows[0].id, id);
    assert_eq!(rows[0].claim_state, ClaimState::Unconfirmed);
    assert_eq!(rows[0].credited, None);
    assert!(rows[0].can_claim);
}
#[test]
fn identities_are_bound_to_owner_and_official_cn_domain() {
    let a = Account {
        uid: "same-uid".into(),
        domain: Domain::CodeBuddy,
        label: "Account".into(),
        token: "synthetic".into(),
    };
    let mut b = a.clone();
    b.domain = Domain::WorkBuddy;
    assert_ne!(a.id("owner-a"), b.id("owner-a"));
    assert_ne!(a.id("owner-a"), a.id("owner-b"));
    for host in [
        "www.workbuddy.ai",
        "evil.example",
        "www.codebuddy.cn.evil.example",
        "",
    ] {
        assert_eq!(Domain::parse(host), Err(Failure::UnsupportedContext));
    }
}
#[tokio::test]
async fn pending_unclaimed_is_unlocked_for_next_explicit_click_without_same_click_submit() {
    let (s, id) = setup();
    let b = store::binding(&s).unwrap();
    let mut rows = store::load(&s, &b).unwrap();
    rows[0].pending = Some(Attempt {
        day: today(NOW),
        before: Some(10.0),
        expected: None,
        receipt: ClaimState::Unconfirmed,
    });
    store::save(&s, &b, rows).unwrap();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    assert_eq!(
        e.claim(&id).await.unwrap().claim_state,
        ClaimState::Available
    );
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
    assert!(store::load(&s, &b).unwrap()[0].pending.is_none());
}
struct MovingClock(std::sync::atomic::AtomicI64);
impl super::clock::Clock for MovingClock {
    fn now_ms(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}
struct CrossMidnight<'a> {
    inner: Fake,
    clock: &'a MovingClock,
    change_at: Endpoint,
    to: i64,
}
impl Transport for CrossMidnight<'_> {
    fn request<'a>(
        &'a self,
        e: Endpoint,
        a: Option<&'a Account>,
        s: Option<&'a str>,
        n: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let result = self.inner.request(e, a, s, n).await;
            if e == self.change_at {
                self.clock
                    .0
                    .store(self.to, std::sync::atomic::Ordering::SeqCst);
            }
            result
        })
    }
}
#[tokio::test]
async fn crossing_cn_midnight_while_getting_baseline_prevents_submission() {
    // 2027-01-15 23:59:59 China standard time.
    let before = 1_800_028_799_000;
    let after = before + 2000;
    let (s, id) = setup();
    let clock = MovingClock(std::sync::atomic::AtomicI64::new(before));
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = CrossMidnight {
        inner: Fake::new(q),
        clock: &clock,
        change_at: Endpoint::Free,
        to: after,
    };
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &clock,
    };
    let row = e.claim(&id).await.unwrap();
    assert_eq!(row.claim_state, ClaimState::Unconfirmed);
    assert!(!f.inner.calls.lock().unwrap().contains(&Endpoint::Claim));
    assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
        .pending
        .is_none());
}
#[tokio::test]
async fn duplicate_claim_and_cancellation_leave_single_pending_attempt() {
    struct Blocking {
        started: tokio::sync::Notify,
        wait: tokio::sync::Notify,
    }
    impl Transport for Blocking {
        fn request<'a>(
            &'a self,
            e: Endpoint,
            _a: Option<&'a Account>,
            _s: Option<&'a str>,
            _n: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
            Box::pin(async move {
                match e {
                    Endpoint::Status => Ok(status(false).1.unwrap()),
                    Endpoint::Summary => Ok(resource(10.0)[0].1.clone().unwrap()),
                    Endpoint::Paid | Endpoint::Free => Ok(json!({"code":0,"data":{"Accounts":[]}})),
                    Endpoint::Claim => {
                        self.started.notify_one();
                        self.wait.notified().await;
                        Err(Failure::Unconfirmed)
                    }
                    _ => panic!("unexpected operation"),
                }
            })
        }
    }
    let (s, id) = setup();
    let transport = Blocking {
        started: tokio::sync::Notify::new(),
        wait: tokio::sync::Notify::new(),
    };
    let e = Engine {
        session: &s,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut operation = Box::pin(e.claim(&id));
    tokio::select! {_ = &mut operation=>panic!("claim should be in flight"),_ = transport.started.notified()=>{}}
    assert!(matches!(e.claim(&id).await, Err(Failure::Busy)));
    drop(operation);
    assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
        .pending
        .is_some());
}
#[tokio::test]
async fn status_returning_after_cn_midnight_cannot_mark_today_claimed() {
    for refresh in [false, true] {
        let before = 1_800_028_799_000;
        let (s, id) = setup();
        let clock = MovingClock(std::sync::atomic::AtomicI64::new(before));
        let mut q = if refresh { resource(10.0) } else { vec![] };
        q.push(status(true));
        let f = CrossMidnight {
            inner: Fake::new(q),
            clock: &clock,
            change_at: Endpoint::Status,
            to: before + 2000,
        };
        let e = Engine {
            session: &s,
            transport: &f,
            clock: &clock,
        };
        let row = if refresh {
            e.refresh(&id).await.unwrap()
        } else {
            e.claim(&id).await.unwrap()
        };
        assert_eq!(row.claim_state, ClaimState::Unconfirmed);
        assert!(row.can_claim);
        assert!(!f.inner.calls.lock().unwrap().contains(&Endpoint::Claim));
    }
}

// Unlike the single-account Fake, this script binds every response to the actual
// selected account, so a same-UID/domain mix-up cannot pass on endpoint order alone.
type AccountBoundResponse = (Domain, &'static str, &'static str, Endpoint, Value);

struct AccountBoundFake {
    queue: Mutex<VecDeque<AccountBoundResponse>>,
}
impl Transport for AccountBoundFake {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        account: Option<&'a Account>,
        state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            assert_ne!(endpoint, Endpoint::Claim, "refresh cannot claim rewards");
            assert!(state.is_none());
            let (domain, uid, token, expected, value) = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected account request");
            let selected = account.expect("refresh requires the selected account");
            assert_eq!(selected.domain, domain);
            assert_eq!(selected.uid, uid);
            assert_eq!(selected.token, token);
            assert_eq!(endpoint, expected);
            Ok(value)
        })
    }
}
fn account_isolation_fixture() -> (std::sync::Arc<SecretSession>, Vec<String>) {
    let session = SecretSession::ephemeral().unwrap();
    let binding = store::binding(&session).unwrap();
    let rows: Vec<_> = [
        (Domain::CodeBuddy, "same-uid", "SYNTHETIC_CODE_A"),
        (Domain::WorkBuddy, "same-uid", "SYNTHETIC_WORK_B"),
        (Domain::CodeBuddy, "other-uid", "SYNTHETIC_CODE_C"),
    ]
    .into_iter()
    .map(|(domain, uid, token)| {
        SavedAccount::new(Account {
            domain,
            uid: uid.into(),
            label: "Synthetic account".into(),
            token: token.into(),
        })
    })
    .collect();
    let ids = rows
        .iter()
        .map(|r| r.account.id(&binding.vault_id))
        .collect();
    store::save(&session, &binding, rows).unwrap();
    (session, ids)
}
fn account_isolation_responses(
    domain: Domain,
    uid: &'static str,
    token: &'static str,
    balance: f64,
) -> Vec<AccountBoundResponse> {
    let mut responses = resource(balance);
    responses.push(status(false));
    responses
        .into_iter()
        .map(|(endpoint, result)| (domain, uid, token, endpoint, result.unwrap()))
        .collect()
}
#[tokio::test]
async fn account_isolation_refresh_selects_domain_and_preserves_other_accounts() {
    let (session, ids) = account_isolation_fixture();
    assert_ne!(ids[0], ids[1]);
    let binding = store::binding(&session).unwrap();
    let before = store::load(&session, &binding).unwrap();
    let transport = AccountBoundFake {
        queue: Mutex::new(
            account_isolation_responses(Domain::WorkBuddy, "same-uid", "SYNTHETIC_WORK_B", 23.0)
                .into(),
        ),
    };
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let view = engine.refresh(&ids[1]).await.unwrap();
    assert_eq!(view.id, ids[1]);
    assert_eq!(view.credits.total_remaining, Some(23.0));
    assert_eq!(view.claim_state, ClaimState::Available);
    assert!(transport.queue.lock().unwrap().is_empty());
    let after = store::load(&session, &binding).unwrap();
    for index in [0, 2] {
        assert_eq!(
            serde_json::to_value(&after[index]).unwrap(),
            serde_json::to_value(&before[index]).unwrap()
        );
    }
    assert_eq!(after[1].account.token, before[1].account.token);
    let dto = serde_json::to_string(&view).unwrap();
    assert!(!dto.contains("SYNTHETIC_") && !dto.contains("same-uid"));
}
#[tokio::test]
async fn account_isolation_refresh_all_routes_each_account_without_merging_same_uid() {
    let (session, ids) = account_isolation_fixture();
    let mut responses =
        account_isolation_responses(Domain::CodeBuddy, "same-uid", "SYNTHETIC_CODE_A", 11.0);
    responses.extend(account_isolation_responses(
        Domain::WorkBuddy,
        "same-uid",
        "SYNTHETIC_WORK_B",
        22.0,
    ));
    responses.extend(account_isolation_responses(
        Domain::CodeBuddy,
        "other-uid",
        "SYNTHETIC_CODE_C",
        33.0,
    ));
    let transport = AccountBoundFake {
        queue: Mutex::new(responses.into()),
    };
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let views = engine.refresh_all().await.unwrap();
    assert_eq!(views.len(), 3);
    assert_eq!(views.iter().map(|v| v.id.clone()).collect::<Vec<_>>(), ids);
    assert_eq!(
        views
            .iter()
            .map(|v| v.credits.total_remaining)
            .collect::<Vec<_>>(),
        vec![Some(11.0), Some(22.0), Some(33.0)]
    );
    assert!(views.iter().all(|v| v.claim_state == ClaimState::Available));
    assert!(transport.queue.lock().unwrap().is_empty());
    let stored = store::load(&session, &store::binding(&session).unwrap()).unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(
        stored
            .iter()
            .map(|r| r.credits.total_remaining)
            .collect::<Vec<_>>(),
        vec![Some(11.0), Some(22.0), Some(33.0)]
    );
    let raw = std::fs::read(session.root().join(store::FILE)).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains("SYNTHETIC_"));
}
#[tokio::test]
async fn account_isolation_foreign_vault_id_is_rejected_before_any_request_or_write() {
    let (_foreign_session, foreign_ids) = account_isolation_fixture();
    let (session, ids) = account_isolation_fixture();
    assert_ne!(foreign_ids[1], ids[1]);
    let path = session.root().join(store::FILE);
    let before = std::fs::read(&path).unwrap();
    let transport = AccountBoundFake {
        queue: Mutex::new(VecDeque::new()),
    };
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    assert_eq!(
        engine.refresh(&foreign_ids[1]).await.unwrap_err(),
        Failure::UnsupportedContext
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

type BatchResponse = (
    Domain,
    &'static str,
    &'static str,
    Endpoint,
    Result<Value, Failure>,
);
struct BatchFake {
    queue: Mutex<VecDeque<BatchResponse>>,
    calls: Mutex<Vec<(Domain, String, Endpoint)>>,
    block_claim: std::sync::atomic::AtomicBool,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl BatchFake {
    fn new(queue: Vec<BatchResponse>, block: bool) -> Self {
        Self {
            queue: Mutex::new(queue.into()),
            calls: Mutex::new(vec![]),
            block_claim: std::sync::atomic::AtomicBool::new(block),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }
}
impl Transport for BatchFake {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        account: Option<&'a Account>,
        _state: Option<&'a str>,
        _now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            let selected = account.expect("claim requires an account");
            let (domain, uid, token, expected, result) = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected batch request");
            assert_eq!(selected.domain, domain);
            assert_eq!(selected.uid, uid);
            assert_eq!(selected.token, token);
            assert_eq!(endpoint, expected);
            self.calls
                .lock()
                .unwrap()
                .push((domain, uid.into(), endpoint));
            if endpoint == Endpoint::Claim
                && self
                    .block_claim
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.started.notify_one();
                self.release.notified().await;
            }
            result
        })
    }
}
fn batch_claim_responses(
    domain: Domain,
    uid: &'static str,
    token: &'static str,
    balance: f64,
) -> Vec<BatchResponse> {
    let mut queue = vec![status(false)];
    queue.extend(resource(balance));
    queue.push((Endpoint::Claim, Ok(json!({"code":0,"credit":20}))));
    queue.push(status(true));
    queue.extend(resource(balance + 20.0));
    queue
        .into_iter()
        .map(|(e, result)| (domain, uid, token, e, result))
        .collect()
}
fn batch_a() -> Vec<BatchResponse> {
    batch_claim_responses(Domain::CodeBuddy, "same-uid", "SYNTHETIC_CODE_A", 10.0)
}
fn batch_b() -> Vec<BatchResponse> {
    batch_claim_responses(Domain::WorkBuddy, "same-uid", "SYNTHETIC_WORK_B", 20.0)
}
fn batch_c() -> Vec<BatchResponse> {
    batch_claim_responses(Domain::CodeBuddy, "other-uid", "SYNTHETIC_CODE_C", 30.0)
}

#[tokio::test]
async fn batch_claims_only_backend_eligible_accounts_once_and_keeps_credentials_isolated() {
    for skipped in [
        ClaimState::Claimed,
        ClaimState::AlreadyClaimed,
        ClaimState::Unavailable,
        ClaimState::NeedsVerification,
    ] {
        let (session, ids) = account_isolation_fixture();
        let binding = store::binding(&session).unwrap();
        let mut rows = store::load(&session, &binding).unwrap();
        rows[1].claim_day = today(NOW);
        rows[1].claim_state = skipped;
        store::save(&session, &binding, rows).unwrap();
        let mut queue = batch_a();
        queue.extend(batch_c());
        let transport = BatchFake::new(queue, false);
        let engine = Engine {
            session: &session,
            transport: &transport,
            clock: &super::clock::FixedClock(NOW),
        };
        let results = engine.claim_all().await.unwrap();
        assert_eq!(
            results
                .iter()
                .map(|r| r.account.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[2].clone()]
        );
        assert!(results.iter().all(|r| r.error.is_none()
            && r.account.claim_state == ClaimState::Claimed
            && r.account.credited == Some(20.0)));
        assert!(engine.claim_all().await.unwrap().is_empty());
        assert!(transport.queue.lock().unwrap().is_empty());
        let dto = serde_json::to_string(&results).unwrap();
        assert!(!dto.contains("SYNTHETIC_") && !dto.contains("same-uid"));
    }
}

#[tokio::test]
async fn batch_verification_failure_does_not_submit_or_stop_other_accounts() {
    let (session, ids) = account_isolation_fixture();
    let mut queue = vec![(
        Domain::CodeBuddy,
        "same-uid",
        "SYNTHETIC_CODE_A",
        Endpoint::Status,
        Err(Failure::NeedsVerification),
    )];
    queue.extend(batch_b());
    queue.push((
        Domain::CodeBuddy,
        "other-uid",
        "SYNTHETIC_CODE_C",
        Endpoint::Status,
        Ok(json!({"code":1002})),
    ));
    let transport = BatchFake::new(queue, false);
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let results = engine.claim_all().await.unwrap();
    assert_eq!(
        results
            .iter()
            .map(|r| r.account.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        results[0].account.claim_state,
        ClaimState::NeedsVerification
    );
    assert_eq!(results[1].account.claim_state, ClaimState::Claimed);
    assert_eq!(results[2].account.claim_state, ClaimState::Unavailable);
    assert_eq!(
        transport
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.2 == Endpoint::Claim)
            .count(),
        1
    );
    assert!(engine.claim_all().await.unwrap().is_empty());
    assert!(transport.queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn batch_cancellation_keeps_pending_and_next_explicit_batch_reconciles_before_retry() {
    let (session, ids) = account_isolation_fixture();
    let mut initial = batch_a();
    initial.truncate(5);
    let blocked = BatchFake::new(initial, true);
    let engine = Engine {
        session: &session,
        transport: &blocked,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut operation = Box::pin(engine.claim_all());
    tokio::select! { _ = &mut operation => panic!("batch must be in flight"), _ = blocked.started.notified() => {} }
    assert!(matches!(engine.claim_all().await, Err(Failure::Busy)));
    assert!(matches!(engine.claim(&ids[0]).await, Err(Failure::Busy)));
    drop(operation);
    let binding = store::binding(&session).unwrap();
    assert!(store::load(&session, &binding).unwrap()[0]
        .pending
        .is_some());
    let mut recovery = vec![status(false)];
    recovery.extend(resource(10.0));
    let mut queue: Vec<_> = recovery
        .into_iter()
        .map(|(e, r)| (Domain::CodeBuddy, "same-uid", "SYNTHETIC_CODE_A", e, r))
        .collect();
    queue.extend(batch_b());
    queue.extend(batch_c());
    queue.extend(batch_a());
    let transport = BatchFake::new(queue, false);
    let restarted = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let results = restarted.claim_all().await.unwrap();
    assert_eq!(results[0].account.claim_state, ClaimState::Available);
    assert!(results[0].error.is_none());
    assert!(!transport
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.0 == Domain::CodeBuddy && c.1 == "same-uid" && c.2 == Endpoint::Claim));
    let retried = restarted.claim_all().await.unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].account.id, ids[0]);
    assert_eq!(retried[0].account.claim_state, ClaimState::Claimed);
    assert!(transport.queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn batch_busy_account_is_reported_while_other_accounts_finish_and_next_batch_recovers() {
    let (session, ids) = account_isolation_fixture();
    let mut queue = batch_a();
    queue.extend(batch_c());
    queue.extend(batch_b());
    let transport = BatchFake::new(queue, true);
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut batch = Box::pin(engine.claim_all());
    tokio::select! { _ = &mut batch => panic!("batch must be in flight"), _ = transport.started.notified() => {} }
    let mut manual = Box::pin(engine.claim(&ids[1]));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(manual.as_mut().poll(&mut context).is_pending());
    transport.release.notify_one();
    assert!(batch.as_mut().poll(&mut context).is_pending());
    drop(manual);
    let results = batch.await.unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[1].error, Some(Failure::Busy));
    assert_eq!(results[1].account.id, ids[1]);
    assert_eq!(results[0].account.claim_state, ClaimState::Claimed);
    assert_eq!(results[2].account.claim_state, ClaimState::Claimed);
    let retry = engine.claim_all().await.unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].account.id, ids[1]);
    assert_eq!(retry[0].account.claim_state, ClaimState::Claimed);
    assert!(transport.queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn batch_crossing_cn_midnight_stops_before_claiming_remaining_accounts() {
    let (session, _) = account_isolation_fixture();
    let before = 1_800_028_799_000;
    let clock = MovingClock(std::sync::atomic::AtomicI64::new(before));
    let transport = CrossMidnight {
        inner: Fake::new(vec![status(false)]),
        clock: &clock,
        change_at: Endpoint::Status,
        to: before + 2000,
    };
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &clock,
    };
    let results = engine.claim_all().await.unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].account.claim_state, ClaimState::Unconfirmed);
    assert!(results[1..]
        .iter()
        .all(|r| r.error == Some(Failure::Unconfirmed)));
    assert!(!transport
        .inner
        .calls
        .lock()
        .unwrap()
        .contains(&Endpoint::Claim));
}

#[tokio::test]
async fn batch_empty_vault_has_no_network_effects() {
    let session = SecretSession::ephemeral().unwrap();
    let transport = Fake::new(vec![]);
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    assert!(engine.claim_all().await.unwrap().is_empty());
    assert!(transport.calls.lock().unwrap().is_empty());
}
