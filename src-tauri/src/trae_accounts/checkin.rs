//! CN check-in through the existing selected native account owner.
//! Third-party wire evidence: autumnsentiment/Trae2api-cn@e5eb6d13,
//! src/trae_client.py:686-750 (MIT; licenses/Trae2api-cn-MIT.txt).
//! This is not an official stable API contract or proof of live credit delivery.
//! No device rotation, identity synthesis, refusal retry, or balance inference.
use super::{
    inference::{self, Lease, OfficialHttp, Transport},
    protocol,
    store::{self, Failure, Realm},
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
) -> Result<Value, ProxyError> {
    let mut request = lease.wire(json!({}), timeout)?;
    // Growth pages receive the original selected device, never synthesized
    // browser/OS/brand metadata or a client-impersonating User-Agent.
    request.url = format!(
        "{}/trae/api/v2/ug/checkin_credits/{name}",
        lease.snapshot.account.realm.auth()
    );
    request
        .headers
        .insert("accept", http::HeaderValue::from_static("application/json"));
    let response =
        inference::admit(&lease.session, &lease.snapshot, transport, request, false).await?;
    lease.fence().check().await?;
    if !response.status().is_success()
        || response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
    {
        return Err(unavailable());
    }
    let bytes = response
        .bytes_with_limit(1024 * 1024)
        .await
        .map_err(|_| unavailable())?;
    lease.fence().check().await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if !value.is_object() {
        return Err(unavailable());
    }
    Ok(value)
}
pub(super) async fn run_with(
    mut lease: Lease,
    timeout: Duration,
    transport: &impl Transport,
) -> Result<Outcome, ProxyError> {
    super::auth::synchronize(&lease.session, &mut lease.snapshot).await?;
    if lease.snapshot.account.realm != Realm::Cn {
        return Ok(Outcome::Inactive);
    }
    // CN calendar day is only an at-most-once safety boundary. Eligibility is
    // ALWAYS read from the service; it never asserts a daily credit entitlement.
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
    let status = page(&lease, "status", timeout, transport).await?;
    let _ = business_code(&status)?;
    if status.get("error").is_some() || protocol::denial(&status).is_some() {
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
    if checked_in {
        return Ok(Outcome::Done);
    }
    if !enabled {
        return Ok(Outcome::Inactive);
    }
    // Reads remain available after an uncertain claim, including after restart.
    // This marker prevents another write; it never fabricates checked_in=true.
    if lease.snapshot.account.checkin_day.is_some_and(|d| d >= day) {
        return Ok(Outcome::AlreadyAttempted);
    }
    let admitted = store::access(&lease.session, Some(&lease.snapshot.binding), |d, _| {
        let current = store::matched(d, &lease.snapshot)?;
        if current.version != lease.snapshot.account.version || current.refresh_pending {
            return Err(Failure::Retired);
        }
        if current.checkin_day.is_some_and(|d| d >= day) {
            return Ok((false, false));
        }
        current.checkin_day = Some(day);
        Ok((true, true))
    })
    .await
    .map_err(|_| unavailable())?;
    if !admitted {
        return Ok(Outcome::AlreadyAttempted);
    }
    // Any uncertainty after this durable admission remains non-replayable today,
    // including cancellation, transport errors, 9074 and 9095. No retry loop.
    let answer = page(&lease, "claim", timeout, transport).await?;
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
