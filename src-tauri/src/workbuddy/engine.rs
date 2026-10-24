//! Manual and opted-in automatic operations share the existing sync/vault lifecycle owner.
use super::{
    checkin::{self, ClaimState},
    credits::{self, Credits},
    model::*,
    protocol::{DispatchFailure, Endpoint, Transport},
    store,
};
use crate::secrets::{session::SecretSession, VaultMetadata};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};
// A weak per-account lease prevents queued double clicks from becoming two submits.
fn lease(session: &SecretSession, id: &str) -> Result<tokio::sync::OwnedMutexGuard<()>, Failure> {
    static LEASES: OnceLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let key = format!("{}:{id}", session.root().display());
    let mutex = {
        let mut leases = LEASES
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| Failure::Busy)?;
        leases.retain(|_, v| v.strong_count() > 0);
        let mutex = leases
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
        leases.insert(key, Arc::downgrade(&mutex));
        mutex
    };
    mutex.try_lock_owned().map_err(|_| Failure::Busy)
}
pub(crate) struct Engine<'a, T: Transport> {
    pub session: &'a SecretSession,
    pub transport: &'a T,
    pub clock: &'a dyn super::clock::Clock,
}
impl<T: Transport> Engine<'_, T> {
    pub async fn list(&self) -> Result<Vec<AccountView>, Failure> {
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let binding = store::binding(self.session)?;
        Ok(store::load(self.session, &binding)?
            .iter()
            .map(|r| r.view(&binding.vault_id, &today(self.clock.now_ms())))
            .collect())
    }
    async fn request(&self, b: &VaultMetadata, e: Endpoint, a: &Account) -> serde_json::Value {
        if store::binding(self.session).ok().as_ref() != Some(b) {
            return serde_json::json!({});
        }
        self.transport
            .request(e, Some(a), None, self.clock.now_ms())
            .await
            .unwrap_or_else(|error| match error {
                Failure::NeedsVerification => serde_json::json!({"code":401}),
                _ => serde_json::json!({}),
            })
    }
    async fn status(&self, b: &VaultMetadata, a: &Account) -> ClaimState {
        let request_day = today(self.clock.now_ms());
        let response = self.request(b, Endpoint::Status, a).await;
        if request_day != today(self.clock.now_ms()) {
            ClaimState::Unconfirmed
        } else {
            checkin::status(&response)
        }
    }
    async fn credits(&self, b: &VaultMetadata, a: &Account) -> Credits {
        let snapshot_day = today(self.clock.now_ms());
        let summary = self.request(b, Endpoint::Summary, a).await;
        let paid = self.request(b, Endpoint::Paid, a).await;
        let free = self.request(b, Endpoint::Free, a).await;
        if snapshot_day != today(self.clock.now_ms()) {
            return Credits::default();
        }
        credits::normalize(&summary, &paid, &free, self.clock.now_ms())
    }
    fn persist(
        &self,
        b: &VaultMetadata,
        rows: Vec<SavedAccount>,
        index: usize,
    ) -> Result<AccountView, Failure> {
        let view = rows[index].view(&b.vault_id, &today(self.clock.now_ms()));
        store::save(self.session, b, rows)?;
        Ok(view)
    }
    fn resolve_pending(&self, row: &mut SavedAccount, state: ClaimState, credits: &Credits) {
        let Some(attempt) = row.pending.as_ref() else {
            return;
        };
        if attempt.day != today(self.clock.now_ms()) {
            // A previous day's receipt cannot prove today's check-in.
            // Today's status cannot resolve an older write with an uncertain outcome.
            row.claim_state = ClaimState::Unconfirmed;
            row.credited = None;
            return;
        }
        let (result, credited) = checkin::reconcile(
            attempt.receipt,
            state,
            attempt.before,
            credits.total_remaining,
            attempt.expected,
        );
        if result == ClaimState::Unconfirmed && state == ClaimState::NeedsVerification {
            // A failed authorization read does not resolve the prior submission.
            row.claim_state = ClaimState::NeedsVerification;
            row.credited = None;
            return;
        }
        row.claim_state = result;
        row.credited = credited;
        if matches!(
            result,
            ClaimState::Claimed
                | ClaimState::AlreadyClaimed
                | ClaimState::Unavailable
                | ClaimState::NeedsVerification
        ) {
            row.pending = None;
        }
    }
    async fn refresh_read(
        &self,
        b: &VaultMetadata,
        epoch: u64,
        snapshot: &SavedAccount,
        endpoint: Endpoint,
    ) -> Result<serde_json::Value, Failure> {
        let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
        let mut pending = None;
        let mut ready = None;
        {
            let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
            self.check_epoch(b, epoch)?;
            let rows = store::load(self.session, b)?;
            matched(&rows, b, snapshot)?;
            let _vault = self
                .session
                .read()
                .map_err(|_| Failure::StorageUnavailable)?;
            if self.session.lifecycle_epoch() != epoch {
                return Err(Failure::ExpiredAuthorization);
            }
            // Reuse dispatch_claim's final admission boundary: creation and first
            // poll are protected, but a pending HTTP read never holds the owner.
            let mut future = self.transport.request(
                endpoint,
                Some(&snapshot.account),
                None,
                self.clock.now_ms(),
            );
            match future
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker))
            {
                std::task::Poll::Ready(result) => ready = Some(result),
                std::task::Poll::Pending => pending = Some(future),
            }
        }
        let result = if let Some(result) = ready {
            result
        } else {
            pending.ok_or(Failure::Unconfirmed)?.await
        };
        Ok(result.unwrap_or_else(|error| match error {
            Failure::NeedsVerification => serde_json::json!({"code":401}),
            _ => serde_json::json!({}),
        }))
    }
    pub async fn refresh(&self, id: &str) -> Result<AccountView, Failure> {
        let _lease = lease(self.session, id)?;
        let (b, epoch, snapshot, read_day) = {
            let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
            let b = store::binding(self.session)?;
            let row = store::load(self.session, &b)?
                .into_iter()
                .find(|r| r.account.id(&b.vault_id) == id)
                .ok_or(Failure::UnsupportedContext)?;
            (
                b,
                self.session.lifecycle_epoch(),
                row,
                today(self.clock.now_ms()),
            )
        };
        // Retain the account lease, but never retain the shared owner over HTTP.
        let summary = self
            .refresh_read(&b, epoch, &snapshot, Endpoint::Summary)
            .await?;
        let paid = self
            .refresh_read(&b, epoch, &snapshot, Endpoint::Paid)
            .await?;
        let free = self
            .refresh_read(&b, epoch, &snapshot, Endpoint::Free)
            .await?;
        let credits = if read_day == today(self.clock.now_ms()) {
            credits::normalize(&summary, &paid, &free, self.clock.now_ms())
        } else {
            Credits::default()
        };
        let status_day = today(self.clock.now_ms());
        let response = self
            .refresh_read(&b, epoch, &snapshot, Endpoint::Status)
            .await?;
        let status = if status_day == today(self.clock.now_ms()) {
            checkin::status(&response)
        } else {
            ClaimState::Unconfirmed
        };
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        self.check_epoch(&b, epoch)?;
        let mut rows = store::load(self.session, &b)?;
        let i = matched(&rows, &b, &snapshot)?;
        let day = today(self.clock.now_ms());
        if read_day != day {
            // The final owner wait can cross midnight after status() returned.
            // Keep the latest row intact instead of relabeling yesterday's read.
            return Ok(rows[i].view(&b.vault_id, &day));
        }
        if rows[i].pending.is_some() {
            self.resolve_pending(&mut rows[i], status, &credits);
        } else if rows[i].claim_day != day
            || !(rows[i].claim_state == ClaimState::Claimed && status == ClaimState::AlreadyClaimed)
        {
            rows[i].claim_state = status;
            rows[i].credited = None;
        }
        if credits.updated_at.is_some() {
            rows[i].credits = credits;
            rows[i].credits.observed_generation = Some(rows[i].generation);
        }
        rows[i].claim_day = day;
        self.persist(&b, rows, i)
    }
    pub async fn refresh_all(&self) -> Result<Vec<AccountView>, Failure> {
        let rows = self.list().await?;
        for row in rows {
            match self.refresh(&row.id).await {
                Ok(_) | Err(Failure::Busy) => {}
                Err(e) => return Err(e),
            }
        }
        self.list().await
    }
    pub async fn claim_all(&self) -> Result<Vec<ClaimResult>, Failure> {
        // Reuse the lease owner; account IDs are hashes and cannot collide with this key.
        let _batch = lease(self.session, "claim-all")?;
        let operation_day = today(self.clock.now_ms());
        let accounts = self.list().await?;
        let mut results = Vec::new();
        for account in accounts.into_iter().filter(|row| row.can_claim) {
            let result = if today(self.clock.now_ms()) == operation_day {
                self.claim(&account.id).await
            } else {
                Err(Failure::Unconfirmed)
            };
            let (account, error) = match result {
                Ok(updated) => (updated, None),
                Err(error) => (account, Some(error)),
            };
            results.push(ClaimResult { account, error });
        }
        Ok(results)
    }
    pub async fn claim(&self, id: &str) -> Result<AccountView, Failure> {
        self.claim_with_policy(id, None).await
    }
    async fn claim_with_policy(
        &self,
        id: &str,
        policy: Option<(u64, u64)>,
    ) -> Result<AccountView, Failure> {
        let operation_day = today(self.clock.now_ms());
        let _lease = lease(self.session, id)?;
        let (b, epoch, snapshot) = {
            let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
            let b = store::binding(self.session)?;
            let rows = store::load(self.session, &b)?;
            let row = rows
                .into_iter()
                .find(|r| r.account.id(&b.vault_id) == id)
                .ok_or(Failure::UnsupportedContext)?;
            automatic_admission(&row, policy)?;
            (b, self.session.lifecycle_epoch(), row)
        };
        // Automatic runs never query/replay an unresolved write. Manual refresh can
        // reconcile a known same-day receipt, but cannot reset an unknown outcome.
        if policy.is_some()
            && (snapshot.pending.is_some()
                || snapshot
                    .attempted_day
                    .as_deref()
                    .is_some_and(|d| d >= operation_day.as_str()))
        {
            return Ok(snapshot.view(&b.vault_id, &operation_day));
        }
        let status = self.status(&b, &snapshot.account).await;
        let credits = if snapshot.pending.is_some() || status == ClaimState::Available {
            self.credits(&b, &snapshot.account).await
        } else {
            Credits::default()
        };
        let (prepared, previous_day) = {
            let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
            self.check_epoch(&b, epoch)?;
            let mut rows = store::load(self.session, &b)?;
            let i = matched(&rows, &b, &snapshot)?;
            automatic_admission(&rows[i], policy)?;
            rows[i].claim_day = today(self.clock.now_ms());
            rows[i].credited = None;
            if credits.updated_at.is_some() {
                rows[i].credits = credits.clone();
                rows[i].credits.observed_generation = Some(rows[i].generation);
            }
            if rows[i].pending.is_some() {
                self.resolve_pending(&mut rows[i], status, &credits);
                return self.persist(&b, rows, i);
            }
            rows[i].claim_state = status;
            if status != ClaimState::Available
                || rows[i]
                    .attempted_day
                    .as_deref()
                    .is_some_and(|d| d >= operation_day.as_str())
            {
                return self.persist(&b, rows, i);
            }
            if credits.total_remaining.is_none() || today(self.clock.now_ms()) != operation_day {
                rows[i].claim_state = ClaimState::Unconfirmed;
                return self.persist(&b, rows, i);
            }
            let previous_day = rows[i].attempted_day.clone();
            rows[i].attempted_day = Some(operation_day.clone());
            rows[i].pending = Some(Attempt {
                day: operation_day.clone(),
                before: credits.total_remaining,
                expected: None,
                receipt: ClaimState::Unconfirmed,
            });
            let prepared = rows[i].clone();
            store::save(self.session, &b, rows)?;
            (prepared, previous_day)
        };
        #[cfg(test)]
        super::engine_tests::before_claim_dispatch(self.session).await;
        let response = self
            .dispatch_claim(&b, epoch, &prepared, policy, &operation_day)
            .await;
        let receipt = match response {
            Err(DispatchFailure::NotDispatched(error)) => {
                // Clear only this exact unsent intent, never a later identity or attempt.
                let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
                let mut rows = store::load(self.session, &b)?;
                let i = matched(&rows, &b, &prepared)?;
                if rows[i].pending == prepared.pending {
                    rows[i].pending = None;
                    rows[i].attempted_day = previous_day;
                    store::save(self.session, &b, rows)?;
                }
                return Err(error);
            }
            Err(DispatchFailure::OutcomeUnknown(_)) => serde_json::json!({}),
            Ok(value) => value,
        };
        {
            let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
            self.check_epoch(&b, epoch)?;
            let mut rows = store::load(self.session, &b)?;
            let i = matched(&rows, &b, &prepared)?;
            if rows[i].pending != prepared.pending {
                return Err(Failure::ExpiredAuthorization);
            }
            if let Some(attempt) = rows[i].pending.as_mut() {
                attempt.receipt = checkin::receipt(&receipt);
                attempt.expected = checkin::receipt_credit(&receipt);
            }
            store::save(self.session, &b, rows)?;
        }
        let after = self.status(&b, &snapshot.account).await;
        let credits = self.credits(&b, &snapshot.account).await;
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        self.check_epoch(&b, epoch)?;
        let mut rows = store::load(self.session, &b)?;
        let i = matched(&rows, &b, &prepared)?;
        if today(self.clock.now_ms()) == operation_day {
            self.resolve_pending(&mut rows[i], after, &credits);
        } else {
            rows[i].claim_day = today(self.clock.now_ms());
            rows[i].claim_state = ClaimState::Unconfirmed;
            rows[i].credited = None;
        }
        if credits.updated_at.is_some() {
            rows[i].credits = credits;
            rows[i].credits.observed_generation = Some(rows[i].generation);
        }
        self.persist(&b, rows, i)
    }
    fn check_epoch(&self, b: &VaultMetadata, epoch: u64) -> Result<(), Failure> {
        if store::binding(self.session)? != *b || self.session.lifecycle_epoch() != epoch {
            return Err(Failure::ExpiredAuthorization);
        }
        Ok(())
    }
    async fn dispatch_claim(
        &self,
        b: &VaultMetadata,
        epoch: u64,
        prepared: &SavedAccount,
        policy: Option<(u64, u64)>,
        day: &str,
    ) -> Result<serde_json::Value, DispatchFailure> {
        use DispatchFailure::NotDispatched;
        let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
        let mut pending = None;
        let mut ready = None;
        {
            let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
            self.check_epoch(b, epoch).map_err(NotDispatched)?;
            let rows = store::load(self.session, b).map_err(NotDispatched)?;
            let i = matched(&rows, b, prepared).map_err(NotDispatched)?;
            automatic_admission(&rows[i], policy).map_err(NotDispatched)?;
            if rows[i].pending != prepared.pending || today(self.clock.now_ms()) != day {
                return Err(NotDispatched(Failure::ExpiredAuthorization));
            }
            let _vault = self
                .session
                .read()
                .map_err(|_| NotDispatched(Failure::StorageUnavailable))?;
            if self.session.lifecycle_epoch() != epoch {
                return Err(NotDispatched(Failure::ExpiredAuthorization));
            }
            // Final admission includes creation AND first poll, as in the existing
            // inference owner; never hold a vault/owner guard over network waits.
            let mut future = self.transport.claim(&prepared.account, self.clock.now_ms());
            match future
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker))
            {
                std::task::Poll::Ready(result) => ready = Some(result),
                std::task::Poll::Pending => pending = Some(future),
            }
        }
        if let Some(result) = ready {
            result
        } else {
            pending.ok_or(NotDispatched(Failure::Unconfirmed))?.await
        }
    }
}
fn matched(
    rows: &[SavedAccount],
    b: &VaultMetadata,
    expected: &SavedAccount,
) -> Result<usize, Failure> {
    rows.iter()
        .position(|r| {
            r.account.id(&b.vault_id) == expected.account.id(&b.vault_id)
                && r.generation == expected.generation
                && r.account.token == expected.account.token
        })
        .ok_or(Failure::ExpiredAuthorization)
}
fn automatic_admission(row: &SavedAccount, policy: Option<(u64, u64)>) -> Result<(), Failure> {
    if policy.is_some_and(|(generation, revision)| {
        !row.automatic_enabled()
            || !row.can_set_automatic()
            || row.generation != generation
            || row.auto_claim_revision != revision
    }) {
        return Err(Failure::ExpiredAuthorization);
    }
    Ok(())
}
#[path = "automatic.rs"]
mod automatic;
