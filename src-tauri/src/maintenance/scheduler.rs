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
