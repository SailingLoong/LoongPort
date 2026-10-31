use super::{AccountSummary, Phase, Reason, Saved, View};
use std::time::Instant;

pub(super) struct Entry<P> {
    pub view: View,
    pub plan: Option<P>,
    pub deadline: Option<Instant>,
    pub epoch: u64,
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
    pub selection: Option<crate::app_store::isolated_scope::SelectionSnapshot>,
    expiry_task: Option<tokio::task::AbortHandle>,
}
impl<P> Entry<P> {
    pub fn preparing(flow: String, epoch: u64) -> Self {
        Self {
            view: View {
                flow_id: Some(flow),
                state: Phase::Preparing,
                available: true,
                account: None,
                saved: None,
                reason: None,
                expires_in_ms: 0,
            },
            plan: None,
            deadline: None,
            epoch,
            #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
            selection: None,
            expiry_task: None,
        }
    }
    pub fn set_expiry_task(&mut self, handle: tokio::task::AbortHandle) {
        self.stop_timer();
        self.expiry_task = Some(handle);
    }
    fn stop_timer(&mut self) {
        if let Some(handle) = self.expiry_task.take() {
            handle.abort();
        }
    }
    pub fn matches(&self, flow: &str) -> bool {
        self.view.flow_id.as_deref() == Some(flow)
    }
    pub fn expire(&mut self, now: Instant, epoch: u64) {
        // Applying owns its pins in the worker; cancellation cannot claim rollback.
        if matches!(self.view.state, Phase::Preparing | Phase::Ready)
            && (self.epoch != epoch || self.deadline.is_some_and(|end| now >= end))
        {
            self.finish(Err(Reason::SourceChanged));
        }
    }
    pub fn ready(&mut self, plan: P, account: AccountSummary, deadline: Instant) {
        if self.view.state == Phase::Preparing {
            self.plan = Some(plan);
            self.deadline = Some(deadline);
            self.view.account = Some(account);
            self.view.state = Phase::Ready;
        }
    }
    pub fn take(&mut self) -> Result<P, Reason> {
        if self.view.state != Phase::Ready {
            return Err(Reason::RequestConflict);
        }
        let plan = self.plan.take().ok_or(Reason::RequestConflict)?;
        self.stop_timer();
        self.view.state = Phase::Applying;
        self.deadline = None;
        Ok(plan)
    }
    pub fn cancel(&mut self) {
        if matches!(self.view.state, Phase::Preparing | Phase::Ready) {
            self.stop_timer();
            self.plan = None;
            self.deadline = None;
            self.view.state = Phase::Cancelled;
        }
    }
    pub fn finish(&mut self, result: Result<Saved, Reason>) {
        self.stop_timer();
        self.plan = None;
        self.deadline = None;
        match result {
            Ok(saved) => {
                self.view.saved = Some(saved);
                self.view.state = Phase::Saved;
                self.view.reason = None;
            }
            Err(reason) => {
                self.view.saved = None;
                self.view.state = Phase::Unknown;
                self.view.reason = Some(reason);
            }
        }
    }
    pub fn snapshot(&self, now: Instant) -> View {
        let mut view = self.view.clone();
        view.expires_in_ms = if view.state == Phase::Ready {
            self.deadline
                .map(|end| {
                    end.saturating_duration_since(now)
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64
                })
                .unwrap_or(0)
        } else {
            0
        };
        view
    }
}

impl<P> Drop for Entry<P> {
    fn drop(&mut self) {
        self.stop_timer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Pin(Arc<AtomicUsize>);
    impl Drop for Pin {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn ready() -> (Entry<Pin>, Arc<AtomicUsize>) {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut entry = Entry::preparing("flow".into(), 7);
        entry.ready(
            Pin(dropped.clone()),
            AccountSummary {
                id: "id".into(),
                label: "reviewed".into(),
                realm: "www.workbuddy.cn".into(),
            },
            Instant::now() + std::time::Duration::from_secs(300),
        );
        (entry, dropped)
    }
    #[test]
    fn cancellation_releases_ready_pins() {
        let (mut entry, dropped) = ready();
        entry.cancel();
        assert_eq!(entry.view.state, Phase::Cancelled);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(entry.take().is_err());
    }
    #[test]
    fn expiry_and_epoch_change_release_ready_pins() {
        for expire in [true, false] {
            let (mut entry, dropped) = ready();
            entry.expire(
                if expire {
                    Instant::now() + std::time::Duration::from_secs(301)
                } else {
                    Instant::now()
                },
                if expire { 7 } else { 8 },
            );
            assert_eq!(entry.view.state, Phase::Unknown);
            assert_eq!(entry.view.reason, Some(Reason::SourceChanged));
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }
    #[test]
    fn cancel_applying_is_not_rollback_and_completion_is_queryable() {
        let (mut entry, dropped) = ready();
        let plan = entry.take().unwrap();
        entry.cancel();
        assert_eq!(entry.view.state, Phase::Applying);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        entry.finish(Ok(Saved {
            selection: crate::workbuddy::native_source::SelectionKey {
                id: "id".into(),
                generation: 1,
                incarnation: uuid::Uuid::new_v4(),
            },
            label: "reviewed".into(),
            realm: "www.workbuddy.cn".into(),
        }));
        assert_eq!(entry.snapshot(Instant::now()).state, Phase::Saved);
        assert_eq!(entry.snapshot(Instant::now()).expires_in_ms, 0);
        assert!(entry.matches("flow"));
        assert!(!entry.matches("other"));
        drop(plan);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn cancelled_preparation_drops_late_worker_pins() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut entry = Entry::preparing("flow".into(), 7);
        entry.cancel();
        entry.ready(
            Pin(dropped.clone()),
            AccountSummary {
                id: "id".into(),
                label: "label".into(),
                realm: "www.workbuddy.cn".into(),
            },
            Instant::now(),
        );
        assert_eq!(entry.view.state, Phase::Cancelled);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn replacement_releases_ready_pins_and_unknown_does_not_claim_saved() {
        let (entry, dropped) = ready();
        let mut slot = Some(entry);
        assert_eq!(slot.as_ref().unwrap().view.state, Phase::Ready);
        slot = Some(Entry::preparing("next".into(), 7));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        let entry = slot.as_mut().unwrap();
        entry.finish(Err(Reason::Io));
        assert_eq!(entry.view.state, Phase::Unknown);
        assert!(entry.view.saved.is_none());
    }
    #[tokio::test]
    async fn leaving_ready_reclaims_its_expiry_task() {
        for action in ["cancel", "take", "finish", "drop"] {
            let (mut entry, _) = ready();
            let task = tokio::spawn(std::future::pending::<()>());
            entry.set_expiry_task(task.abort_handle());
            match action {
                "cancel" => entry.cancel(),
                "take" => {
                    drop(entry.take().unwrap());
                }
                "finish" => entry.finish(Err(Reason::Io)),
                "drop" => drop(entry),
                _ => unreachable!(),
            }
            assert!(task.await.unwrap_err().is_cancelled(), "{action}");
        }
    }
}
