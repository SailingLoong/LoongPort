use std::{future::Future, time::Duration};

use crate::error::AppError;

#[derive(Debug, Clone, Copy)]
pub struct TaskSchedule {
    pub startup_delay: Duration,
    pub interval: Duration,
    pub retry_delay: Duration,
}

impl TaskSchedule {
    pub const fn new(startup_delay: Duration, interval: Duration, retry_delay: Duration) -> Self {
        Self {
            startup_delay,
            interval,
            retry_delay,
        }
    }

    pub fn next_delay(self, succeeded: bool) -> Duration {
        if succeeded {
            self.interval
        } else {
            self.retry_delay
        }
    }
}

pub fn spawn_periodic<F, Fut>(name: &'static str, schedule: TaskSchedule, task: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), AppError>> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(schedule.startup_delay).await;
        loop {
            let succeeded = match task().await {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("background task {name} failed: {error}");
                    false
                }
            };
            tokio::time::sleep(schedule.next_delay(succeeded)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::TaskSchedule;
    use std::time::Duration;

    #[test]
    fn successful_tasks_use_the_normal_interval() {
        let schedule = TaskSchedule::new(
            Duration::from_secs(5),
            Duration::from_secs(6 * 60 * 60),
            Duration::from_secs(15 * 60),
        );

        assert_eq!(schedule.next_delay(true), schedule.interval);
    }

    #[test]
    fn failed_tasks_use_the_bounded_retry_delay() {
        let schedule = TaskSchedule::new(
            Duration::ZERO,
            Duration::from_secs(6 * 60 * 60),
            Duration::from_secs(15 * 60),
        );

        assert_eq!(schedule.next_delay(false), schedule.retry_delay);
    }
}

// BEGIN AUTOMATIC_CLAIM_RUNTIME
pub(crate) use crate::automatic_claim::{AutomaticOutcome, AutomaticReason, AutomaticResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomaticProvider {
    Workbuddy,
    MinimaxCode,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AutomaticPhase {
    Waiting,
    Checking,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutomaticStatus {
    pub provider: AutomaticProvider,
    pub phase: AutomaticPhase,
    pub last_checked_at: Option<i64>,
    pub next_check_at: Option<i64>,
    pub result: Option<AutomaticResult>,
    pub reason: Option<AutomaticReason>,
}
impl AutomaticStatus {
    fn new(provider: AutomaticProvider) -> Self {
        Self {
            provider,
            phase: AutomaticPhase::Waiting,
            last_checked_at: None,
            next_check_at: None,
            result: None,
            reason: None,
        }
    }
}
pub(crate) struct AutomaticTask {
    wake: tokio::sync::Notify,
    requested: std::sync::atomic::AtomicBool,
    status: Mutex<AutomaticStatus>,
}
impl AutomaticTask {
    fn new(provider: AutomaticProvider) -> Self {
        Self {
            wake: tokio::sync::Notify::new(),
            requested: std::sync::atomic::AtomicBool::new(false),
            status: Mutex::new(AutomaticStatus::new(provider)),
        }
    }
    fn snapshot(&self) -> AutomaticStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn wake(&self) {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_check_at = Some(automatic_now());
        if !self
            .requested
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.wake.notify_one();
        }
    }
}
#[derive(Default)]
pub(crate) struct AutomaticRegistry(Mutex<HashMap<AutomaticProvider, Arc<AutomaticTask>>>);
impl AutomaticRegistry {
    pub fn register(&self, provider: AutomaticProvider) -> Option<Arc<AutomaticTask>> {
        let mut tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if tasks.contains_key(&provider) {
            return None;
        }
        let task = Arc::new(AutomaticTask::new(provider));
        tasks.insert(provider, task.clone());
        Some(task)
    }
    pub fn wake(&self, provider: AutomaticProvider) {
        if let Some(task) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&provider)
        {
            task.wake();
        }
    }
    pub fn status(&self, provider: AutomaticProvider) -> AutomaticStatus {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&provider)
            .map(|task| task.snapshot())
            .unwrap_or_else(|| AutomaticStatus::new(provider))
    }
}
pub(crate) fn automatic_registry() -> &'static AutomaticRegistry {
    static REGISTRY: OnceLock<AutomaticRegistry> = OnceLock::new();
    REGISTRY.get_or_init(AutomaticRegistry::default)
}
fn automatic_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub(crate) async fn run_wakeable<F, Fut, O>(
    control: Arc<AutomaticTask>,
    schedule: TaskSchedule,
    task: F,
    observe: O,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = AutomaticOutcome>,
    O: Fn(AutomaticStatus),
{
    let mut delay = schedule.startup_delay;
    loop {
        {
            let mut status = control.status.lock().unwrap_or_else(|e| e.into_inner());
            status.phase = AutomaticPhase::Waiting;
            status.next_check_at = Some(
                automatic_now().saturating_add(delay.as_millis().min(i64::MAX as u128) as i64),
            );
        }
        observe(control.snapshot());
        // Notify retains one permit: rapid policy edits coalesce, including a
        // wake arriving while this worker is checking an earlier snapshot.
        tokio::select! { biased; _ = control.wake.notified() => {}, _ = tokio::time::sleep(delay) => {} }
        control
            .requested
            .store(false, std::sync::atomic::Ordering::SeqCst);
        {
            let mut status = control.status.lock().unwrap_or_else(|e| e.into_inner());
            status.phase = AutomaticPhase::Checking;
            status.next_check_at = None;
        }
        observe(control.snapshot());
        let outcome = task().await;
        {
            let mut status = control.status.lock().unwrap_or_else(|e| e.into_inner());
            status.last_checked_at = Some(automatic_now());
            status.result = Some(outcome.result);
            status.reason = outcome.reason;
        }
        delay = schedule.next_delay(outcome.result != AutomaticResult::Failed);
    }
}
// END AUTOMATIC_CLAIM_RUNTIME

#[cfg(test)]
mod automatic_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test(start_paused = true)]
    async fn registered_worker_enable_wakes_without_waiting_fifteen_minutes_and_coalesces() {
        let registry = AutomaticRegistry::default();
        let control = registry.register(AutomaticProvider::Workbuddy).unwrap();
        assert!(registry.register(AutomaticProvider::Workbuddy).is_none());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observed = observations.clone();
        let task = tokio::spawn(run_wakeable(
            control,
            TaskSchedule::new(
                Duration::from_secs(30),
                Duration::from_secs(900),
                Duration::from_secs(900),
            ),
            move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    AutomaticOutcome::completed()
                }
            },
            move |s| observed.lock().unwrap().push(s),
        ));
        tokio::task::yield_now().await;
        registry.wake(AutomaticProvider::Workbuddy);
        registry.wake(AutomaticProvider::Workbuddy);
        registry.wake(AutomaticProvider::Workbuddy);
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "enable wakes original registered worker promptly"
        );
        assert_eq!(
            registry.status(AutomaticProvider::Workbuddy).result,
            Some(AutomaticResult::Completed)
        );
        assert!(observations
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.phase == AutomaticPhase::Checking));
        tokio::time::advance(Duration::from_secs(899)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        task.abort();
    }
    #[tokio::test(start_paused = true)]
    async fn wake_during_check_is_coalesced_and_never_runs_concurrently() {
        let registry = AutomaticRegistry::default();
        let control = registry.register(AutomaticProvider::MinimaxCode).unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let signal = entered.clone();
        let gate = release.clone();
        let task = tokio::spawn(run_wakeable(
            control,
            TaskSchedule::new(
                Duration::from_secs(30),
                Duration::from_secs(900),
                Duration::from_secs(900),
            ),
            move || {
                let count = count.clone();
                let signal = signal.clone();
                let gate = gate.clone();
                async move {
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        signal.notify_one();
                        gate.notified().await;
                    }
                    AutomaticOutcome::completed()
                }
            },
            |_| {},
        ));
        registry.wake(AutomaticProvider::MinimaxCode);
        entered.notified().await;
        registry.wake(AutomaticProvider::MinimaxCode);
        registry.wake(AutomaticProvider::MinimaxCode);
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_one();
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            registry.status(AutomaticProvider::MinimaxCode).phase,
            AutomaticPhase::Waiting
        );
        task.abort();
    }
    #[tokio::test(start_paused = true)]
    async fn failures_and_locked_skips_are_observable_without_suppressing_next_wake() {
        let registry = AutomaticRegistry::default();
        let control = registry.register(AutomaticProvider::Workbuddy).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let task = tokio::spawn(run_wakeable(
            control,
            TaskSchedule::new(
                Duration::from_secs(30),
                Duration::from_secs(900),
                Duration::from_secs(900),
            ),
            move || {
                let count = count.clone();
                async move {
                    match count.fetch_add(1, Ordering::SeqCst) {
                        0 => AutomaticOutcome::skipped(AutomaticReason::Locked),
                        1 => AutomaticOutcome::failed(),
                        _ => AutomaticOutcome::completed(),
                    }
                }
            },
            |_| {},
        ));
        for (result, reason) in [
            (AutomaticResult::Skipped, Some(AutomaticReason::Locked)),
            (AutomaticResult::Failed, Some(AutomaticReason::CheckFailed)),
            (AutomaticResult::Completed, None),
        ] {
            registry.wake(AutomaticProvider::Workbuddy);
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
            let status = registry.status(AutomaticProvider::Workbuddy);
            assert_eq!(status.result, Some(result));
            assert_eq!(status.reason, reason);
            assert!(status.last_checked_at.is_some());
            assert!(status.next_check_at.is_some());
        }
        assert!(registry
            .status(AutomaticProvider::MinimaxCode)
            .last_checked_at
            .is_none());
        task.abort();
    }
    #[tokio::test(start_paused = true)]
    async fn cancelled_worker_cannot_run_queued_wakes() {
        let registry = AutomaticRegistry::default();
        let control = registry.register(AutomaticProvider::Workbuddy).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let task = tokio::spawn(run_wakeable(
            control,
            TaskSchedule::new(
                Duration::from_secs(30),
                Duration::from_secs(900),
                Duration::from_secs(900),
            ),
            move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    AutomaticOutcome::completed()
                }
            },
            |_| {},
        ));
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        registry.wake(AutomaticProvider::Workbuddy);
        tokio::time::advance(Duration::from_secs(900)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
