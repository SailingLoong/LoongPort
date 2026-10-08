//! Serial process ownership, shared by manual and maintenance claim rounds.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
#[derive(Default, Clone)]
pub(crate) struct Jobs(Arc<Mutex<QueueState>>);
#[derive(Default)]
struct QueueState {
    active: Option<Arc<AtomicBool>>,
    stopped: bool,
}
pub(crate) struct Lease {
    owner: Jobs,
    token: Arc<AtomicBool>,
}
impl Jobs {
    pub fn begin(&self) -> Result<Lease, &'static str> {
        let mut state = self.0.lock().map_err(|_| "unavailable")?;
        if state.stopped {
            return Err("cancelled");
        }
        if state.active.is_some() {
            return Err("busy");
        }
        let token = Arc::new(AtomicBool::new(false));
        state.active = Some(token.clone());
        Ok(Lease {
            owner: self.clone(),
            token,
        })
    }
    pub fn cancel(&self) {
        if let Ok(active) = self.0.lock() {
            if let Some(token) = active.active.as_ref() {
                token.store(true, Ordering::Release);
            }
        }
    }
    pub fn shutdown(&self) {
        if let Ok(mut state) = self.0.lock() {
            // Closing admission and cancelling the lease share the same lock as begin.
            state.stopped = true;
            if let Some(token) = state.active.as_ref() {
                token.store(true, Ordering::Release);
            }
        }
    }
    pub fn busy(&self) -> bool {
        self.0.lock().map_or(true, |state| state.active.is_some())
    }
}
impl Lease {
    pub fn cancelled(&self) -> bool {
        self.token.load(Ordering::Acquire)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut active) = self.owner.0.lock() {
            if active
                .active
                .as_ref()
                .is_some_and(|token| Arc::ptr_eq(token, &self.token))
            {
                active.active = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_cancels_active_round_and_rejects_all_new_rounds() {
        let jobs = Jobs::default();
        let lease = jobs.begin().unwrap();
        jobs.shutdown();
        assert!(lease.cancelled());
        assert!(jobs.begin().is_err());
        drop(lease);
        assert!(!jobs.busy());
        assert!(jobs.begin().is_err());
    }
    #[test]
    fn shutdown_before_first_round_is_permanent_and_idempotent() {
        let jobs = Jobs::default();
        jobs.shutdown();
        jobs.shutdown();
        assert!(jobs.begin().is_err());
    }
    #[test]
    fn duplicate_click_is_rejected_until_physical_job_finishes() {
        let jobs = Jobs::default();
        let lease = jobs.begin().unwrap();
        assert!(jobs.busy());
        assert!(jobs.begin().is_err());
        jobs.cancel();
        assert!(lease.cancelled());
        assert!(jobs.begin().is_err());
        drop(lease);
        assert!(!jobs.busy());
        assert!(jobs.begin().is_ok());
    }
}
