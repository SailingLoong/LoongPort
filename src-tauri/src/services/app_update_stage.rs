//! Pre-download ownership. Independent of Tauri so the real state machine can
//! be exercised with local fixture artifacts and no installer/network effects.

use std::{
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};
use tokio::sync::oneshot;

#[derive(Debug, PartialEq, Eq)]
pub enum PrestageAction {
    DropStaged,
    Keep,
    Start,
    Nothing,
}

pub struct ReadyUpdate<T> {
    pub update: T,
    pub installer: PathBuf,
}

pub struct DownloadTicket {
    pub id: u64,
    pub cancel: oneshot::Receiver<()>,
    pub done: Option<oneshot::Sender<Result<(), String>>>,
}

pub struct DownloadWait {
    pub id: u64,
    pub done: oneshot::Receiver<Result<(), String>>,
}

enum Stage<T> {
    Idle,
    Downloading {
        id: u64,
        version: String,
        _cancel: oneshot::Sender<()>,
        done: Option<oneshot::Receiver<Result<(), String>>>,
    },
    Ready {
        version: String,
        ready: Box<ReadyUpdate<T>>,
    },
}

struct State<T> {
    stage: Stage<T>,
    next: u64,
    check: u64,
    foreground: Option<u64>,
    progress_request: Option<String>,
    channel: Option<bool>,
    known_version: Option<String>,
}

impl<T> State<T> {
    fn next_id(&mut self) -> u64 {
        self.next = self
            .next
            .checked_add(1)
            .expect("update attempt counter exhausted");
        self.next
    }
}

pub struct UpdateStage<T> {
    inner: Mutex<State<T>>,
    /// Installing the same application twice is never a useful fallback.
    pub install: tokio::sync::Mutex<()>,
    pub download: tokio::sync::Mutex<()>,
}

