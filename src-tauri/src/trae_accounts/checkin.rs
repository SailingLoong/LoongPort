//! CN check-in through the existing selected native account owner.
//! Third-party wire evidence: autumnsentiment/Trae2api-cn@e5eb6d13,
//! src/trae_client.py:686-750 (MIT; licenses/Trae2api-cn-MIT.txt).
//! This is not an official stable API contract or proof of live credit delivery.
//! No device rotation, identity synthesis, refusal retry, or balance inference.
use super::{
    inference::{self, Lease, OfficialHttp, Transport},
    protocol,
    store::{self, Account, AccountView, CheckinAttempt, CheckinAttemptStatus, Failure, Realm},
};
use crate::{proxy::ProxyError, secrets::session::SecretSession};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum Outcome {
    Inactive,
    Done,
    /// The claim reply reported acceptance; only a later status query can
    /// confirm this account's check-in. This is not a balance or credit delta.
    Accepted {
        reported_credits: f64,
    },
    Ineligible,
    AlreadyAttempted,
}
pub(crate) async fn run(
    session: Arc<SecretSession>,
    id: &str,
    timeout: Duration,
) -> Result<Outcome, ProxyError> {
    let lease = Lease::acquire(session, id, "checkin")
        .await
        .map_err(|_| unavailable())?;
    run_with(lease, timeout, &OfficialHttp).await
}
fn unavailable() -> ProxyError {
    ProxyError::AuthError("trae.checkin_unavailable".into())
}

