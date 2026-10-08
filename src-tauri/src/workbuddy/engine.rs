//! Explicit operations share the existing sync/vault lifecycle owner.
use super::{
    checkin::{self, ClaimState},
    credits::{self, Credits},
    model::*,
    protocol::{Endpoint, Transport},
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
    fn resolve_pending(
        &self,
        row: &mut SavedAccount,
        state: ClaimState,
        credits: &Credits,
        allow_reset: bool,
    ) {
        let Some(attempt) = row.pending.as_ref() else {
            return;
        };
        if attempt.day != today(self.clock.now_ms()) {
            // A previous day's receipt cannot prove today's check-in.
            row.pending = None;
            row.claim_state = state;
            row.credited = None;
            return;
        }
        if allow_reset
            && attempt.receipt == ClaimState::Unconfirmed
            && state == ClaimState::Available
            && credits.total_remaining.is_some()
        {
            row.pending = None;
            row.claim_state = ClaimState::Available;
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
    pub async fn refresh(&self, id: &str) -> Result<AccountView, Failure> {
        let _lease = lease(self.session, id)?;
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let b = store::binding(self.session)?;
        let mut rows = store::load(self.session, &b)?;
        let i = rows
            .iter()
            .position(|r| r.account.id(&b.vault_id) == id)
            .ok_or(Failure::UnsupportedContext)?;
        let credits = self.credits(&b, &rows[i].account).await;
        let status = self.status(&b, &rows[i].account).await;
        if rows[i].pending.is_some() {
            self.resolve_pending(&mut rows[i], status, &credits, true);
        } else if rows[i].claim_day != today(self.clock.now_ms())
            || !(rows[i].claim_state == ClaimState::Claimed && status == ClaimState::AlreadyClaimed)
        {
            rows[i].claim_state = status;
            rows[i].credited = None;
        }
        if credits.updated_at.is_some() {
            rows[i].credits = credits;
        }
        rows[i].claim_day = today(self.clock.now_ms());
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
    pub async fn claim(&self, id: &str) -> Result<AccountView, Failure> {
        let operation_day = today(self.clock.now_ms());
        let _lease = lease(self.session, id)?;
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let b = store::binding(self.session)?;
        let mut rows = store::load(self.session, &b)?;
        let i = rows
            .iter()
            .position(|r| r.account.id(&b.vault_id) == id)
            .ok_or(Failure::UnsupportedContext)?;
        let status = self.status(&b, &rows[i].account).await;
        rows[i].claim_day = today(self.clock.now_ms());
        rows[i].credited = None;
        if rows[i].pending.is_some() {
            // An explicit retry first reconciles the durable pending attempt and never
            // submits within that same operation, including after process restart.
            let credits = self.credits(&b, &rows[i].account).await;
            self.resolve_pending(&mut rows[i], status, &credits, true);
            if credits.updated_at.is_some() {
                rows[i].credits = credits;
            }
            return self.persist(&b, rows, i);
        }
        rows[i].claim_state = status;
        if status != ClaimState::Available {
            return self.persist(&b, rows, i);
        }
        let before = self.credits(&b, &rows[i].account).await;
        // Without a complete baseline, do not submit a write we cannot verify.
        if before.total_remaining.is_none() || today(self.clock.now_ms()) != operation_day {
            rows[i].claim_state = ClaimState::Unconfirmed;
            return self.persist(&b, rows, i);
        }
        rows[i].credits = before.clone();
        rows[i].pending = Some(Attempt {
            day: today(self.clock.now_ms()),
            before: before.total_remaining,
            expected: None,
            receipt: ClaimState::Unconfirmed,
        });
        // Persist before dispatch: a lost response or cancellation is recoverable.
        store::save(self.session, &b, rows.clone())?;
        if today(self.clock.now_ms()) != operation_day {
            rows[i].pending = None;
            rows[i].claim_day = today(self.clock.now_ms());
            rows[i].claim_state = ClaimState::Unconfirmed;
            return self.persist(&b, rows, i);
        }
        let receipt = self.request(&b, Endpoint::Claim, &rows[i].account).await;
        if let Some(attempt) = rows[i].pending.as_mut() {
            attempt.receipt = checkin::receipt(&receipt);
            attempt.expected = checkin::receipt_credit(&receipt);
        }
        store::save(self.session, &b, rows.clone())?;
        let after = self.status(&b, &rows[i].account).await;
        let credits = self.credits(&b, &rows[i].account).await;
        if today(self.clock.now_ms()) == operation_day {
            self.resolve_pending(&mut rows[i], after, &credits, false);
        } else {
            rows[i].claim_day = today(self.clock.now_ms());
            rows[i].claim_state = ClaimState::Unconfirmed;
            rows[i].credited = None;
        }
        if credits.updated_at.is_some() {
            rows[i].credits = credits;
        }
        self.persist(&b, rows, i)
    }
}
