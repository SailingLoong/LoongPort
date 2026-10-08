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