fn business_code(value: &Value) -> Result<Option<i64>, ProxyError> {
    let mut code = None;
    for value in [value.get("code"), value.get("Code")].into_iter().flatten() {
        let parsed = value
            .as_i64()
            .or_else(|| value.as_str()?.parse().ok())
            .ok_or_else(unavailable)?;
        if code.is_some_and(|old| old != parsed) {
            return Err(unavailable());
        }
        code = Some(parsed);
    }
    Ok(code)
}
async fn page(
    lease: &Lease,
    name: &str,
    timeout: Duration,
    transport: &impl Transport,
    policy: Option<u64>,
) -> Result<Value, inference::DispatchFailure> {
    use inference::DispatchFailure::{NotDispatched, OutcomeUnknown};
    let mut request = lease.wire(json!({}), timeout).map_err(NotDispatched)?;
    // Growth pages receive the original selected device, never synthesized
    // browser/OS/brand metadata or a client-impersonating User-Agent.
    request.url = format!(
        "{}/trae/api/v2/ug/checkin_credits/{name}",
        lease.snapshot.account.realm.auth()
    );
    request
        .headers
        .insert("accept", http::HeaderValue::from_static("application/json"));
    let response = inference::admit_checked(
        &lease.session,
        &lease.snapshot,
        transport,
        request,
        false,
        |a| {
            if name == "claim" {
                automatic_admission(a, policy)?;
                if !same_pending(a, &lease.snapshot.account) {
                    return Err(Failure::Retired);
                }
            }
            Ok(())
        },
    )
    .await?;
    lease.fence().check().await.map_err(OutcomeUnknown)?;
    if !response.status().is_success()
        || response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
    {
        return Err(OutcomeUnknown(unavailable()));
    }
    let bytes = response
        .bytes_with_limit(1024 * 1024)
        .await
        .map_err(|_| OutcomeUnknown(unavailable()))?;
    lease.fence().check().await.map_err(OutcomeUnknown)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| OutcomeUnknown(unavailable()))?;
    if !value.is_object() {
        return Err(OutcomeUnknown(unavailable()));
    }
    Ok(value)
}
pub(super) async fn run_with(
    lease: Lease,
    timeout: Duration,
    transport: &impl Transport,
) -> Result<Outcome, ProxyError> {
    run_with_policy(lease, timeout, transport, None).await
}
fn automatic_admission(a: &Account, policy: Option<u64>) -> Result<(), Failure> {
    if policy.is_some_and(|revision| {
        !a.can_set_automatic() || !a.auto_checkin_enabled || a.auto_checkin_revision != revision
    }) {
        return Err(Failure::Retired);
    }
    Ok(())
}
fn same_pending(a: &Account, prepared: &Account) -> bool {
    a.version == prepared.version
        && a.generation == prepared.generation
        && a.checkin_day == prepared.checkin_day
        && a.checkin_attempt
            .as_ref()
            .zip(prepared.checkin_attempt.as_ref())
            .is_some_and(|(a, b)| {
                a.id == b.id
                    && a.generation == b.generation
                    && a.version == b.version
                    && a.status == CheckinAttemptStatus::Pending
            })
}
async fn run_with_policy(
    mut lease: Lease,
    timeout: Duration,
    transport: &impl Transport,
    policy: Option<u64>,
) -> Result<Outcome, ProxyError> {
    super::auth::synchronize(&lease.session, &mut lease.snapshot).await?;
    if lease.snapshot.account.realm != Realm::Cn {
        return Ok(Outcome::Inactive);
    }
    automatic_admission(&lease.snapshot.account, policy).map_err(|_| unavailable())?;
    // Reuse the existing CN UTC+8 day; it never asserts a daily entitlement.
    let day = (store::now() / 1000 + 8 * 3600).div_euclid(86400);
    if lease
        .snapshot
        .account
        .grant
        .as_ref()
        .ok_or_else(unavailable)?
        .expires
        <= store::now() + 120000
    {
        super::auth::refresh(&lease.session, &mut lease.snapshot, timeout, transport).await?;
    }
    let status = page(&lease, "status", timeout, transport, policy)
        .await
        .map_err(inference::DispatchFailure::error)?;
    let code = business_code(&status)?;
    if status.get("error").is_some()
        || protocol::denial(&status).is_some()
        || code.is_some_and(|c| c != 0)
    {
        return Err(unavailable());
    }
    let enabled = status
        .get("enable")
        .and_then(Value::as_bool)
        .ok_or_else(unavailable)?;
    let checked_in = status
        .get("checked_in")
        .and_then(Value::as_bool)
        .ok_or_else(unavailable)?;
    // A status page may confirm today, but cannot resolve an older unknown POST.
    if checked_in {
        return Ok(Outcome::Done);
    }
    if !enabled {
        return Ok(Outcome::Inactive);
    }
    let prepared = store::access(&lease.session, Some(&lease.snapshot.binding), |d, b| {
        let current = store::matched(d, &lease.snapshot)?;
        if current.version != lease.snapshot.account.version || current.refresh_pending {
            return Err(Failure::Retired);
        }
        automatic_admission(current, policy)?;
        if current.checkin_unresolved() || current.checkin_day.is_some_and(|d| d >= day) {
            return Ok((None, false));
        }
        let before = current.clone();
        current.checkin_day = Some(day);
        current.checkin_attempt = Some(CheckinAttempt {
            id: uuid::Uuid::new_v4().to_string(),
            generation: current.generation,
            version: current.version,
            status: CheckinAttemptStatus::Pending,
        });
        Ok((
            Some((
                store::Snapshot {
                    binding: b.clone(),
                    account: current.clone(),
                },
                before,
            )),
            true,
        ))
    })
    .await
    .map_err(|_| unavailable())?;
    let Some((prepared, before)) = prepared else {
        return Ok(Outcome::AlreadyAttempted);
    };
    lease.snapshot = prepared;
    #[cfg(test)]
    super::tests::before_checkin_dispatch(&lease.session).await;
    let response = page(&lease, "claim", timeout, transport, policy).await;
    let response = match response {
        Err(inference::DispatchFailure::NotDispatched(error)) => {
            // Roll back only this exact unsent intent. A new generation/version,
            // replacement attempt, cancellation or publication uncertainty wins.
            store::access(&lease.session, Some(&lease.snapshot.binding), |d, _| {
                let a = store::matched(d, &lease.snapshot)?;
                if !same_pending(a, &lease.snapshot.account) {
                    return Err(Failure::Retired);
                }
                a.checkin_day = before.checkin_day;
                a.checkin_attempt = before.checkin_attempt.clone();
                Ok(((), true))
            })
            .await
            .map_err(|_| unavailable())?;
            return Err(error);
        }
        Err(inference::DispatchFailure::OutcomeUnknown(error)) => Err(error),
        Ok(answer) => parse_claim(answer),
    };
    store::access(&lease.session, Some(&lease.snapshot.binding), |d, _| {
        let a = store::matched(d, &lease.snapshot)?;
        if !same_pending(a, &lease.snapshot.account) {
            return Err(Failure::Retired);
        }
        a.checkin_attempt.as_mut().ok_or(Failure::Retired)?.status = match &response {
            Ok(Outcome::Accepted { .. }) => CheckinAttemptStatus::Accepted,
            Ok(Outcome::Ineligible) => CheckinAttemptStatus::Ineligible,
            _ => CheckinAttemptStatus::Unknown,
        };
        Ok(((), true))
    })
    .await
    .map_err(|_| unavailable())?;
    response
}
fn parse_claim(answer: Value) -> Result<Outcome, ProxyError> {
    let code = business_code(&answer)?;
    if matches!(code, Some(9074 | 9095)) {
        return Ok(Outcome::Ineligible);
    }
    if answer.get("error").is_some()
        || protocol::denial(&answer).is_some()
        || code.is_some_and(|code| code != 0)
    {
        return Err(unavailable());
    }
    let credits = answer
        .get("credits")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(unavailable)?;
    Ok(Outcome::Accepted {
        reported_credits: credits,
    })
}
/// Exact account generation and policy revision consent, matching MiniMax's owner.
pub(crate) async fn set_automatic(
    session: &SecretSession,
    id: &str,
    generation: u64,
    revision: u64,
    enabled: bool,
) -> Result<AccountView, Failure> {
    store::access(session, None, |d, _| {
        let a = d
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(Failure::Retired)?;
        if a.generation != generation || a.auto_checkin_revision != revision {
            return Err(Failure::Retired);
        }
        if enabled && !a.can_set_automatic() {
            return Err(Failure::Unsupported);
        }
        a.auto_checkin_revision = a
            .auto_checkin_revision
            .checked_add(1)
            .ok_or(Failure::Storage)?;
        a.auto_checkin_enabled = enabled;
        Ok((a.view(), true))
    })
    .await
}
pub(crate) async fn automatic_tick(session: Arc<SecretSession>) -> Result<(), Failure> {
    automatic_tick_with(session, &OfficialHttp).await
}
pub(super) async fn automatic_tick_with(
    session: Arc<SecretSession>,
    transport: &impl Transport,
) -> Result<(), Failure> {
    for view in store::list(&session).await? {
        if !view.auto_checkin_enabled || !view.can_set_automatic {
            continue;
        }
        let Ok(lease) = Lease::acquire(session.clone(), &view.id, "automatic-checkin").await else {
            continue;
        };
        if lease.snapshot.account.generation != view.generation {
            continue;
        }
        // Each account is isolated. A refusal never triggers a second claim;
        // later ticks may observe status, while durable unknown remains blocked.
        let _ = run_with_policy(
            lease,
            Duration::from_secs(30),
            transport,
            Some(view.auto_checkin_revision),
        )
        .await;
    }
    Ok(())
}
