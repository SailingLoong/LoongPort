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
async fn pending_unknown_claim_survives_verification_and_reauthorization_without_replay() {
    let (s, id) = setup();
    let b = store::binding(&s).unwrap();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    q.push((Endpoint::Claim, Err(Failure::Unconfirmed)));
    q.push((Endpoint::Status, Ok(json!({"code":401}))));
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    let row = e.claim(&id).await.unwrap();
    assert_eq!(row.claim_state, ClaimState::NeedsVerification);
    assert!(!row.can_claim);
    assert_eq!(row.credited, None);
    let pending = serde_json::to_value(&store::load(&s, &b).unwrap()[0].pending).unwrap();
    assert_eq!(pending["receipt"], "unconfirmed");
    assert_eq!(pending["before"], 10.0);
    assert!(pending["expected"].is_null());

    // Recreate the operation owner and cover both explicit recovery entry points.
    for (code, refresh) in [(403, true), (10085, false)] {
        let mut q = if refresh { resource(10.0) } else { vec![] };
        q.push((Endpoint::Status, Ok(json!({"code":code}))));
        if !refresh {
            q.extend(resource(10.0));
        }
        let recovery = Fake::new(q);
        let e = Engine {
            session: &s,
            transport: &recovery,
            clock: &super::clock::FixedClock(NOW),
        };
        let row = if refresh {
            e.refresh(&id).await.unwrap()
        } else {
            e.claim(&id).await.unwrap()
        };
        assert_eq!(row.claim_state, ClaimState::NeedsVerification);
        assert!(!row.can_claim);
        assert_eq!(row.credited, None);
        assert_eq!(
            serde_json::to_value(&store::load(&s, &b).unwrap()[0].pending).unwrap(),
            pending
        );
        assert!(!recovery.calls.lock().unwrap().contains(&Endpoint::Claim));
        assert!(recovery.queue.lock().unwrap().is_empty());
    }

    let authorization = Fake::new(vec![
        (
            Endpoint::AuthState,
            Ok(
                json!({"code":0,"data":{"state":"synthetic-recovery","authUrl":"https://www.codebuddy.cn/login?state=synthetic-recovery"}}),
            ),
        ),
        (
            Endpoint::AuthToken,
            Ok(
                json!({"code":0,"data":{"accessToken":"SYNTHETIC_REAUTHORIZED_TOKEN","domain":"www.codebuddy.cn"}}),
            ),
        ),
        (
            Endpoint::Profile,
            Ok(
                json!({"code":0,"data":{"uid":"synthetic-uid","nickname":"Synthetic reauthorized account","domain":"www.codebuddy.cn"}}),
            ),
        ),
    ]);
    let login = super::authorization::begin(&s, &authorization, NOW)
        .await
        .unwrap();
    assert_eq!(
        super::authorization::finish(&s, &authorization, &login.flow_id, NOW + 1)
            .await
            .unwrap()
            .state,
        super::authorization::AuthorizationState::Saved,
    );
    let rows = store::load(&s, &b).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].account.id(&b.vault_id), id);
    assert_eq!(rows[0].account.token, "SYNTHETIC_REAUTHORIZED_TOKEN");
    assert_eq!(serde_json::to_value(&rows[0].pending).unwrap(), pending);
    assert!(!authorization
        .calls
        .lock()
        .unwrap()
        .contains(&Endpoint::Claim));

    for _ in 0..2 {
        let mut q = vec![status(false)];
        q.extend(resource(10.0));
        let reconciled = Fake::new(q);
        let e = Engine {
            session: &s,
            transport: &reconciled,
            clock: &super::clock::FixedClock(NOW),
        };
        assert_eq!(
            e.claim(&id).await.unwrap().claim_state,
            ClaimState::Unconfirmed
        );
        assert!(store::load(&s, &b).unwrap()[0].pending.is_some());
        assert!(!reconciled.calls.lock().unwrap().contains(&Endpoint::Claim));
    }
}

