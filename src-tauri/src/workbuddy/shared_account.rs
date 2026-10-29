//! Canonical account reads/claims borrow the original inference credential owner.
//! No token store, refresh loop, endpoint configuration or claim retry is added.
use super::*;
use crate::workbuddy::{
    engine,
    model::today,
    protocol::{DispatchFailure, Endpoint as AccountEndpoint},
};

pub(super) struct Admission<'a> {
    snapshot: &'a SavedAccount,
    policy: Option<(u64, u64)>,
    day: Option<&'a str>,
    clock: &'a dyn crate::workbuddy::clock::Clock,
    started: AtomicBool,
}
impl Admission<'_> {
    pub(super) fn dispatched(&self) {
        self.started.store(true, Ordering::Release);
    }
    pub(super) fn check(
        &self,
        _session: &SecretSession,
        rows: &[SavedAccount],
        binding: &StorageIdentity,
    ) -> Result<(), Failure> {
        self.snapshot.account.api_token()?;
        let i = engine::matched(rows, binding, self.snapshot)?;
        rows[i]
            .account
            .api_token()
            .map_err(|_| Failure::ExpiredAuthorization)?;
        if self.day.is_some() {
            engine::automatic_admission(&rows[i], self.policy)?;
            if rows[i].pending != self.snapshot.pending
                || self.day != Some(today(self.clock.now_ms()).as_str())
            {
                return Err(Failure::ExpiredAuthorization);
            }
        }
        Ok(())
    }
}
pub(crate) async fn request(
    session: &Arc<SecretSession>,
    snapshot: &SavedAccount,
    endpoint: AccountEndpoint,
    policy: Option<(u64, u64)>,
    day: Option<&str>,
    clock: &dyn crate::workbuddy::clock::Clock,
) -> Result<Value, DispatchFailure> {
    request_with(session, snapshot, endpoint, policy, day, clock, &Official).await
}
pub(super) async fn request_with<T: Transport>(
    session: &Arc<SecretSession>,
    snapshot: &SavedAccount,
    endpoint: AccountEndpoint,
    policy: Option<(u64, u64)>,
    day: Option<&str>,
    clock: &dyn crate::workbuddy::clock::Clock,
    transport: &T,
) -> Result<Value, DispatchFailure> {
    use DispatchFailure::{NotDispatched, OutcomeUnknown};
    if !snapshot.account.is_api()
        || !snapshot.shared_authorization
        || matches!(
            endpoint,
            AccountEndpoint::AuthState | AccountEndpoint::AuthToken | AccountEndpoint::Profile
        )
    {
        return Err(NotDispatched(Failure::UnsupportedContext));
    }
    let now = clock.now_ms();
    let binding = store::binding(session).map_err(NotDispatched)?;
    let account_id = snapshot.account.id(binding.store_id());
    let mut lease = Lease::acquire_for_purpose(session.clone(), &account_id, false)
        .await
        .map_err(NotDispatched)?;
    if lease.grant.expires_ms <= now.saturating_add(EARLY_MS) {
        lease.refresh(transport, now).await.map_err(NotDispatched)?;
    }
    let admission = Admission {
        snapshot,
        policy,
        day,
        clock,
        started: AtomicBool::new(false),
    };
    // Preflight identifies a definitively unsent claim, while send_admitted repeats
    // the same checks under the final first-poll ownership fence.
    {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        let (rows, _) = lease.owner.rows(session).map_err(NotDispatched)?;
        admission
            .check(session, &rows, &binding)
            .map_err(NotDispatched)?;
    }
    let headers = signed(&lease.grant).map_err(NotDispatched)?;
    let request = Request {
        endpoint: Endpoint::Account(endpoint),
        query: None,
        headers,
        body: serde_json::to_vec(&crate::workbuddy::http::body(endpoint, now))
            .map_err(|_| NotDispatched(Failure::Unconfirmed))?,
        timeout: Duration::from_secs(20),
    };
    let response = send_admitted(
        session,
        &lease.owner,
        Some(&lease.grant),
        transport,
        request,
        None,
        Some(&admission),
    )
    .await
    .map_err(|_| {
        if admission.started.load(Ordering::Acquire) {
            OutcomeUnknown(Failure::Unconfirmed)
        } else {
            NotDispatched(Failure::ExpiredAuthorization)
        }
    })?;
    let checkin_unauthorized = response.status() == http::StatusCode::UNAUTHORIZED
        && matches!(endpoint, AccountEndpoint::Status | AccountEndpoint::Claim);
    if checkin_unauthorized {
        // Same credential and fixed origin. This is a read, never another claim.
        let summary = send_owner(
            session,
            &lease.owner,
            Some(&lease.grant),
            transport,
            Request {
                endpoint: Endpoint::Account(AccountEndpoint::Summary),
                query: None,
                headers: signed(&lease.grant).map_err(OutcomeUnknown)?,
                body: b"{}".to_vec(),
                timeout: Duration::from_secs(20),
            },
        )
        .await
        .map_err(|_| OutcomeUnknown(Failure::Unconfirmed))?;
        let summary = json_response(summary).await.map_err(OutcomeUnknown)?;
        if crate::workbuddy::checkin::success(&summary) {
            return Ok(json!({"loongportCheckinRestricted":true}));
        }
        return Err(OutcomeUnknown(Failure::NeedsVerification));
    }
    json_response(response).await.map_err(OutcomeUnknown)
}
