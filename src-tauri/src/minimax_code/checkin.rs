//! Single-dispatch check-in: the encrypted pending marker precedes the POST.
use super::{
    catalog::{self, *},
    http::{Endpoint, Request, Response, Transport},
};
use crate::secrets::session::SecretSession;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, Weak},
};

type LeaseMap = HashMap<(std::path::PathBuf, String), Weak<tokio::sync::Mutex<()>>>;
fn lease(session: &SecretSession, id: &str) -> Result<tokio::sync::OwnedMutexGuard<()>, Failure> {
    static LEASES: OnceLock<Mutex<LeaseMap>> = OnceLock::new();
    let mutex = {
        let mut map = LEASES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| Failure::Busy)?;
        map.retain(|_, v| v.strong_count() > 0);
        let key = (session.root().to_path_buf(), id.to_owned());
        let m = map
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
        map.insert(key, Arc::downgrade(&m));
        m
    };
    mutex.try_lock_owned().map_err(|_| Failure::Busy)
}
#[derive(Deserialize)]
struct Day {
    day_no: u8,
    points: f64,
    #[serde(default)]
    bonus_points: Option<f64>,
    status: u8,
    is_today: bool,
}
#[derive(Deserialize)]
struct Panel {
    scene: u8,
    days: Vec<Day>,
}
pub(super) fn data(response: &Response) -> Result<&Value, Failure> {
    if response.status == 401 {
        return Err(Failure::NeedsLogin);
    }
    if !(200..300).contains(&response.status) {
        return Err(Failure::NetworkUnavailable);
    }
    let code = response
        .body
        .get("base_resp")
        .and_then(|b| b.get("status_code"))
        .and_then(Value::as_i64);
    if code != Some(0) {
        return Err(Failure::InvalidResponse);
    }
    response
        .body
        .get("data")
        .filter(|v| v.is_object())
        .ok_or(Failure::InvalidResponse)
}
pub(super) fn panel_status(value: &Value) -> Result<Status, Failure> {
    let p: Panel = serde_json::from_value(value.clone()).map_err(|_| Failure::InvalidResponse)?;
    let mut numbers = HashSet::new();
    if p.scene > 4
        || p.days.len() != 7
        || p.days.iter().filter(|d| d.status == 2).count() > 1
        || p.days.iter().filter(|d| d.is_today).count() > 1
        || p.days.iter().any(|d| {
            !(1..=7).contains(&d.day_no)
                || !numbers.insert(d.day_no)
                || !d.points.is_finite()
                || d.points < 0.0
                || d.bonus_points.is_some_and(|n| !n.is_finite() || n < 0.0)
                || !(1..=4).contains(&d.status)
        })
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(if p.days.iter().any(|d| d.is_today && d.status == 3) {
        Status::AlreadyClaimed
    } else if p.days.iter().any(|d| d.status == 2) {
        Status::Claimable
    } else {
        Status::Unavailable
    })
}
#[derive(Deserialize)]
struct Receipt {
    claim_id: String,
    claim_result: u8,
    day_no: u8,
    points: f64,
    expire_at_ms: f64,
    panel: Value,
}
fn receipt(response: Response) -> Result<(AttemptStatus, f64, String), Failure> {
    let receipt: Receipt =
        serde_json::from_value(data(&response)?.clone()).map_err(|_| Failure::InvalidResponse)?;
    panel_status(&receipt.panel)?;
    if receipt.claim_id.trim().is_empty()
        || receipt.claim_id.len() > 1024
        || !(1..=2).contains(&receipt.claim_result)
        || !(1..=7).contains(&receipt.day_no)
        || !receipt.points.is_finite()
        || receipt.points < 0.0
        || !receipt.expire_at_ms.is_finite()
    {
        return Err(Failure::InvalidResponse);
    }
    Ok((
        if receipt.claim_result == 1 {
            AttemptStatus::Claimed
        } else {
            AttemptStatus::AlreadyClaimed
        },
        receipt.points,
        receipt.claim_id,
    ))
}
pub(super) fn request(snapshot: &Snapshot, endpoint: Endpoint) -> Result<Request, Failure> {
    let credentials = snapshot
        .account
        .credentials
        .as_ref()
        .ok_or(Failure::NeedsLogin)?;
    Ok(Request {
        endpoint,
        region: snapshot.account.region,
        access_token: Some(credentials.access_token.clone()),
        real_user_id: Some(snapshot.account.real_user_id.clone()),
        body: if endpoint == Endpoint::Claim {
            Some(serde_json::json!({}))
        } else {
            None
        },
    })
}
pub(crate) struct Engine<'a, T: Transport> {
    pub session: &'a SecretSession,
    pub transport: &'a T,
}
impl<T: Transport> Engine<'_, T> {
    pub async fn list(&self) -> Result<Vec<AccountView>, Failure> {
        catalog::list(self.session).await
    }
    async fn send(
        &self,
        snapshot: &Snapshot,
        endpoint: Endpoint,
        policy: Option<u64>,
    ) -> Result<Response, Failure> {
        self.send_classified(snapshot, endpoint, policy)
            .await
            .map_err(catalog::DispatchFailure::failure)
    }
    async fn send_classified(
        &self,
        snapshot: &Snapshot,
        endpoint: Endpoint,
        policy: Option<u64>,
    ) -> Result<Response, catalog::DispatchFailure> {
        catalog::dispatch_classified(
            self.session,
            &snapshot.binding,
            self.transport,
            request(snapshot, endpoint).map_err(catalog::DispatchFailure::NotDispatched)?,
            |d, _| {
                let current = catalog::matched(d, snapshot)?;
                if current.credential_version != snapshot.account.credential_version {
                    return Err(Failure::StaleIdentity);
                }
                if endpoint == Endpoint::Claim {
                    automatic_admission(current, policy)?;
                }
                if endpoint == Endpoint::Claim
                    && current.operation.as_ref().map(|o| &o.attempt_id)
                        != snapshot.account.operation.as_ref().map(|o| &o.attempt_id)
                {
                    return Err(Failure::StaleIdentity);
                }
                Ok(())
            },
        )
        .await
    }
    async fn get_status(
        &self,
        mut snapshot: Snapshot,
        automatic: bool,
    ) -> Result<(Snapshot, Status), Failure> {
        let mut response = self.send(&snapshot, Endpoint::Status, None).await?;
        if response.status == 401 {
            snapshot =
                super::auth::refresh_snapshot(self.session, self.transport, &snapshot).await?;
            response = self.send(&snapshot, Endpoint::Status, None).await?;
        }
        let status = if automatic {
            automatic_panel_status(data(&response)?)?
        } else {
            panel_status(data(&response)?)?
        };
        Ok((snapshot, status))
    }
    pub async fn refresh(&self, id: &str) -> Result<AccountView, Failure> {
        let _lease = lease(self.session, id)?;
        let snapshot = catalog::snapshot(self.session, id).await?;
        let (snapshot, status) = self.get_status(snapshot, false).await?;
        catalog::access(self.session, Some(&snapshot.binding), |d, _| {
            let a = catalog::matched(d, &snapshot)?;
            a.last_checked_at = Some(now_ms());
            a.last_observed_status = Some(status);
            // A daily status panel does not identify the original claim. It cannot clear
            // an uncertain attempt, including tomorrow or after reauthorization.
            if !a.unresolved() {
                a.status = status;
            }
            Ok((a.view(), true))
        })
        .await
    }
    pub async fn recheck(&self, id: &str) -> Result<AccountView, Failure> {
        self.refresh(id).await
    }
    pub async fn claim(&self, id: &str) -> Result<AccountView, Failure> {
        self.claim_with_policy(id, None).await
    }
    async fn claim_with_policy(
        &self,
        id: &str,
        policy: Option<u64>,
    ) -> Result<AccountView, Failure> {
        let _lease = lease(self.session, id)?;
        let snapshot = catalog::snapshot(self.session, id).await?;
        automatic_admission(&snapshot.account, policy)?;
        if snapshot.account.unresolved() {
            if policy.is_some() {
                let (snapshot, status) = self.get_status(snapshot, policy.is_some()).await?;
                return catalog::access(self.session, Some(&snapshot.binding), |d, _| {
                    let a = catalog::matched(d, &snapshot)?;
                    a.last_checked_at = Some(now_ms());
                    a.last_observed_status = Some(status);
                    // Query only: the daily panel cannot resolve an old POST.
                    Ok((a.view(), true))
                })
                .await;
            }
            return Ok(snapshot.account.view());
        }
        let (snapshot, status) = self.get_status(snapshot, policy.is_some()).await?;
        let now = now_ms();
        let operation_day = day(now);
        let prepared = catalog::access(self.session, Some(&snapshot.binding), |d, b| {
            let a = catalog::matched(d, &snapshot)?;
            if a.credential_version != snapshot.account.credential_version {
                return Err(Failure::StaleIdentity);
            }
            automatic_admission(a, policy)?;
            a.last_checked_at = Some(now);
            a.last_observed_status = Some(status);
            if a.unresolved() {
                return Ok((None, false));
            }
            // A contradictory same-day panel cannot erase our terminal receipt.
            if a.operation.as_ref().is_some_and(|o| o.day == operation_day) {
                return Ok((None, true));
            }
            a.status = status;
            if status != Status::Claimable {
                return Ok((None, true));
            }
            a.operation = Some(Operation {
                attempt_id: uuid::Uuid::new_v4().to_string(),
                started_at: now,
                day: operation_day.clone(),
                bound_auth_generation: a.auth_generation,
                claim_id: None,
                status: AttemptStatus::Pending,
                credit_amount: None,
            });
            a.status = Status::Unknown;
            Ok((
                Some(Snapshot {
                    binding: b.clone(),
                    account: a.clone(),
                }),
                true,
            ))
        })
        .await?;
        let Some(prepared) = prepared else {
            return catalog::access(self.session, Some(&snapshot.binding), |d, _| {
                Ok((catalog::matched(d, &snapshot)?.view(), false))
            })
            .await;
        };
        let attempt = prepared
            .account
            .operation
            .as_ref()
            .ok_or(Failure::StorageUnavailable)?
            .attempt_id
            .clone();
        // The prior access returns only after durable publication and authenticated
        // readback. Revalidate again immediately before the one network dispatch.
        catalog::access(self.session, Some(&prepared.binding), |d, _| {
            let a = catalog::matched(d, &prepared)?;
            if a.operation.as_ref().map(|o| o.attempt_id.as_str()) != Some(attempt.as_str()) {
                return Err(Failure::StaleIdentity);
            }
            Ok(((), false))
        })
        .await?;
        let response = self
            .send_classified(&prepared, Endpoint::Claim, policy)
            .await;
        let response = match response {
            Err(catalog::DispatchFailure::NotDispatched(_)) => {
                // No transport future was created/polled. Only the exact original
                // Pending intent may be withdrawn; newer identity/attempts survive.
                return catalog::access(self.session, Some(&prepared.binding), |d, _| {
                    let a = catalog::matched(d, &prepared)?;
                    if a.credential_version != prepared.account.credential_version
                        || !a.operation.as_ref().is_some_and(|op| {
                            op.attempt_id == attempt
                                && op.status == AttemptStatus::Pending
                                && op.bound_auth_generation == prepared.account.auth_generation
                        })
                    {
                        return Err(Failure::StaleIdentity);
                    }
                    a.operation = snapshot.account.operation.clone();
                    a.status = status;
                    Ok((a.view(), true))
                })
                .await;
            }
            Err(catalog::DispatchFailure::OutcomeUnknown(failure)) => Err(failure),
            Ok(response) => Ok(response),
        };
        // A 401 is allowed to refresh the same identity, never to replay this POST.
        if response.as_ref().is_ok_and(|r| r.status == 401) {
            let _ = super::auth::refresh_snapshot(self.session, self.transport, &prepared).await;
        }
        let outcome = response.and_then(receipt);
        catalog::access(self.session, Some(&prepared.binding), |d, _| {
            let a = catalog::matched(d, &prepared)?;
            let op = a
                .operation
                .as_mut()
                .filter(|o| o.attempt_id == attempt)
                .ok_or(Failure::StaleIdentity)?;
            match outcome {
                Ok((result, points, claim_id)) => {
                    op.claim_id = Some(claim_id);
                    op.status = result;
                    op.credit_amount = Some(points);
                    a.status = if result == AttemptStatus::Claimed {
                        Status::Claimed
                    } else {
                        Status::AlreadyClaimed
                    };
                }
                Err(_) => {
                    op.status = AttemptStatus::Unknown;
                    a.status = Status::Unknown;
                }
            }
            Ok((a.view(), true))
        })
        .await
    }
    /// Persist opt-in per authenticated account. Old UI revisions cannot enable a new login.
    pub async fn set_automatic(
        &self,
        id: &str,
        auth_generation: u64,
        revision: u64,
        enabled: bool,
    ) -> Result<AccountView, Failure> {
        catalog::access(self.session, None, |d, _| {
            let a = d
                .accounts
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or(Failure::StaleIdentity)?;
            if a.auth_generation != auth_generation || a.auto_claim_revision != revision {
                return Err(Failure::StaleIdentity);
            }
            if enabled && a.credentials.is_none() {
                return Err(Failure::NeedsLogin);
            }
            a.auto_claim_revision = a
                .auto_claim_revision
                .checked_add(1)
                .ok_or(Failure::StorageUnavailable)?;
            a.auto_claim_enabled = enabled;
            a.auto_claim_failure = None;
            Ok((a.view(), true))
        })
        .await
    }
    /// No local midnight/reset/credit arithmetic. The existing owner consults the
    /// live panel, then applies its durable intent and same-day receipt protection.
    pub async fn automatic_tick(&self) -> Result<(), Failure> {
        for view in self.list().await? {
            if !view.auto_claim_enabled || !view.authorized {
                continue;
            }
            let result = self
                .claim_with_policy(&view.id, Some(view.auto_claim_revision))
                .await;
            let error = result.err();
            // Persist only an observation of this same policy and login, never overwrite
            // a policy edit/re-login that happened while the network call was pending.
            catalog::access(self.session, None, |d, _| {
                let Some(a) = d.accounts.iter_mut().find(|a| a.id == view.id) else {
                    return Ok(((), false));
                };
                if a.auth_generation != view.auth_generation
                    || a.auto_claim_revision != view.auto_claim_revision
                {
                    return Ok(((), false));
                }
                let changed = a.auto_claim_failure != error;
                a.auto_claim_failure = error;
                Ok(((), changed))
            })
            .await?;
        }
        Ok(())
    }
    pub async fn retire(&self, id: &str) -> Result<(), Failure> {
        catalog::access(self.session, None, |d, _| {
            let a = d
                .accounts
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or(Failure::UnsupportedContext)?;
            a.credentials = None;
            a.auth_generation = a
                .auth_generation
                .checked_add(1)
                .ok_or(Failure::StorageUnavailable)?;
            if !a.unresolved() {
                a.status = Status::NeedsLogin
            };
            Ok(((), true))
        })
        .await
    }
}

fn automatic_admission(account: &Account, policy: Option<u64>) -> Result<(), Failure> {
    if policy.is_some_and(|revision| {
        !account.auto_claim_enabled || account.auto_claim_revision != revision
    }) {
        return Err(Failure::StaleIdentity);
    }
    Ok(())
}

pub(super) fn automatic_panel_status(value: &Value) -> Result<Status, Failure> {
    let status = panel_status(value)?;
    let panel: Panel =
        serde_json::from_value(value.clone()).map_err(|_| Failure::InvalidResponse)?;
    Ok(
        if status == Status::Claimable && !panel.days.iter().any(|d| d.is_today && d.status == 2) {
            Status::Unavailable
        } else {
            status
        },
    )
}