#[tokio::test]
async fn pending_known_receipt_survives_verification_until_authoritative_readback() {
    for (receipt, expected, credited) in [
        (
            json!({"code":0,"credit":20}),
            ClaimState::Claimed,
            Some(20.0),
        ),
        (json!({"code":1001}), ClaimState::AlreadyClaimed, None),
    ] {
        let (s, id) = setup();
        let b = store::binding(&s).unwrap();
        let mut q = vec![status(false)];
        q.extend(resource(10.0));
        q.push((Endpoint::Claim, Ok(receipt)));
        q.push((Endpoint::Status, Ok(json!({"code":401}))));
        q.extend(resource(30.0));
        let f = Fake::new(q);
        let e = Engine {
            session: &s,
            transport: &f,
            clock: &super::clock::FixedClock(NOW),
        };
        let row = e.claim(&id).await.unwrap();
        assert_eq!(row.claim_state, ClaimState::NeedsVerification);
        assert!(!row.can_claim);
        assert_eq!(row.credited, None);
        let pending = store::load(&s, &b).unwrap()[0].pending.clone().unwrap();
        assert_eq!(pending.receipt, expected);
        assert_eq!(pending.before, Some(10.0));
        assert_eq!(pending.expected, credited);

        let mut q = resource(30.0);
        q.push(status(true));
        let recovery = Fake::new(q);
        let e = Engine {
            session: &s,
            transport: &recovery,
            clock: &super::clock::FixedClock(NOW),
        };
        let row = e.refresh(&id).await.unwrap();
        assert_eq!(row.claim_state, expected);
        assert_eq!(row.credited, credited);
        assert!(!row.can_claim);
        assert!(store::load(&s, &b).unwrap()[0].pending.is_none());
        assert!(!recovery.calls.lock().unwrap().contains(&Endpoint::Claim));
        assert!(recovery.queue.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn terminal_claim_receipts_keep_their_existing_resolution_on_verification() {
    for (code, expected) in [
        (401, ClaimState::NeedsVerification),
        (1002, ClaimState::Unavailable),
    ] {
        let (s, id) = setup();
        let mut q = vec![status(false)];
        q.extend(resource(10.0));
        q.push((Endpoint::Claim, Ok(json!({"code":code}))));
        q.push((Endpoint::Status, Ok(json!({"code":401}))));
        q.extend(resource(10.0));
        let f = Fake::new(q);
        let e = Engine {
            session: &s,
            transport: &f,
            clock: &super::clock::FixedClock(NOW),
        };
        let row = e.claim(&id).await.unwrap();
        assert_eq!(row.claim_state, expected);
        assert!(!row.can_claim);
        assert_eq!(row.credited, None);
        assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
            .pending
            .is_none());
        assert!(f.queue.lock().unwrap().is_empty());
        assert_eq!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|endpoint| **endpoint == Endpoint::Claim)
                .count(),
            1
        );
    }
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
async fn pending_unclaimed_remains_blocked_without_an_authoritative_receipt() {
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
        ClaimState::Unconfirmed
    );
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
    assert!(store::load(&s, &b).unwrap()[0].pending.is_some());
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
async fn batch_cancellation_keeps_pending_and_later_batches_skip_unresolved_account() {
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
    let mut queue = batch_b();
    queue.extend(batch_c());
    let transport = BatchFake::new(queue, false);
    let restarted = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let results = restarted.claim_all().await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].account.id, ids[1]);
    assert_eq!(results[1].account.id, ids[2]);
    assert!(results
        .iter()
        .all(|r| r.error.is_none() && r.account.claim_state == ClaimState::Claimed));
    assert!(!transport
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.0 == Domain::CodeBuddy && c.1 == "same-uid" && c.2 == Endpoint::Claim));
    assert!(restarted.claim_all().await.unwrap().is_empty());
    assert!(store::load(&session, &binding).unwrap()[0]
        .pending
        .is_some());
    assert!(transport.queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn batch_busy_account_is_reported_while_other_accounts_finish_and_next_batch_recovers() {
    struct WaitingStatus(tokio::sync::Notify);
    impl Transport for WaitingStatus {
        fn request<'a>(
            &'a self,
            e: Endpoint,
            _: Option<&'a Account>,
            _: Option<&'a str>,
            _: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
            Box::pin(async move {
                assert_eq!(e, Endpoint::Status);
                self.0.notify_one();
                std::future::pending().await
            })
        }
    }
    let (session, ids) = account_isolation_fixture();
    let manual_transport = WaitingStatus(tokio::sync::Notify::new());
    let manual_engine = Engine {
        session: &session,
        transport: &manual_transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut manual = Box::pin(manual_engine.claim(&ids[1]));
    tokio::select! { _ = &mut manual => panic!("manual must be in flight"), _ = manual_transport.0.notified() => {} }
    let mut queue = batch_a();
    queue.extend(batch_c());
    queue.extend(batch_b());
    let transport = BatchFake::new(queue, false);
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    let results = engine.claim_all().await.unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[1].error, Some(Failure::Busy));
    assert_eq!(results[1].account.id, ids[1]);
    assert_eq!(results[0].account.claim_state, ClaimState::Claimed);
    assert_eq!(results[2].account.claim_state, ClaimState::Claimed);
    drop(manual);
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

#[tokio::test]
async fn unresolved_attempt_never_reopens_on_read_or_next_day() {
    for offset in [0, 86400000] {
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
        for _ in 0..2 {
            let mut q = vec![status(false)];
            q.extend(resource(10.0));
            let f = Fake::new(q);
            let e = Engine {
                session: &s,
                transport: &f,
                clock: &super::clock::FixedClock(NOW + offset),
            };
            let row = e.claim(&id).await.unwrap();
            assert!(
                store::load(&s, &b).unwrap()[0].pending.is_some(),
                "unknown POST must survive day and repeated reconciliation"
            );
            assert!(!row.can_claim);
            assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
        }
    }
}

// Root-scoped offline dispatch rendezvous: no live network or global test ordering.
struct DispatchGate {
    reached: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
fn gates(
) -> &'static Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Arc<DispatchGate>>> {
    static GATES: std::sync::OnceLock<
        Mutex<std::collections::HashMap<std::path::PathBuf, std::sync::Arc<DispatchGate>>>,
    > = std::sync::OnceLock::new();
    GATES.get_or_init(Default::default)
}
pub(super) async fn before_claim_dispatch(session: &SecretSession) {
    let gate = gates().lock().unwrap().remove(session.root());
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.resume.notified().await;
    }
}
fn gate(session: &SecretSession) -> std::sync::Arc<DispatchGate> {
    let gate = std::sync::Arc::new(DispatchGate {
        reached: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    gates()
        .lock()
        .unwrap()
        .insert(session.root().into(), gate.clone());
    gate
}
async fn enable(e: &Engine<'_, impl Transport>, id: &str) -> AccountView {
    let row = e
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap();
    e.set_automatic(
        id,
        row.generation,
        row.auto_claim_revision,
        row.auto_claim_policy_version,
        true,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn automatic_defaults_off_optin_bound_and_stale_setter_rejected() {
    let (s, id) = setup();
    let f = Fake::new(vec![]);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    e.automatic_tick().await.unwrap();
    let before = e.list().await.unwrap().remove(0);
    assert!(!before.auto_claim_enabled);
    let after = enable(&e, &id).await;
    assert!(after.auto_claim_enabled);
    assert_eq!(after.auto_claim_revision, before.auto_claim_revision + 1);
    assert!(e
        .set_automatic(
            &id,
            before.generation,
            before.auto_claim_revision,
            before.auto_claim_policy_version,
            false
        )
        .await
        .is_err());
    assert!(e
        .set_automatic(&id, after.generation, after.auto_claim_revision, 999, true)
        .await
        .is_err());
    let b = store::binding(&s).unwrap();
    let mut rows = store::load(&s, &b).unwrap();
    rows[0].account.token = "SYNTHETIC_REPLACEMENT".into();
    store::save(&s, &b, rows).unwrap();
    e.automatic_tick().await.unwrap();
    assert!(!e.list().await.unwrap()[0].auto_claim_enabled);
    assert!(f.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn opted_in_account_claims_once_and_never_infers_award() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    q.push((Endpoint::Claim, Ok(json!({"code":0,"credit":7}))));
    q.push(status(true));
    q.extend(resource(17.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&e, &id).await;
    e.automatic_tick().await.unwrap();
    e.automatic_tick().await.unwrap();
    let row = e.list().await.unwrap().remove(0);
    assert_eq!(row.credited, Some(7.0));
    assert!(row.auto_claim_enabled);
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
}
#[tokio::test]
async fn disable_after_persist_before_dispatch_clears_only_proven_unsent_intent() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    let on = enable(&e, &id).await;
    let gate = gate(&s);
    let mut tick = Box::pin(e.automatic_tick());
    tokio::select! { _ = &mut tick => panic!("must pause after persistence"), _ = gate.reached.notified() => {} }
    assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
        .pending
        .is_some());
    let off = e
        .set_automatic(
            &id,
            on.generation,
            on.auto_claim_revision,
            on.auto_claim_policy_version,
            false,
        )
        .await
        .unwrap();
    assert!(!off.auto_claim_enabled);
    gate.resume.notify_one();
    tick.await.unwrap();
    assert!(store::load(&s, &store::binding(&s).unwrap()).unwrap()[0]
        .pending
        .is_none());
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
}
#[tokio::test]
async fn lock_unblock_before_dispatch_invalidates_old_lease_without_write() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&e, &id).await;
    let gate = gate(&s);
    let mut tick = Box::pin(e.automatic_tick());
    tokio::select! { _ = &mut tick => panic!("must pause"), _ = gate.reached.notified() => {} }
    s.set_blocked(true);
    s.set_blocked(false);
    gate.resume.notify_one();
    tick.await.unwrap();
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
    s.set_blocked(true);
    assert!(e.automatic_tick().await.is_err());
    s.set_blocked(false);
}
#[tokio::test]
async fn pending_survives_optout_reenable_and_future_automatic_ticks_without_reads() {
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
    let f = Fake::new(vec![]);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW + 86400000),
    };
    let on = enable(&e, &id).await;
    e.set_automatic(
        &id,
        on.generation,
        on.auto_claim_revision,
        on.auto_claim_policy_version,
        false,
    )
    .await
    .unwrap();
    enable(&e, &id).await;
    e.automatic_tick().await.unwrap();
    assert!(e.list().await.unwrap()[0].auto_claim_blocked);
    assert!(store::load(&s, &b).unwrap()[0].pending.is_some());
    assert!(f.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn reauthorization_revokes_automatic_consent_even_for_same_token() {
    let (s, id) = setup();
    let f = Fake::new(vec![]);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    let on = enable(&e, &id).await;
    let authorization = Fake::new(vec![
        (
            Endpoint::AuthState,
            Ok(
                json!({"code":0,"data":{"state":"auto-relogin","authUrl":"https://www.codebuddy.cn/login?state=auto-relogin"}}),
            ),
        ),
        (
            Endpoint::AuthToken,
            Ok(
                json!({"code":0,"data":{"accessToken":"SYNTHETIC_TOKEN_CANARY","domain":"www.codebuddy.cn"}}),
            ),
        ),
        (
            Endpoint::Profile,
            Ok(json!({"code":0,"data":{"uid":"synthetic-uid","domain":"www.codebuddy.cn"}})),
        ),
    ]);
    let login = super::authorization::begin(&s, &authorization, NOW)
        .await
        .unwrap();
    super::authorization::finish(&s, &authorization, &login.flow_id, NOW + 1)
        .await
        .unwrap();
    let after = e.list().await.unwrap().remove(0);
    assert!(!after.auto_claim_enabled);
    assert!(after.generation > on.generation);
    assert!(e
        .set_automatic(
            &id,
            on.generation,
            on.auto_claim_revision,
            on.auto_claim_policy_version,
            true
        )
        .await
        .is_err());
    e.automatic_tick().await.unwrap();
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn auto_claim_only_uses_the_opted_in_account_identity() {
    let (session, ids) = account_isolation_fixture();
    let transport = BatchFake::new(batch_b(), false);
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&engine, &ids[1]).await;
    engine.automatic_tick().await.unwrap();
    assert!(transport.queue.lock().unwrap().is_empty());
    assert!(transport
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.0 == Domain::WorkBuddy && c.1 == "same-uid"));
    let rows = engine.list().await.unwrap();
    assert!(!rows[0].auto_claim_enabled);
    assert!(!rows[2].auto_claim_enabled);
}
#[tokio::test]
async fn uncertain_claim_failure_and_optout_after_dispatch_never_replay() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    q.push((Endpoint::Claim, Err(Failure::NeedsVerification)));
    q.push(status(false));
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&e, &id).await;
    e.automatic_tick().await.unwrap();
    let row = e.list().await.unwrap().remove(0);
    assert!(row.auto_claim_blocked);
    assert_eq!(row.credited, None);
    e.set_automatic(
        &id,
        row.generation,
        row.auto_claim_revision,
        row.auto_claim_policy_version,
        false,
    )
    .await
    .unwrap();
    enable(&e, &id).await;
    e.automatic_tick().await.unwrap();
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == Endpoint::Claim)
            .count(),
        1
    );
}
#[tokio::test]
async fn transport_proven_unsent_failure_can_clear_prepared_intent() {
    struct Unsent(Fake);
    impl Transport for Unsent {
        fn request<'a>(
            &'a self,
            e: Endpoint,
            a: Option<&'a Account>,
            s: Option<&'a str>,
            n: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
            self.0.request(e, a, s, n)
        }
        fn claim<'a>(
            &'a self,
            _: &'a Account,
            _: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, DispatchFailure>> + Send + 'a>> {
            Box::pin(async { Err(DispatchFailure::NotDispatched(Failure::UnsupportedContext)) })
        }
    }
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = Unsent(Fake::new(q));
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&e, &id).await;
    e.automatic_tick().await.unwrap();
    let rows = store::load(&s, &store::binding(&s).unwrap()).unwrap();
    assert!(rows[0].pending.is_none());
    assert!(rows[0].attempted_day.is_none());
    assert!(!f.0.calls.lock().unwrap().contains(&Endpoint::Claim));
}
#[test]
fn legacy_account_is_default_off_and_policy_wire_is_single_owner() {
    let (s, _) = setup();
    let b = store::binding(&s).unwrap();
    let rows = store::load(&s, &b).unwrap();
    let mut legacy = serde_json::to_value(&rows[0]).unwrap();
    for key in [
        "generation",
        "auto_claim_enabled",
        "auto_claim_revision",
        "auto_claim_policy_version",
        "auto_claim_credential",
        "attempted_day",
    ] {
        legacy.as_object_mut().unwrap().remove(key);
    }
    let legacy: SavedAccount = serde_json::from_value(legacy).unwrap();
    assert!(!legacy.automatic_enabled());
    let commands = include_str!("../commands/workbuddy.rs");
    let registrations = include_str!("../lib.rs");
    let frontend = include_str!("../../../src/lib/api/workbuddy.ts");
    for source in [commands, registrations, frontend] {
        assert!(source.contains("set_workbuddy_automatic_claim"));
    }
    let maintenance = include_str!("../maintenance/mod.rs");
    assert!(maintenance.contains("start_workbuddy_automatic_claim(app.clone())"));
    assert!(maintenance.contains(".automatic_tick()"));
}