impl<T> Default for UpdateStage<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> UpdateStage<T> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(State {
                stage: Stage::Idle,
                next: 0,
                check: 0,
                foreground: None,
                progress_request: None,
                channel: None,
                known_version: None,
            }),
            install: tokio::sync::Mutex::new(()),
            download: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn next_id(&self) -> u64 {
        self.lock().next_id()
    }

    pub fn align_channel(&self, beta: bool) {
        let mut inner = self.lock();
        if inner.channel.is_some_and(|previous| previous != beta) {
            if let Stage::Ready { ready, .. } = &inner.stage {
                let _ = std::fs::remove_file(&ready.installer);
            }
            inner.stage = Stage::Idle;
            inner.known_version = None;
            inner.check = inner.next_id();
        }
        inner.channel = Some(beta);
    }

    pub fn minimum_version(&self) -> Option<String> {
        self.lock().known_version.clone()
    }

    pub fn remember_version(&self, version: &str) -> Result<(), String> {
        Self::remember_locked(&mut self.lock(), version)
    }

    fn remember_locked(inner: &mut State<T>, version: &str) -> Result<(), String> {
        let next = semver::Version::parse(version).map_err(|_| "更新版本格式无效".to_string())?;
        if inner
            .known_version
            .as_deref()
            .and_then(|known| semver::Version::parse(known).ok())
            .is_some_and(|known| known > next)
        {
            return Err("清单版本低于已发现的更新".into());
        }
        inner.known_version = Some(version.to_string());
        Ok(())
    }

    pub fn remember_for_check(&self, check: u64, version: Option<&str>) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.check != check {
            return Err("更新检查已被较新的检查取代".into());
        }
        match version {
            Some(version) => Self::remember_locked(&mut inner, version),
            None if inner.known_version.is_some() => {
                Err("无法确认已有更新，保留现有版本信息".into())
            }
            None => Ok(()),
        }
    }

    pub fn begin_check(&self) -> u64 {
        let mut inner = self.lock();
        inner.check = inner.next_id();
        inner.check
    }

    #[cfg(test)]
    pub fn is_current_check(&self, id: u64) -> bool {
        self.lock().check == id
    }

    #[cfg(test)]
    pub fn staged_version(&self) -> Option<String> {
        match &self.lock().stage {
            Stage::Idle => None,
            Stage::Downloading { version, .. } | Stage::Ready { version, .. } => {
                Some(version.clone())
            }
        }
    }

    #[cfg(test)]
    pub fn begin_if_needed(
        &self,
        available: Option<&str>,
    ) -> (PrestageAction, Option<DownloadTicket>) {
        let mut inner = self.lock();
        Self::begin_locked(&mut inner, available)
    }

    pub fn begin_for_check(
        &self,
        check: u64,
        available: Option<&str>,
    ) -> Result<(PrestageAction, Option<DownloadTicket>), String> {
        let mut inner = self.lock();
        if inner.check != check {
            return Err("更新检查已被较新的检查取代".into());
        }
        if let (Some(known), Some(available)) = (&inner.known_version, available) {
            if matches!((semver::Version::parse(known), semver::Version::parse(available)), (Ok(known), Ok(available)) if known > available)
            {
                return Err("清单版本低于已发现的更新，保留已有更新".into());
            }
        }
        if available.is_none()
            && (inner.known_version.is_some() || !matches!(inner.stage, Stage::Idle))
        {
            return Err("清单未确认已有更新，保留预下载产物并重试".into());
        }
        Ok(Self::begin_locked(&mut inner, available))
    }

    fn begin_locked(
        inner: &mut State<T>,
        available: Option<&str>,
    ) -> (PrestageAction, Option<DownloadTicket>) {
        if let (Some(known), Some(available)) = (&inner.known_version, available) {
            if matches!((semver::Version::parse(known), semver::Version::parse(available)), (Ok(known), Ok(available)) if known > available)
            {
                return (PrestageAction::Keep, None);
            }
        }
        let current = match &inner.stage {
            Stage::Idle => None,
            Stage::Downloading { version, .. } | Stage::Ready { version, .. } => {
                Some(version.as_str())
            }
        };
        let action = match (current, available) {
            (None, None) => PrestageAction::Nothing,
            (Some(_), None) => PrestageAction::DropStaged,
            (Some(current), Some(next))
                if current == next
                    || matches!(
                        (semver::Version::parse(current), semver::Version::parse(next)),
                        (Ok(current), Ok(next)) if current > next
                    ) =>
            {
                PrestageAction::Keep
            }
            _ => PrestageAction::Start,
        };
        match action {
            PrestageAction::DropStaged | PrestageAction::Start => {
                if let Stage::Ready { ready, .. } = &inner.stage {
                    let _ = std::fs::remove_file(&ready.installer);
                }
                // Dropping a Downloading slot closes its cancellation sender.
                inner.stage = Stage::Idle;
                if action == PrestageAction::DropStaged {
                    return (action, None);
                }
                let id = inner.next_id();
                inner.known_version = available.map(str::to_owned);
                let (done_tx, done_rx) = oneshot::channel();
                let (cancel_tx, cancel_rx) = oneshot::channel();
                inner.stage = Stage::Downloading {
                    id,
                    version: available
                        .expect("Start has an available version")
                        .to_string(),
                    _cancel: cancel_tx,
                    done: Some(done_rx),
                };
                (
                    action,
                    Some(DownloadTicket {
                        id,
                        cancel: cancel_rx,
                        done: Some(done_tx),
                    }),
                )
            }
            _ => (action, None),
        }
    }

    /// Publish a unique temporary file only while this exact attempt owns the
    /// slot. The rename and publication share the lock, including same-version
    /// retries; a late old task may remove only its own temporary file.
    pub fn set_ready(
        &self,
        id: u64,
        mut ready: ReadyUpdate<T>,
        final_path: PathBuf,
    ) -> Result<bool, String> {
        let mut inner = self.lock();
        let version = match &inner.stage {
            Stage::Downloading {
                id: current,
                version,
                ..
            } if *current == id => version.clone(),
            _ => {
                let _ = std::fs::remove_file(&ready.installer);
                return Ok(false);
            }
        };
        std::fs::rename(&ready.installer, &final_path)
            .map_err(|e| format!("发布预下载更新失败: {e}"))?;
        ready.installer = final_path;
        inner.stage = Stage::Ready {
            version,
            ready: Box::new(ready),
        };
        Ok(true)
    }

    pub fn set_failed(&self, id: u64) {
        let mut inner = self.lock();
        if matches!(&inner.stage, Stage::Downloading { id: current, .. } if *current == id) {
            inner.stage = Stage::Idle;
        }
    }

    pub fn take_ready(&self) -> Option<Box<ReadyUpdate<T>>> {
        let mut inner = self.lock();
        match std::mem::replace(&mut inner.stage, Stage::Idle) {
            Stage::Ready { ready, .. } => Some(ready),
            other => {
                inner.stage = other;
                None
            }
        }
    }

    pub fn take_downloading_done(&self) -> Option<DownloadWait> {
        let mut inner = self.lock();
        match &mut inner.stage {
            Stage::Downloading { id, done, .. } => {
                done.take().map(|done| DownloadWait { id: *id, done })
            }
            _ => None,
        }
    }

    pub fn foreground(&self, id: u64) -> Foreground<'_, T> {
        self.lock().foreground = Some(id);
        Foreground { stage: self, id }
    }

    #[cfg(test)]
    pub fn owns_progress(&self, id: u64) -> bool {
        self.lock().foreground == Some(id)
    }

    pub fn set_progress_request(&self, request: Option<String>) {
        self.lock().progress_request = request;
    }

    pub fn progress_request(&self, id: u64) -> Option<Option<String>> {
        let inner = self.lock();
        (inner.foreground == Some(id)).then(|| inner.progress_request.clone())
    }
}

