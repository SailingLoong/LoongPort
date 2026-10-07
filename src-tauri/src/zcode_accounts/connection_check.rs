//! Bounded ownership for an explicit single-account status read.
//! Only non-secret request identities and cancellation state live here.
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Reading,
    Committing,
    Finished,
}
struct Entry {
    generation: u64,
    scope: Option<String>,
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    phase: Phase,
    owned: bool,
    deadline: Instant,
    expires: Instant,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<String, Entry>,
    next: u64,
}
#[derive(Clone, Default)]
pub(crate) struct Requests(Arc<Mutex<State>>);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckError {
    Invalid,
    Known,
    Busy,
    Cancelled,
    Changed,
    Poisoned,
}
pub(crate) struct Lease {
    owner: Requests,
    id: String,
    generation: u64,
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    deadline: Instant,
}
fn valid(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}
fn prune(state: &mut State, now: Instant) {
    for entry in state.entries.values_mut() {
        if entry.phase == Phase::Reading && now >= entry.deadline {
            entry.cancelled.store(true, Ordering::Release);
            entry.notify.notify_one();
        }
    }
    state
        .entries
        .retain(|_, entry| entry.owned || now < entry.expires);
}
impl Requests {
    pub(crate) fn begin(&self, id: String, now: Instant) -> Result<Lease, CheckError> {
        if !valid(&id) {
            return Err(CheckError::Invalid);
        }
        let mut state = self.0.lock().map_err(|_| CheckError::Poisoned)?;
        prune(&mut state, now);
        if state.entries.contains_key(&id) {
            return Err(CheckError::Known);
        }
        if state.entries.len() >= 512
            || state.entries.values().filter(|entry| entry.owned).count() >= 2
        {
            return Err(CheckError::Busy);
        }
        state.next = state.next.checked_add(1).ok_or(CheckError::Busy)?;
        let generation = state.next;
        let cancelled = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(tokio::sync::Notify::new());
        let deadline = now + Duration::from_secs(75);
        state.entries.insert(
            id.clone(),
            Entry {
                generation,
                scope: None,
                cancelled: cancelled.clone(),
                notify: notify.clone(),
                phase: Phase::Reading,
                owned: true,
                deadline,
                expires: now + Duration::from_secs(600),
            },
        );
        Ok(Lease {
            owner: self.clone(),
            id,
            generation,
            cancelled,
            notify,
            deadline,
        })
    }
    pub(crate) fn cancel(&self, id: &str, now: Instant) -> Result<&'static str, CheckError> {
        if !valid(id) {
            return Err(CheckError::Invalid);
        }
        let mut state = self.0.lock().map_err(|_| CheckError::Poisoned)?;
        prune(&mut state, now);
        if let Some(entry) = state.entries.get_mut(id) {
            if entry.phase != Phase::Reading {
                return Ok("tooLate");
            }
            entry.cancelled.store(true, Ordering::Release);
            entry.notify.notify_one();
            return Ok("cancelled");
        }
        if state.entries.len() >= 512 {
            return Err(CheckError::Busy);
        }
        state.entries.insert(
            id.into(),
            Entry {
                generation: 0,
                scope: None,
                cancelled: Arc::new(AtomicBool::new(true)),
                notify: Arc::new(tokio::sync::Notify::new()),
                phase: Phase::Reading,
                owned: false,
                deadline: now,
                expires: now + Duration::from_secs(600),
            },
        );
        Ok("cancelled")
    }
}
impl Lease {
    pub(crate) fn check(&self) -> Result<(), CheckError> {
        if self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            Err(CheckError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(crate) fn bind(&self, scope: String) -> Result<(), CheckError> {
        self.check()?;
        let mut state = self.owner.0.lock().map_err(|_| CheckError::Poisoned)?;
        if state.entries.iter().any(|(id, entry)| {
            id != &self.id && entry.owned && entry.scope.as_ref() == Some(&scope)
        }) {
            return Err(CheckError::Busy);
        }
        let entry = state.entries.get_mut(&self.id).ok_or(CheckError::Changed)?;
        if entry.generation != self.generation
            || entry.scope.as_ref().is_some_and(|old| old != &scope)
        {
            return Err(CheckError::Changed);
        }
        entry.scope = Some(scope);
        Ok(())
    }
    pub(crate) async fn cancelled(&self) {
        if self.check().is_err() {
            return;
        }
        tokio::select! { _=self.notify.notified()=>{}, _=tokio::time::sleep_until(self.deadline.into())=>{} }
    }
    pub(crate) fn admit_commit(&self) -> Result<(), CheckError> {
        let mut state = self.owner.0.lock().map_err(|_| CheckError::Poisoned)?;
        let entry = state.entries.get_mut(&self.id).ok_or(CheckError::Changed)?;
        if entry.generation != self.generation || entry.phase != Phase::Reading || !entry.owned {
            return Err(CheckError::Changed);
        }
        self.check()?;
        entry.phase = Phase::Committing;
        Ok(())
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.0.lock() {
            if let Some(entry) = state.entries.get_mut(&self.id) {
                if entry.generation == self.generation {
                    entry.owned = false;
                    entry.phase = Phase::Finished;
                    entry.expires = Instant::now() + Duration::from_secs(600);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn early_cancel_and_cancellation_before_commit_never_admit_a_late_save() {
        let requests = Requests::default();
        let now = Instant::now();
        let early = uuid::Uuid::new_v4().to_string();
        assert_eq!(requests.cancel(&early, now).unwrap(), "cancelled");
        assert!(matches!(requests.begin(early, now), Err(CheckError::Known)));
        let id = uuid::Uuid::new_v4().to_string();
        let lease = requests.begin(id.clone(), now).unwrap();
        lease.bind("same-account".into()).unwrap();
        assert_eq!(requests.cancel(&id, now).unwrap(), "cancelled");
        lease.cancelled().await;
        assert_eq!(lease.admit_commit(), Err(CheckError::Cancelled));
    }
    #[test]
    fn actual_owner_keeps_capacity_until_drop_and_committed_work_cannot_be_cancelled() {
        let requests = Requests::default();
        let now = Instant::now();
        let first = uuid::Uuid::new_v4().to_string();
        let a = requests.begin(first.clone(), now).unwrap();
        a.bind("account".into()).unwrap();
        let b = requests
            .begin(uuid::Uuid::new_v4().to_string(), now)
            .unwrap();
        assert_eq!(b.bind("account".into()), Err(CheckError::Busy));
        requests.cancel(&first, now).unwrap();
        assert!(matches!(
            requests.begin(uuid::Uuid::new_v4().to_string(), now),
            Err(CheckError::Busy)
        ));
        drop(a);
        let id = uuid::Uuid::new_v4().to_string();
        let c = requests.begin(id.clone(), now).unwrap();
        c.admit_commit().unwrap();
        assert_eq!(requests.cancel(&id, now).unwrap(), "tooLate");
        drop(c);
        assert!(matches!(requests.begin(id, now), Err(CheckError::Known)));
    }
    #[test]
    fn unpolled_request_drop_releases_only_its_physical_owner() {
        let requests = Requests::default();
        let now = Instant::now();
        let a = requests
            .begin(uuid::Uuid::new_v4().to_string(), now)
            .unwrap();
        let b = requests
            .begin(uuid::Uuid::new_v4().to_string(), now)
            .unwrap();
        drop(a);
        let c = requests
            .begin(uuid::Uuid::new_v4().to_string(), now)
            .unwrap();
        assert!(b.check().is_ok());
        assert!(c.check().is_ok());
    }
}