#[tokio::test]
async fn continuously_locked_pre_dispatch_keeps_intent_fail_closed_after_unlock() {
    let (s, id) = setup();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&e, &id).await;
    let gate = gate(&s);
    let mut tick = Box::pin(e.automatic_tick());
    tokio::select! { _ = &mut tick => panic!("must pause"), _ = gate.reached.notified() => {} }
    s.set_blocked(true);
    gate.resume.notify_one();
    tick.await.unwrap();
    s.set_blocked(false);
    e.automatic_tick().await.unwrap();
    assert!(e.list().await.unwrap()[0].auto_claim_blocked);
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
}
#[test]
fn automatic_future_is_send_for_existing_scheduler() {
    fn is_send<T: Send>(_: T) {}
    let (s, _) = setup();
    let f = Fake::new(vec![]);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    is_send(e.automatic_tick());
}

#[tokio::test]
async fn late_claim_receipt_cannot_overwrite_reauthorized_account_or_revive_consent() {
    struct InFlight {
        inner: Fake,
        reached: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }
    impl Transport for InFlight {
        fn request<'a>(
            &'a self,
            e: Endpoint,
            a: Option<&'a Account>,
            s: Option<&'a str>,
            n: i64,
        ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
            Box::pin(async move {
                if e == Endpoint::Claim {
                    self.reached.notify_one();
                    self.resume.notified().await;
                    Ok(json!({"code":0,"credit":7}))
                } else {
                    self.inner.request(e, a, s, n).await
                }
            })
        }
    }
    let (s, id) = setup();
    let b = store::binding(&s).unwrap();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = InFlight {
        inner: Fake::new(q),
        reached: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    };
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    let before = enable(&e, &id).await;
    let mut tick = Box::pin(e.automatic_tick());
    tokio::select! { _ = &mut tick => panic!("claim must be in flight"), _ = f.reached.notified() => {} }
    let pending = store::load(&s, &b).unwrap()[0].pending.clone();
    assert!(pending.is_some());
    let auth = Fake::new(vec![
        (
            Endpoint::AuthState,
            Ok(
                json!({"code":0,"data":{"state":"inflight-relogin","authUrl":"https://www.codebuddy.cn/login?state=inflight-relogin"}}),
            ),
        ),
        (
            Endpoint::AuthToken,
            Ok(
                json!({"code":0,"data":{"accessToken":"SYNTHETIC_NEW_TOKEN","domain":"www.codebuddy.cn"}}),
            ),
        ),
        (
            Endpoint::Profile,
            Ok(json!({"code":0,"data":{"uid":"synthetic-uid","domain":"www.codebuddy.cn"}})),
        ),
    ]);
    let login = super::authorization::begin(&s, &auth, NOW).await.unwrap();
    super::authorization::finish(&s, &auth, &login.flow_id, NOW + 1)
        .await
        .unwrap();
    f.resume.notify_one();
    tick.await.unwrap();
    let saved = store::load(&s, &b).unwrap().remove(0);
    assert!(saved.generation > before.generation);
    assert!(!saved.automatic_enabled());
    assert_eq!(saved.account.token, "SYNTHETIC_NEW_TOKEN");
    assert!(saved.pending == pending);
    assert_eq!(saved.credited, None);
    enable(&e, &id).await;
    e.automatic_tick().await.unwrap();
    assert!(f.inner.queue.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unsent_rollback_does_not_clear_a_replacement_pending_attempt() {
    let (s, id) = setup();
    let b = store::binding(&s).unwrap();
    let mut q = vec![status(false)];
    q.extend(resource(10.0));
    let f = Fake::new(q);
    let e = Engine {
        session: &s,
        transport: &f,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&e, &id).await;
    let gate = gate(&s);
    let mut tick = Box::pin(e.automatic_tick());
    tokio::select! { _ = &mut tick => panic!("must pause after persistence"), _ = gate.reached.notified() => {} }
    let mut rows = store::load(&s, &b).unwrap();
    rows[0].pending.as_mut().unwrap().before = Some(22.0);
    let replacement = rows[0].pending.clone();
    store::save(&s, &b, rows).unwrap();
    gate.resume.notify_one();
    tick.await.unwrap();
    assert!(store::load(&s, &b).unwrap()[0].pending == replacement);
    assert!(!f.calls.lock().unwrap().contains(&Endpoint::Claim));
}

// The gate lives inside the real transport future: reaching it proves refresh
// has entered HTTP, rather than merely being queued for the catalog mutex.
struct RefreshReadGate {
    inner: Fake,
    endpoint: Endpoint,
    reached: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
impl RefreshReadGate {
    fn new(endpoint: Endpoint, balance: f64, checked: bool) -> Self {
        let mut requests = resource(balance);
        requests.push(status(checked));
        Self {
            inner: Fake::new(requests),
            endpoint,
            reached: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        }
    }
}
impl Transport for RefreshReadGate {
    fn request<'a>(
        &'a self,
        endpoint: Endpoint,
        account: Option<&'a Account>,
        state: Option<&'a str>,
        now: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Value, Failure>> + Send + 'a>> {
        Box::pin(async move {
            if endpoint == self.endpoint {
                self.reached.notify_one();
                self.resume.notified().await;
            }
            self.inner.request(endpoint, account, state, now).await
        })
    }
}
async fn refresh_progress<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(std::time::Duration::from_secs(5), future)
        .await
        .expect("unrelated local operation must finish while refresh HTTP is suspended")
}