pub struct Foreground<'a, T> {
    stage: &'a UpdateStage<T>,
    id: u64,
}
impl<T> Drop for Foreground<'_, T> {
    fn drop(&mut self) {
        let mut inner = self.stage.lock();
        if inner.foreground == Some(self.id) {
            inner.foreground = None;
            inner.progress_request = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "loongport-update-stage-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn artifact(&self, id: u64, text: &str) -> ReadyUpdate<String> {
            let installer = self.0.join(format!("{id}.partial"));
            std::fs::write(&installer, text).unwrap();
            ReadyUpdate {
                update: text.into(),
                installer,
            }
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn same_version_keeps_one_task() {
        let stage = UpdateStage::<String>::new();
        let (action, first) = stage.begin_if_needed(Some("1.0.0"));
        assert_eq!(action, PrestageAction::Start);
        assert!(first.is_some());
        let (action, duplicate) = stage.begin_if_needed(Some("1.0.0"));
        assert_eq!(action, PrestageAction::Keep);
        assert!(duplicate.is_none());
    }

    #[test]
    fn stale_available_version_cannot_replace_a_newer_staged_download() {
        let stage = UpdateStage::<String>::new();
        let _ticket = stage.begin_if_needed(Some("6.27.0-beta.4")).1.unwrap();
        let (action, stale) = stage.begin_if_needed(Some("6.27.0-beta.2"));
        assert_eq!(action, PrestageAction::Keep);
        assert!(stale.is_none());
        assert_eq!(stage.staged_version().as_deref(), Some("6.27.0-beta.4"));
    }

    #[tokio::test]
    async fn superseded_same_version_task_cannot_publish_or_remove_the_new_artifact() {
        let stage = UpdateStage::<String>::new();
        let dir = TestDir::new();
        let old = stage.begin_if_needed(Some("1.0.0")).1.unwrap();
        stage.begin_if_needed(None);
        let new = stage.begin_if_needed(Some("1.0.0")).1.unwrap();
        assert_ne!(old.id, new.id);
        assert!(old.cancel.await.is_err());
        let final_path = dir.0.join("1.0.0.installer");
        assert!(!stage
            .set_ready(old.id, dir.artifact(old.id, "old"), final_path.clone())
            .unwrap());
        stage.set_failed(old.id);
        assert!(stage
            .set_ready(new.id, dir.artifact(new.id, "new"), final_path.clone())
            .unwrap());
        assert_eq!(std::fs::read_to_string(&final_path).unwrap(), "new");
        assert!(!stage
            .set_ready(old.id, dir.artifact(old.id, "late"), final_path.clone())
            .unwrap());
        assert_eq!(std::fs::read_to_string(&final_path).unwrap(), "new");
        assert_eq!(stage.take_ready().unwrap().update, "new");
    }

    #[tokio::test]
    async fn failure_releases_the_original_waiter_and_allows_retry() {
        let stage = UpdateStage::<String>::new();
        let mut ticket = stage.begin_if_needed(Some("1.0.0")).1.unwrap();
        let waiting = stage.take_downloading_done().unwrap();
        stage.set_failed(ticket.id);
        ticket
            .done
            .take()
            .unwrap()
            .send(Err("download stalled".into()))
            .unwrap();
        assert_eq!(waiting.done.await.unwrap(), Err("download stalled".into()));
        assert_eq!(
            stage.begin_if_needed(Some("1.0.0")).0,
            PrestageAction::Start
        );
    }

    #[test]
    fn only_the_current_foreground_attempt_can_emit_progress() {
        let stage = UpdateStage::<String>::new();
        let first = stage.next_id();
        let old = stage.foreground(first);
        assert!(stage.owns_progress(first));
        let second = stage.next_id();
        let _new = stage.foreground(second);
        assert!(!stage.owns_progress(first));
        drop(old);
        assert!(stage.owns_progress(second));
    }

    #[test]
    fn late_check_results_are_not_current_after_a_new_check_begins() {
        let stage = UpdateStage::<String>::new();
        let first = stage.begin_check();
        let second = stage.begin_check();
        assert!(!stage.is_current_check(first));
        assert!(stage.is_current_check(second));
    }

    #[test]
    fn download_failure_preserves_the_known_version_floor() {
        let stage = UpdateStage::<String>::new();
        stage.align_channel(true);
        let ticket = stage.begin_if_needed(Some("6.27.0-beta.4")).1.unwrap();
        stage.set_failed(ticket.id);
        assert_eq!(stage.minimum_version().as_deref(), Some("6.27.0-beta.4"));
        let check = stage.begin_check();
        assert!(stage.begin_for_check(check, Some("6.27.0-beta.2")).is_err());
    }

    #[tokio::test]
    async fn changing_channels_cancels_old_work_and_clears_its_floor() {
        let stage = UpdateStage::<String>::new();
        stage.align_channel(true);
        let ticket = stage.begin_if_needed(Some("6.27.0-beta.4")).1.unwrap();
        let check = stage.begin_check();
        stage.align_channel(false);
        assert!(ticket.cancel.await.is_err());
        assert!(stage.minimum_version().is_none());
        assert!(stage.begin_for_check(check, Some("6.27.0-beta.4")).is_err());
        assert!(stage.take_ready().is_none());
    }

    #[test]
    fn a_superseded_check_cannot_clear_or_reconcile_the_current_stage() {
        let stage = UpdateStage::<String>::new();
        let old = stage.begin_check();
        let current = stage.begin_check();
        let _ticket = stage
            .begin_for_check(current, Some("2.0.0"))
            .unwrap()
            .1
            .unwrap();
        assert!(stage.begin_for_check(old, None).is_err());
        assert_eq!(stage.staged_version().as_deref(), Some("2.0.0"));
    }

    #[test]
    fn a_disk_floor_survives_a_failed_startup_check() {
        let stage = UpdateStage::<String>::new();
        stage.align_channel(true);
        let floor = super::super::app_update_policy::staged_version_floor(
            ["6.27.0-beta.4", "6.27.0-beta.3"],
            "6.26.2",
            true,
        )
        .unwrap();
        stage.remember_version(&floor).unwrap();
        let _failed_startup_check = stage.begin_check();
        // A timeout commits no result. A later stale manifest may not lower it.
        let later = stage.begin_check();
        assert!(stage.begin_for_check(later, Some("6.27.0-beta.2")).is_err());
        assert!(stage.begin_for_check(later, None).is_err());
        assert_eq!(stage.minimum_version().as_deref(), Some("6.27.0-beta.4"));
    }

    #[test]
    fn changing_channels_discards_a_ready_beta_artifact() {
        let stage = UpdateStage::<String>::new();
        let dir = TestDir::new();
        stage.align_channel(true);
        let ticket = stage.begin_if_needed(Some("6.27.0-beta.4")).1.unwrap();
        let final_path = dir.0.join("6.27.0-beta.4.installer");
        stage
            .set_ready(
                ticket.id,
                dir.artifact(ticket.id, "beta"),
                final_path.clone(),
            )
            .unwrap();
        stage.align_channel(false);
        assert!(stage.take_ready().is_none());
        assert!(!final_path.exists());
    }
}
