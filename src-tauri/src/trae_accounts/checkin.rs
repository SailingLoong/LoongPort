//! Synthetic-only CN check-in protocol research; no production consumer is wired.
//! CN authorization and check-in remain unimplemented product requirements.
use super::{
    inference::{self, Lease, OfficialHttp, Transport},
    protocol,
    store::{self, Failure, Realm},
};
use crate::{proxy::ProxyError, secrets::session::SecretSession};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Inactive,
    Done,
    Claimed,
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
async fn page(
    lease: &Lease,
    name: &str,
    timeout: Duration,
    transport: &impl Transport,
) -> Result<Value, ProxyError> {
    let mut request = lease.wire(json!({"req_source":1}), timeout)?;
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
    if lease.snapshot.account.checkin_day.is_some_and(|d| d >= day) {
        return Ok(Outcome::AlreadyAttempted);
    }
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
    if protocol::denial(&status).is_some() {
        return Err(unavailable());
    }
    if status.get("checked_in").and_then(Value::as_bool) == Some(true) {
        return Ok(Outcome::Done);
    }
    match status.get("enable").and_then(Value::as_bool) {
        Some(false) => return Ok(Outcome::Inactive),
        Some(true) if status.get("checked_in").and_then(Value::as_bool) == Some(false) => (),
        _ => return Err(unavailable()),
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
    let code = answer
        .get("code")
        .or_else(|| answer.get("Code"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
    if matches!(code, Some(9074 | 9095)) {
        return Ok(Outcome::Ineligible);
    }
    if protocol::denial(&answer).is_some() || code != Some(0) {
        return Err(unavailable());
    }
    let credits = answer
        .get("credits")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(unavailable)?;
    let _ = credits; // No quota balance is inferred or written to the pool.
    Ok(Outcome::Claimed)
}