#[tokio::test]
async fn refresh_pending_http_allows_other_accounts_and_minimax_owner_to_progress() {
    let _minimax = crate::minimax_code::isolated_fixture_guard().await;
    let (s, ids) = account_isolation_fixture();
    let gate = RefreshReadGate::new(Endpoint::Summary, 10.0, false);
    let engine = Engine {
        session: &s,
        transport: &gate,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut refresh = Box::pin(engine.refresh(&ids[0]));
    tokio::select! { _ = &mut refresh => panic!("Summary must be suspended"), _ = gate.reached.notified() => {} }
    assert!(matches!(engine.refresh(&ids[0]).await, Err(Failure::Busy)));
    assert!(matches!(engine.claim(&ids[0]).await, Err(Failure::Busy)));
    assert_eq!(refresh_progress(engine.list()).await.unwrap().len(), 3);
    let minimax = crate::minimax_code::checkin::Engine {
        session: &s,
        transport: &crate::minimax_code::http::OfficialHttp,
    };
    // Real MiniMax catalog consumer; list has no HTTP side effects.
    assert!(refresh_progress(minimax.list()).await.unwrap().is_empty());
    let mut requests = resource(77.0);
    requests.push(status(true));
    let other_transport = Fake::new(requests);
    let other = Engine {
        session: &s,
        transport: &other_transport,
        clock: &super::clock::FixedClock(NOW),
    };
    assert_eq!(
        refresh_progress(other.refresh(&ids[1]))
            .await
            .unwrap()
            .credits
            .total_remaining,
        Some(77.0)
    );
    gate.resume.notify_one();
    assert_eq!(refresh.await.unwrap().credits.total_remaining, Some(10.0));
    let rows = engine.list().await.unwrap();
    assert_eq!(
        rows.iter()
            .find(|r| r.id == ids[1])
            .unwrap()
            .credits
            .total_remaining,
        Some(77.0)
    );
    assert_eq!(
        rows.iter().find(|r| r.id == ids[1]).unwrap().claim_state,
        ClaimState::AlreadyClaimed
    );
}

#[tokio::test]
async fn refresh_pending_http_preserves_concurrent_policy_and_current_account_fields() {
    let (s, id) = setup();
    let gate = RefreshReadGate::new(Endpoint::Summary, 10.0, false);
    let engine = Engine {
        session: &s,
        transport: &gate,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut refresh = Box::pin(engine.refresh(&id));
    tokio::select! { _ = &mut refresh => panic!("Summary must be suspended"), _ = gate.reached.notified() => {} }
    let on = refresh_progress(enable(&engine, &id)).await;
    let off = refresh_progress(engine.set_automatic(
        &id,
        on.generation,
        on.auto_claim_revision,
        on.auto_claim_policy_version,
        false,
    ))
    .await
    .unwrap();
    {
        let _owner = refresh_progress(crate::services::sync_protocol::sync_mutex().lock()).await;
        let b = store::binding(&s).unwrap();
        let mut rows = store::load(&s, &b).unwrap();
        rows[0].account.label = "Updated while reading".into();
        store::save(&s, &b, rows).unwrap();
    }
    gate.resume.notify_one();
    let refreshed = refresh.await.unwrap();
    assert!(!refreshed.auto_claim_enabled);
    assert_eq!(refreshed.auto_claim_revision, off.auto_claim_revision);
    assert_eq!(refreshed.label, "Updated while reading");
    assert_eq!(refreshed.credits.total_remaining, Some(10.0));
}

#[tokio::test]
async fn refresh_late_result_rejects_relogin_generation_token_identity_deletion_and_epoch() {
    for endpoint in [Endpoint::Summary, Endpoint::Status] {
        for change in [
            "relogin",
            "generation",
            "token",
            "identity",
            "delete",
            "epoch",
        ] {
            let (s, id) = setup();
            let gate = RefreshReadGate::new(endpoint, 99.0, true);
            let engine = Engine {
                session: &s,
                transport: &gate,
                clock: &super::clock::FixedClock(NOW),
            };
            let mut refresh = Box::pin(engine.refresh(&id));
            tokio::select! { _ = &mut refresh => panic!("read must be suspended"), _ = gate.reached.notified() => {} }
            if change == "relogin" {
                let auth = Fake::new(vec![
                    (
                        Endpoint::AuthState,
                        Ok(
                            json!({"code":0,"data":{"state":"refresh-relogin","authUrl":"https://www.codebuddy.cn/login?state=refresh-relogin"}}),
                        ),
                    ),
                    (
                        Endpoint::AuthToken,
                        Ok(
                            json!({"code":0,"data":{"accessToken":"SYNTHETIC_RELOGIN_TOKEN","domain":"www.codebuddy.cn"}}),
                        ),
                    ),
                    (
                        Endpoint::Profile,
                        Ok(
                            json!({"code":0,"data":{"uid":"synthetic-uid","domain":"www.codebuddy.cn"}}),
                        ),
                    ),
                ]);
                let login = refresh_progress(super::authorization::begin(&s, &auth, NOW))
                    .await
                    .unwrap();
                refresh_progress(super::authorization::finish(
                    &s,
                    &auth,
                    &login.flow_id,
                    NOW + 1,
                ))
                .await
                .unwrap();
            } else if change == "epoch" {
                s.set_blocked(true);
                s.set_blocked(false);
            } else {
                let _owner =
                    refresh_progress(crate::services::sync_protocol::sync_mutex().lock()).await;
                let b = store::binding(&s).unwrap();
                let mut rows = store::load(&s, &b).unwrap();
                match change {
                    "generation" => rows[0].generation += 1,
                    "token" => rows[0].account.token = "SYNTHETIC_REPLACEMENT_TOKEN".into(),
                    "identity" => rows[0].account.uid = "replacement-uid".into(),
                    "delete" => rows.clear(),
                    _ => unreachable!(),
                }
                store::save(&s, &b, rows).unwrap();
            }
            let latest = std::fs::read(s.root().join(store::FILE)).unwrap();
            gate.resume.notify_one();
            assert!(
                matches!(refresh.await, Err(Failure::ExpiredAuthorization)),
                "{change}"
            );
            let expected = if endpoint == Endpoint::Summary {
                vec![Endpoint::Summary]
            } else {
                vec![
                    Endpoint::Summary,
                    Endpoint::Paid,
                    Endpoint::Free,
                    Endpoint::Status,
                ]
            };
            assert_eq!(
                *gate.inner.calls.lock().unwrap(),
                expected,
                "{change}: no follow-up request may use the retired snapshot"
            );
            assert_eq!(
                latest,
                std::fs::read(s.root().join(store::FILE)).unwrap(),
                "{change}"
            );
        }
    }
}

#[tokio::test]
async fn refresh_cancellation_releases_lease_without_writing_partial_results() {
    let (s, id) = setup();
    let before = std::fs::read(s.root().join(store::FILE)).unwrap();
    let gate = RefreshReadGate::new(Endpoint::Summary, 10.0, false);
    let engine = Engine {
        session: &s,
        transport: &gate,
        clock: &super::clock::FixedClock(NOW),
    };
    let mut refresh = Box::pin(engine.refresh(&id));
    tokio::select! { _ = &mut refresh => panic!("Summary must be suspended"), _ = gate.reached.notified() => {} }
    drop(refresh);
    assert_eq!(before, std::fs::read(s.root().join(store::FILE)).unwrap());
    gate.resume.notify_one();
    assert_eq!(
        refresh_progress(engine.refresh(&id))
            .await
            .unwrap()
            .credits
            .total_remaining,
        Some(10.0)
    );
}

#[tokio::test]
async fn refresh_waiting_to_merge_across_midnight_does_not_relabel_yesterday_data() {
    let (s, id) = setup();
    let before = 1_800_028_799_000;
    let clock = MovingClock(std::sync::atomic::AtomicI64::new(before));
    let gate = RefreshReadGate::new(Endpoint::Status, 10.0, true);
    let engine = Engine {
        session: &s,
        transport: &gate,
        clock: &clock,
    };
    let original = std::fs::read(s.root().join(store::FILE)).unwrap();
    let mut refresh = Box::pin(engine.refresh(&id));
    tokio::select! { _ = &mut refresh => panic!("Status must be suspended"), _ = gate.reached.notified() => {} }
    let owner = refresh_progress(crate::services::sync_protocol::sync_mutex().lock()).await;
    gate.resume.notify_one();
    let polled = std::future::poll_fn(|cx| std::task::Poll::Ready(refresh.as_mut().poll(cx))).await;
    assert!(polled.is_pending(), "HTTP completed; merge waits for owner");
    assert_eq!(
        gate.inner.calls.lock().unwrap().last(),
        Some(&Endpoint::Status)
    );
    clock
        .0
        .store(before + 2000, std::sync::atomic::Ordering::SeqCst);
    drop(owner);
    let row = refresh.await.unwrap();
    assert_eq!(row.claim_state, ClaimState::Unconfirmed);
    assert_eq!(row.credits.total_remaining, None);
    assert_eq!(original, std::fs::read(s.root().join(store::FILE)).unwrap());
}

#[tokio::test]
async fn automatic_failed_account_is_reported_without_starving_other_accounts() {
    let (session, ids) = account_isolation_fixture();
    let transport = Fake::new(vec![
        (Endpoint::Status, Err(Failure::Unconfirmed)),
        status(true),
    ]);
    let engine = Engine {
        session: &session,
        transport: &transport,
        clock: &super::clock::FixedClock(NOW),
    };
    enable(&engine, &ids[0]).await;
    enable(&engine, &ids[1]).await;
    assert_eq!(engine.automatic_tick().await.unwrap(), 1);
    assert!(transport.queue.lock().unwrap().is_empty());
    let rows = engine.list().await.unwrap();
    assert_eq!(
        rows.iter().find(|r| r.id == ids[0]).unwrap().claim_state,
        ClaimState::Unconfirmed
    );
    assert_eq!(
        rows.iter().find(|r| r.id == ids[1]).unwrap().claim_state,
        ClaimState::AlreadyClaimed
    );
    assert!(!transport.calls.lock().unwrap().contains(&Endpoint::Claim));
}
