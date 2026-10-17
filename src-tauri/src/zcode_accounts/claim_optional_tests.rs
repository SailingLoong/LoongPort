//! Synthetic transport/persistence boundary tests; never contact an account.
use super::super::{
    claim_protocol::Transport,
    official::{OfficialResponse, TransportFuture},
};
use super::*;
use serde_json::{json, Value};
use std::collections::VecDeque;
type Step = (Endpoint, bool, Result<Value, OfficialError>);
struct Fake {
    steps: Mutex<VecDeque<Step>>,
    trace: Arc<Mutex<Vec<String>>>,
}
impl Transport for Fake {
    fn send(&self, request: Request) -> TransportFuture<'_> {
        let (endpoint, proof, reply) = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("no live fallback");
        assert_eq!(request.endpoint, endpoint);
        assert_eq!(request.captcha.is_some(), proof);
        assert_eq!(request.jwt.expose(), "synthetic-jwt");
        if endpoint == Endpoint::Claim {
            assert_eq!(request.plan.as_deref(), Some("synthetic-plan"));
            assert_eq!(
                self.trace.lock().unwrap().last().unwrap(),
                "saved:ResultPending"
            );
        }
        self.trace.lock().unwrap().push(format!("{endpoint:?}"));
        Box::pin(async move {
            reply.map(|v| OfficialResponse {
                status: 200,
                body: zeroize::Zeroizing::new(serde_json::to_vec(&v).unwrap()),
            })
        })
    }
}
fn pending() -> Record {
    Record {
        status: Status::Claimable,
        plan_id: Some("synthetic-plan".into()),
        prior_plan_ids: vec!["prior".into()],
        ..Record::default()
    }
}
fn request() -> Request {
    Request {
        endpoint: Endpoint::Claim,
        version: "3.15.3".into(),
        jwt: StartJwt::new("synthetic-jwt").unwrap(),
        plan: Some("synthetic-plan".into()),
        captcha: None,
        region: None,
    }
}
async fn run(steps: Vec<Step>, human: Result<(), &'static str>) -> (Record, Vec<String>) {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let client = Client(Fake {
        steps: Mutex::new(steps.into()),
        trace: trace.clone(),
    });
    let mut r = pending();
    let persisted = trace.clone();
    let verified = trace.clone();
    submit_claim(
        &client,
        request(),
        &mut r,
        &|| Ok(()),
        move |saved: Record| {
            let trace = persisted.clone();
            async move {
                let value = serde_json::to_string(&saved).unwrap();
                assert!(!value.contains("synthetic-jwt"));
                assert!(!value.contains("synthetic-human-proof"));
                trace
                    .lock()
                    .unwrap()
                    .push(format!("saved:{:?}", saved.status));
                Ok(())
            }
        },
        move |cfg: CaptchaConfig| {
            let trace = verified.clone();
            async move {
                assert!(cfg.enabled);
                trace.lock().unwrap().push("human".into());
                human.map(|_| {
                    (
                        SecretValue::new("synthetic-human-proof").unwrap(),
                        "cn".into(),
                    )
                })
            }
        },
    )
    .await
    .unwrap();
    assert!(client.0.steps.lock().unwrap().is_empty());
    let log = trace.lock().unwrap().clone();
    (r, log)
}
fn challenge() -> Value {
    json!({"code":0,"data":{"configs":{"captcha":{"enabled":true,"region":"cn","prefix":"synthetic","sceneId":"synthetic"}}}})
}
#[tokio::test]
async fn no_challenge_claims_without_config_or_human() {
    let (r, trace) = run(
        vec![(
            Endpoint::Claim,
            false,
            Ok(json!({"code":0,"data":{"plan":{"plan_id":"synthetic-plan"}}})),
        )],
        Err("must not prompt"),
    )
    .await;
    assert_eq!(r.status, Status::Claimed);
    assert_eq!(trace, ["saved:ResultPending", "Claim", "saved:Claimed"]);
}
#[tokio::test]
async fn only_explicit_challenge_admits_one_verified_retry() {
    let (r, trace) = run(
        vec![
            (Endpoint::Claim, false, Ok(json!({"code":3007}))),
            (Endpoint::Config, false, Ok(challenge())),
            (
                Endpoint::Claim,
                true,
                Ok(json!({"code":0,"data":{"plan":{"plan_id":"synthetic-plan"}}})),
            ),
        ],
        Ok(()),
    )
    .await;
    assert_eq!(r.status, Status::Claimed);
    assert_eq!(
        trace,
        [
            "saved:ResultPending",
            "Claim",
            "saved:VerificationRequired",
            "Config",
            "human",
            "saved:ResultPending",
            "Claim",
            "saved:Claimed"
        ]
    );
}
#[tokio::test]
async fn numeric_string_challenge_uses_the_same_human_path() {
    let (r, trace) = run(
        vec![
            (Endpoint::Claim, false, Ok(json!({"code":"3007"}))),
            (Endpoint::Config, false, Ok(challenge())),
            (
                Endpoint::Claim,
                true,
                Ok(json!({"code":"0","data":{"plan":{"plan_id":"synthetic-plan"}}})),
            ),
        ],
        Ok(()),
    )
    .await;
    assert_eq!(r.status, Status::Claimed);
    assert_eq!(trace.iter().filter(|v| *v == "human").count(), 1);
}
#[tokio::test]
async fn disabled_or_invalid_challenge_config_does_not_prompt_or_resubmit() {
    for cfg in [
        json!({"code":0,"data":{"configs":{"captcha":{"enabled":false}}}}),
        json!({"code":0,"data":{}}),
    ] {
        let (r, trace) = run(
            vec![
                (Endpoint::Claim, false, Ok(json!({"code":3007}))),
                (Endpoint::Config, false, Ok(cfg)),
            ],
            Err("must not prompt"),
        )
        .await;
        assert_eq!(r.status, Status::VerificationRequired);
        assert_eq!(r.reason.as_deref(), Some("captchaUnavailable"));
        assert!(!trace.iter().any(|v| v == "human"));
        assert_eq!(trace.iter().filter(|v| *v == "Claim").count(), 1);
    }
}
#[tokio::test]
async fn challenge_cancel_or_second_rejection_never_loops() {
    let (r, trace) = run(
        vec![
            (Endpoint::Claim, false, Ok(json!({"code":3007}))),
            (Endpoint::Config, false, Ok(challenge())),
        ],
        Err("cancelled"),
    )
    .await;
    assert_eq!(r.status, Status::VerificationRequired);
    assert_eq!(trace.iter().filter(|v| *v == "Claim").count(), 1);
    let (r, trace) = run(
        vec![
            (Endpoint::Claim, false, Ok(json!({"code":3007}))),
            (Endpoint::Config, false, Ok(challenge())),
            (Endpoint::Claim, true, Ok(json!({"code":3007}))),
        ],
        Ok(()),
    )
    .await;
    assert_eq!(r.status, Status::VerificationRequired);
    assert_eq!(trace.iter().filter(|v| *v == "human").count(), 1);
    assert_eq!(trace.iter().filter(|v| *v == "Claim").count(), 2);
}
#[tokio::test]
async fn uncertain_claim_reads_balance_and_never_resubmits() {
    for balance in [
        json!({"code":0,"data":{"plans":[]}}),
        json!({"code":0,"data":{"plans":[{"plan_id":"synthetic-plan","user_plan_id":"new","status":"active"}]}}),
    ] {
        let (r, trace) = run(
            vec![
                (Endpoint::Claim, false, Err(OfficialError::Timeout)),
                (Endpoint::Balance, false, Ok(balance.clone())),
            ],
            Err("must not prompt"),
        )
        .await;
        assert_eq!(
            r.status,
            if balance["data"]["plans"].as_array().unwrap().is_empty() {
                Status::ResultPending
            } else {
                Status::Claimed
            }
        );
        assert_eq!(trace.iter().filter(|v| *v == "Claim").count(), 1);
        assert!(!trace.iter().any(|v| v == "human"));
    }
}
#[tokio::test]
async fn pending_or_failed_persistence_admits_no_claim() {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let client = Client(Fake {
        steps: Mutex::new(VecDeque::new()),
        trace,
    });
    let mut r = pending();
    r.mark_submitted();
    let mut restored: Record = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert!(submit_claim(
        &client,
        request(),
        &mut restored,
        &|| Ok(()),
        |_| async { Ok(()) },
        |_| async { Err("must not prompt") }
    )
    .await
    .is_err());
    let mut r = pending();
    assert!(submit_claim(
        &client,
        request(),
        &mut r,
        &|| Ok(()),
        |_| async { Err("locked") },
        |_| async { Err("must not prompt") }
    )
    .await
    .is_err());
}
