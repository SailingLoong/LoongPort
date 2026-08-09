use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
};

use futures::future::{AbortHandle, Abortable};
use tauri::Emitter;
use tokio::sync::mpsc;

use crate::{
    database::Database,
    relay::model_verification::passive::{EvidenceBatch, VerificationIngress},
    relay::model_verification::types::{
        RunFailureKind, RunState, StartRunResponse, TargetKey, TargetScope,
        VerificationProgressEvent, VerificationReport,
    },
};

pub type VerificationFuture =
    Pin<Box<dyn Future<Output = Result<VerificationReport, RunFailureKind>> + Send + 'static>>;
pub type ProbeProgress = Arc<dyn Fn(u8) + Send + Sync + 'static>;

pub struct PreparedVerification {
    pub total_checks: u8,
    pub future: VerificationFuture,
}

pub trait ActiveVerifier: Send + Sync {
    fn prepare(
        &self,
        target: TargetKey,
        progress: ProbeProgress,
    ) -> Result<PreparedVerification, RunFailureKind>;
}

pub trait VerificationEventSink: Send + Sync {
    fn attach_app_handle(&self, _app_handle: tauri::AppHandle) {}
    fn emit_progress(&self, event: &VerificationProgressEvent) -> Result<(), ()>;
    fn emit_changed(&self, scope: &TargetScope) -> Result<(), ()>;
}

pub(crate) struct NoopEventSink;

impl VerificationEventSink for NoopEventSink {
    fn emit_progress(&self, _event: &VerificationProgressEvent) -> Result<(), ()> {
        Ok(())
    }

    fn emit_changed(&self, _scope: &TargetScope) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Default)]
struct TauriEventSink {
    app_handle: RwLock<Option<tauri::AppHandle>>,
}

impl VerificationEventSink for TauriEventSink {
    fn attach_app_handle(&self, app_handle: tauri::AppHandle) {
        *self
            .app_handle
            .write()
            .expect("model verification app handle lock poisoned") = Some(app_handle);
    }

    fn emit_progress(&self, event: &VerificationProgressEvent) -> Result<(), ()> {
        let app_handle = self
            .app_handle
            .read()
            .expect("model verification app handle lock poisoned")
            .clone();
        app_handle.map_or(Ok(()), |app_handle| {
            app_handle
                .emit(crate::events::MODEL_VERIFICATION_PROGRESS, event)
                .map_err(|_| ())
        })
    }

    fn emit_changed(&self, scope: &TargetScope) -> Result<(), ()> {
        let app_handle = self
            .app_handle
            .read()
            .expect("model verification app handle lock poisoned")
            .clone();
        app_handle.map_or(Ok(()), |app_handle| {
            app_handle
                .emit(crate::events::MODEL_VERIFICATION_CHANGED, scope)
                .map_err(|_| ())
        })
    }
}

pub struct ModelVerificationCoordinator {
    db: Arc<Database>,
    verifier: Arc<dyn ActiveVerifier>,
    event_sink: Arc<dyn VerificationEventSink>,
    mutation: Mutex<()>,
    state: Mutex<CoordinatorState>,
    passive_receiver: Mutex<Option<mpsc::Receiver<EvidenceBatch>>>,
    passive_ingress: VerificationIngress,
}

#[derive(Default)]
struct CoordinatorState {
    active: HashMap<TargetKey, ActiveRun>,
    generations: HashMap<TargetScope, u64>,
}

struct ActiveRun {
    run_id: String,
    generation: u64,
    abort_handle: AbortHandle,
    completed_checks: u8,
    total_checks: u8,
}

impl ModelVerificationCoordinator {
    pub fn new(db: Arc<Database>) -> Self {
        let verifier = Arc::new(
            crate::relay::model_verification::active::BalancedActiveVerifier::new(db.clone()),
        );
        Self::with_dependencies(db, verifier, Arc::new(TauriEventSink::default()))
    }

    pub fn with_verifier(db: Arc<Database>, verifier: Arc<dyn ActiveVerifier>) -> Self {
        Self::with_dependencies(db, verifier, Arc::new(NoopEventSink))
    }

    pub(crate) fn with_dependencies(
        db: Arc<Database>,
        verifier: Arc<dyn ActiveVerifier>,
        event_sink: Arc<dyn VerificationEventSink>,
    ) -> Self {
        let (passive_ingress, passive_receiver) = VerificationIngress::channel();
        Self {
            db,
            verifier,
            event_sink,
            mutation: Mutex::new(()),
            state: Mutex::new(CoordinatorState::default()),
            passive_receiver: Mutex::new(Some(passive_receiver)),
            passive_ingress,
        }
    }

    pub(crate) fn with_passive_ingress(
        db: Arc<Database>,
        verifier: Arc<dyn ActiveVerifier>,
        event_sink: Arc<dyn VerificationEventSink>,
        passive_ingress: VerificationIngress,
        passive_receiver: mpsc::Receiver<EvidenceBatch>,
    ) -> Self {
        Self {
            db,
            verifier,
            event_sink,
            mutation: Mutex::new(()),
            state: Mutex::new(CoordinatorState::default()),
            passive_receiver: Mutex::new(Some(passive_receiver)),
            passive_ingress,
        }
    }

    pub fn passive_ingress(&self) -> VerificationIngress {
        self.passive_ingress.clone()
    }

    pub fn take_passive_receiver(&self) -> Option<mpsc::Receiver<EvidenceBatch>> {
        self.passive_receiver
            .lock()
            .expect("model verification passive receiver lock poisoned")
            .take()
    }

    pub fn attach_app_handle(&self, app_handle: tauri::AppHandle) {
        self.event_sink.attach_app_handle(app_handle);
    }

    pub async fn start(
        self: &Arc<Self>,
        target: TargetKey,
    ) -> Result<StartRunResponse, RunFailureKind> {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        let mut state = self
            .state
            .lock()
            .expect("model verification state mutex poisoned");
        if let Some(existing) = state.active.get(&target) {
            return Ok(StartRunResponse {
                run_id: existing.run_id.clone(),
                state: RunState::Running,
            });
        }

        let run_id = uuid::Uuid::new_v4().to_string();
        let scope = scope_for(&target);
        let generation = state.generations.get(&scope).copied().unwrap_or_default();
        let weak_coordinator = Arc::downgrade(self);
        let progress_target = target.clone();
        let progress_run_id = run_id.clone();
        let progress: ProbeProgress = Arc::new(move |completed_checks| {
            if let Some(coordinator) = weak_coordinator.upgrade() {
                coordinator.report_probe_progress(
                    &progress_target,
                    &progress_run_id,
                    generation,
                    completed_checks,
                );
            }
        });
        let prepared = self.verifier.prepare(target.clone(), progress)?;
        if prepared.total_checks == 0 {
            return Err(RunFailureKind::InvalidResponse);
        }
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        state.active.insert(
            target.clone(),
            ActiveRun {
                run_id: run_id.clone(),
                generation,
                abort_handle,
                completed_checks: 0,
                total_checks: prepared.total_checks,
            },
        );
        drop(state);

        self.emit_progress(progress_event(
            &target,
            &run_id,
            RunState::Running,
            0,
            prepared.total_checks,
            None,
        ));
        drop(_mutation);

        let coordinator = Arc::clone(self);
        let spawned_run_id = run_id.clone();
        let spawned_target = target.clone();
        tauri::async_runtime::spawn(async move {
            let result = Abortable::new(prepared.future, abort_registration).await;
            coordinator.finish(spawned_target, spawned_run_id, generation, result);
        });

        Ok(StartRunResponse {
            run_id,
            state: RunState::Running,
        })
    }

    pub fn list_results(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<VerificationReport>, RunFailureKind> {
        crate::relay::model_verification::store::list_for_provider_ids(&self.db, provider_ids)
            .map_err(|_| RunFailureKind::InvalidResponse)
    }

    pub fn cancel(&self, run_id: &str) -> Result<(), RunFailureKind> {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        let cancelled = {
            let mut state = self
                .state
                .lock()
                .expect("model verification state mutex poisoned");
            let target = state
                .active
                .iter()
                .find_map(|(target, run)| (run.run_id == run_id).then(|| target.clone()));
            target.and_then(|target| state.active.remove(&target).map(|run| (target, run)))
        };
        if let Some((target, run)) = cancelled {
            run.abort_handle.abort();
            self.emit_progress(progress_event(
                &target,
                &run.run_id,
                RunState::Cancelled,
                run.completed_checks,
                run.total_checks,
                Some(RunFailureKind::Cancelled),
            ));
        }
        drop(_mutation);
        Ok(())
    }

    pub fn cancel_scope(&self, scope: &TargetScope) {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        let cancelled = {
            let mut state = self
                .state
                .lock()
                .expect("model verification state mutex poisoned");
            remove_scope_runs(&mut state, scope)
        };
        for (target, run) in cancelled {
            run.abort_handle.abort();
            self.emit_progress(progress_event(
                &target,
                &run.run_id,
                RunState::Cancelled,
                run.completed_checks,
                run.total_checks,
                Some(RunFailureKind::Cancelled),
            ));
        }
        drop(_mutation);
    }

    pub fn clear_scope(&self, scope: &TargetScope) -> Result<(), RunFailureKind> {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        crate::relay::model_verification::store::clear_scope(&self.db, scope)
            .map_err(|_| RunFailureKind::InvalidResponse)?;

        let cancelled = {
            let mut state = self
                .state
                .lock()
                .expect("model verification state mutex poisoned");
            let generation = state.generations.entry(scope.clone()).or_default();
            *generation = generation.saturating_add(1);
            remove_scope_runs(&mut state, scope)
        };
        for (target, run) in cancelled {
            run.abort_handle.abort();
            self.emit_progress(progress_event(
                &target,
                &run.run_id,
                RunState::Cancelled,
                run.completed_checks,
                run.total_checks,
                Some(RunFailureKind::Cancelled),
            ));
        }
        self.emit_changed(scope);
        drop(_mutation);
        Ok(())
    }

    fn finish(
        &self,
        target: TargetKey,
        run_id: String,
        generation: u64,
        result: Result<Result<VerificationReport, RunFailureKind>, futures::future::Aborted>,
    ) {
        match result {
            Ok(Ok(report)) if report.target == target => {
                let _ = self.persist_if_current_with(&target, &run_id, generation, || {
                    crate::relay::model_verification::store::upsert_active(&self.db, &report)
                        .map_err(|_| RunFailureKind::InvalidResponse)
                });
            }
            Ok(Err(failure)) => {
                self.remove_if_current_and_emit(
                    &target,
                    &run_id,
                    generation,
                    RunState::Failed,
                    failure,
                );
            }
            Ok(Ok(_)) => {
                self.remove_if_current_and_emit(
                    &target,
                    &run_id,
                    generation,
                    RunState::Failed,
                    RunFailureKind::InvalidResponse,
                );
            }
            Err(_) => {
                self.remove_if_current_and_emit(
                    &target,
                    &run_id,
                    generation,
                    RunState::Cancelled,
                    RunFailureKind::Cancelled,
                );
            }
        }
    }

    fn remove_if_current_and_emit(
        &self,
        target: &TargetKey,
        run_id: &str,
        generation: u64,
        run_state: RunState,
        failure: RunFailureKind,
    ) {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        let mut state = self
            .state
            .lock()
            .expect("model verification state mutex poisoned");
        if run_is_current(&state, target, run_id, generation) {
            let run = state
                .active
                .remove(target)
                .expect("current model verification run must exist");
            drop(state);
            self.emit_progress(progress_event(
                target,
                run_id,
                run_state,
                run.completed_checks,
                run.total_checks,
                Some(failure),
            ));
        }
    }

    fn persist_if_current_with(
        &self,
        target: &TargetKey,
        run_id: &str,
        generation: u64,
        persist: impl FnOnce() -> Result<(), RunFailureKind>,
    ) -> Result<bool, RunFailureKind> {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        {
            let state = self
                .state
                .lock()
                .expect("model verification state mutex poisoned");
            if !run_is_current(&state, target, run_id, generation) {
                return Ok(false);
            }
        }

        let persisted = persist();
        let mut state = self
            .state
            .lock()
            .expect("model verification state mutex poisoned");
        let run = run_is_current(&state, target, run_id, generation)
            .then(|| state.active.remove(target))
            .flatten();
        drop(state);

        let Some(run) = run else {
            return Ok(false);
        };

        match persisted {
            Ok(()) => {
                self.emit_progress(progress_event(
                    target,
                    run_id,
                    RunState::Completed,
                    run.completed_checks,
                    run.total_checks,
                    None,
                ));
                self.emit_changed(&scope_for(target));
                Ok(true)
            }
            Err(failure) => {
                self.emit_progress(progress_event(
                    target,
                    run_id,
                    RunState::Failed,
                    run.completed_checks,
                    run.total_checks,
                    Some(failure),
                ));
                Err(failure)
            }
        }
    }

    fn report_probe_progress(
        &self,
        target: &TargetKey,
        run_id: &str,
        generation: u64,
        completed_checks: u8,
    ) {
        let _mutation = self
            .mutation
            .lock()
            .expect("model verification mutation mutex poisoned");
        let mut state = self
            .state
            .lock()
            .expect("model verification state mutex poisoned");
        if !run_is_current(&state, target, run_id, generation) {
            return;
        }
        let run = state
            .active
            .get_mut(target)
            .expect("current model verification run must exist");
        if completed_checks <= run.completed_checks || completed_checks > run.total_checks {
            return;
        }
        run.completed_checks = completed_checks;
        let total_checks = run.total_checks;
        drop(state);
        self.emit_progress(progress_event(
            target,
            run_id,
            RunState::Running,
            completed_checks,
            total_checks,
            None,
        ));
    }

    fn emit_progress(&self, event: VerificationProgressEvent) {
        if self.event_sink.emit_progress(&event).is_err() {
            log::warn!("模型验证进度事件发送失败");
        }
    }

    fn emit_changed(&self, scope: &TargetScope) {
        if self.event_sink.emit_changed(scope).is_err() {
            log::warn!("模型验证结果变化事件发送失败");
        }
    }
}

fn progress_event(
    target: &TargetKey,
    run_id: &str,
    state: RunState,
    completed_checks: u8,
    total_checks: u8,
    failure: Option<RunFailureKind>,
) -> VerificationProgressEvent {
    VerificationProgressEvent {
        run_id: run_id.to_string(),
        provider_id: target.provider_id.clone(),
        app_type: target.app_type.clone(),
        model: target.model.clone(),
        state,
        completed_checks,
        total_checks,
        failure,
    }
}

fn scope_for(target: &TargetKey) -> TargetScope {
    TargetScope::new(target.provider_id.clone(), target.app_type.clone())
}

fn run_is_current(
    state: &CoordinatorState,
    target: &TargetKey,
    run_id: &str,
    generation: u64,
) -> bool {
    state
        .generations
        .get(&scope_for(target))
        .copied()
        .unwrap_or_default()
        == generation
        && state
            .active
            .get(target)
            .is_some_and(|run| run.run_id == run_id && run.generation == generation)
}

fn remove_scope_runs(
    state: &mut CoordinatorState,
    scope: &TargetScope,
) -> Vec<(TargetKey, ActiveRun)> {
    let matching: Vec<_> = state
        .active
        .keys()
        .filter(|target| scope_for(target) == *scope)
        .cloned()
        .collect();
    matching
        .into_iter()
        .filter_map(|target| state.active.remove(&target).map(|run| (target, run)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    use futures::channel::oneshot;

    use crate::{
        database::Database,
        relay::model_verification::types::{
            EvidenceLevel, RunFailureKind, TargetKey, TargetScope, Verdict, VerificationReport,
            RULES_VERSION,
        },
    };

    use super::{
        ActiveVerifier, ModelVerificationCoordinator, PreparedVerification, ProbeProgress,
        VerificationEventSink,
    };

    #[derive(Default)]
    struct RecordingSink {
        progress: Mutex<Vec<crate::relay::model_verification::types::VerificationProgressEvent>>,
        changed: Mutex<Vec<TargetScope>>,
    }

    impl VerificationEventSink for RecordingSink {
        fn emit_progress(
            &self,
            event: &crate::relay::model_verification::types::VerificationProgressEvent,
        ) -> Result<(), ()> {
            self.progress.lock().unwrap().push(event.clone());
            Ok(())
        }

        fn emit_changed(&self, scope: &TargetScope) -> Result<(), ()> {
            self.changed.lock().unwrap().push(scope.clone());
            Ok(())
        }
    }

    struct BlockingProgressSink {
        blocked_state: crate::relay::model_verification::types::RunState,
        entered: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl VerificationEventSink for BlockingProgressSink {
        fn emit_progress(
            &self,
            event: &crate::relay::model_verification::types::VerificationProgressEvent,
        ) -> Result<(), ()> {
            if event.state == self.blocked_state {
                if let Some(entered) = self.entered.lock().unwrap().take() {
                    entered.send(()).unwrap();
                    self.release.lock().unwrap().recv().unwrap();
                }
            }
            Ok(())
        }

        fn emit_changed(&self, _scope: &TargetScope) -> Result<(), ()> {
            Ok(())
        }
    }

    fn blocking_sink(
        blocked_state: crate::relay::model_verification::types::RunState,
    ) -> (
        Arc<BlockingProgressSink>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    ) {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        (
            Arc::new(BlockingProgressSink {
                blocked_state,
                entered: Mutex::new(Some(entered_tx)),
                release: Mutex::new(release_rx),
            }),
            entered_rx,
            release_tx,
        )
    }

    type BlockedCompletion = (
        oneshot::Sender<Result<VerificationReport, RunFailureKind>>,
        ProbeProgress,
    );

    struct BlockedVerifier {
        calls: AtomicUsize,
        senders: Mutex<HashMap<TargetKey, BlockedCompletion>>,
    }

    impl BlockedVerifier {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                senders: Mutex::new(HashMap::new()),
            }
        }

        fn complete(
            &self,
            target: &TargetKey,
            result: Result<VerificationReport, RunFailureKind>,
        ) -> bool {
            self.senders
                .lock()
                .unwrap()
                .remove(target)
                .is_some_and(|(sender, progress)| {
                    if result.is_ok() {
                        for completed in 1..=3 {
                            progress(completed);
                        }
                    }
                    sender.send(result).is_ok()
                })
        }

        fn advance(&self, target: &TargetKey, completed_checks: u8) -> bool {
            self.senders
                .lock()
                .unwrap()
                .get(target)
                .is_some_and(|(_, progress)| {
                    progress(completed_checks);
                    true
                })
        }
    }

    impl ActiveVerifier for BlockedVerifier {
        fn prepare(
            &self,
            target: TargetKey,
            progress: ProbeProgress,
        ) -> Result<PreparedVerification, RunFailureKind> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (sender, receiver) = oneshot::channel();
            self.senders
                .lock()
                .unwrap()
                .insert(target, (sender, progress));
            let future: Pin<
                Box<
                    dyn Future<Output = Result<VerificationReport, RunFailureKind>>
                        + Send
                        + 'static,
                >,
            > = Box::pin(async move { receiver.await.unwrap() });
            Ok(PreparedVerification {
                total_checks: 3,
                future,
            })
        }
    }

    fn target() -> TargetKey {
        TargetKey::new("provider-a", "codex", "gpt-5.6-sol")
    }

    fn database_with_providers(providers: &[(&str, &str)]) -> Arc<Database> {
        let db = Database::memory().unwrap();
        {
            let conn = db.conn.lock().unwrap();
            for (provider_id, app_type) in providers {
                conn.execute(
                    "INSERT INTO providers (id, app_type, name, settings_config) VALUES (?1, ?2, ?1, '{}')",
                    rusqlite::params![provider_id, app_type],
                )
                .unwrap();
            }
        }
        Arc::new(db)
    }

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("condition was not reached");
    }

    #[tokio::test]
    async fn duplicate_start_reuses_run_and_issues_one_probe() {
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            Arc::new(Database::memory().unwrap()),
            verifier.clone(),
        ));

        let first = coordinator.start(target()).await.unwrap();
        let duplicate = coordinator.start(target()).await.unwrap();

        assert_eq!(duplicate.run_id, first.run_id);
        assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_targets_complete_and_persist_independently() {
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            database_with_providers(&[("provider-a", "codex"), ("provider-b", "codex")]),
            verifier.clone(),
        ));
        let first_target = target();
        let second_target = TargetKey::new("provider-b", "codex", "gpt-5.4");

        coordinator.start(first_target.clone()).await.unwrap();
        coordinator.start(second_target.clone()).await.unwrap();
        assert!(verifier.complete(&second_target, Ok(completed_report(second_target.clone()))));
        wait_until(|| {
            coordinator
                .list_results(&["provider-b".into()])
                .is_ok_and(|rows| rows.len() == 1)
        })
        .await;
        assert!(coordinator
            .list_results(&["provider-a".into()])
            .unwrap()
            .is_empty());

        assert!(verifier.complete(&first_target, Ok(completed_report(first_target.clone()))));
        wait_until(|| {
            coordinator
                .list_results(&["provider-a".into(), "provider-b".into()])
                .is_ok_and(|rows| rows.len() == 2)
        })
        .await;
    }

    #[tokio::test]
    async fn every_core_failure_leaves_an_existing_completed_report_unchanged() {
        for failure in [
            RunFailureKind::Authentication,
            RunFailureKind::RateLimited,
            RunFailureKind::InsufficientBalance,
            RunFailureKind::Network,
            RunFailureKind::Upstream,
            RunFailureKind::Timeout,
            RunFailureKind::ModelUnavailable,
            RunFailureKind::InvalidResponse,
        ] {
            let db = database_with_providers(&[("provider-a", "codex")]);
            let prior = VerificationReport {
                verdict: Verdict::Suspicious,
                ..completed_report(target())
            };
            crate::relay::model_verification::store::upsert_active(&db, &prior).unwrap();
            let verifier = Arc::new(BlockedVerifier::new());
            let sink = Arc::new(RecordingSink::default());
            let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
                db,
                verifier.clone(),
                sink.clone(),
            ));

            let run = coordinator.start(target()).await.unwrap();
            assert!(verifier.complete(&target(), Err(failure)));
            wait_until(|| {
                sink.progress.lock().unwrap().iter().any(|event| {
                    event.run_id == run.run_id
                        && event.state == crate::relay::model_verification::types::RunState::Failed
                        && event.failure == Some(failure)
                })
            })
            .await;

            let rows = coordinator.list_results(&["provider-a".into()]).unwrap();
            assert_eq!(rows, vec![prior], "failure {failure:?}");
        }
    }

    #[tokio::test]
    async fn cancellation_never_persists_and_preserves_the_prior_report() {
        let db = database_with_providers(&[("provider-a", "codex")]);
        let prior = VerificationReport {
            verdict: Verdict::Suspicious,
            ..completed_report(target())
        };
        crate::relay::model_verification::store::upsert_active(&db, &prior).unwrap();
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            db,
            verifier.clone(),
        ));

        let run = coordinator.start(target()).await.unwrap();
        coordinator.cancel(&run.run_id).unwrap();
        verifier.complete(
            &target(),
            Ok(VerificationReport {
                verdict: Verdict::Trusted,
                ..completed_report(target())
            }),
        );
        tokio::task::yield_now().await;

        let rows = coordinator.list_results(&["provider-a".into()]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].verdict, Verdict::Suspicious);
    }

    #[tokio::test]
    async fn cancel_scope_stops_only_matching_runs_without_clearing_results_or_generation() {
        let db = database_with_providers(&[("provider-a", "codex"), ("provider-b", "codex")]);
        let prior = VerificationReport {
            verdict: Verdict::Suspicious,
            ..completed_report(target())
        };
        crate::relay::model_verification::store::upsert_active(&db, &prior).unwrap();
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            db,
            verifier.clone(),
        ));
        let matching = target();
        let other = TargetKey::new("provider-b", "codex", "gpt-5.4");

        coordinator.start(matching.clone()).await.unwrap();
        let generation_before = coordinator
            .state
            .lock()
            .unwrap()
            .generations
            .get(&TargetScope::new("provider-a", "codex"))
            .copied();
        coordinator.start(other.clone()).await.unwrap();

        coordinator.cancel_scope(&TargetScope::new("provider-a", "codex"));

        assert_eq!(
            coordinator
                .state
                .lock()
                .unwrap()
                .generations
                .get(&TargetScope::new("provider-a", "codex"))
                .copied(),
            generation_before,
            "cancel must not advance the reset generation"
        );
        assert_eq!(
            coordinator
                .list_results(&["provider-a".into()])
                .unwrap()
                .len(),
            1,
            "cancel must preserve the prior report"
        );
        assert!(verifier.complete(&other, Ok(completed_report(other.clone()))));
        wait_until(|| {
            coordinator
                .list_results(&["provider-b".into()])
                .is_ok_and(|rows| rows.len() == 1)
        })
        .await;
        assert!(!verifier.complete(&matching, Ok(completed_report(matching.clone()))));
    }

    #[tokio::test]
    async fn cancel_after_finish_and_unknown_run_are_idempotent() {
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            database_with_providers(&[("provider-a", "codex")]),
            verifier.clone(),
        ));

        let run = coordinator.start(target()).await.unwrap();
        assert!(verifier.complete(&target(), Ok(completed_report(target()))));
        wait_until(|| {
            coordinator
                .list_results(&["provider-a".into()])
                .is_ok_and(|rows| rows.len() == 1)
        })
        .await;

        coordinator.cancel(&run.run_id).unwrap();
        coordinator.cancel("unknown-run").unwrap();
    }

    #[tokio::test]
    async fn clear_scope_cancels_old_generation_and_rejects_late_completion() {
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            database_with_providers(&[("provider-a", "codex")]),
            verifier.clone(),
        ));

        coordinator.start(target()).await.unwrap();
        coordinator
            .clear_scope(&TargetScope::new("provider-a", "codex"))
            .unwrap();
        let _ = verifier.complete(&target(), Ok(completed_report(target())));
        tokio::task::yield_now().await;
        assert!(coordinator
            .list_results(&["provider-a".into()])
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn generation_check_and_upsert_are_atomic_against_clear() {
        let db = database_with_providers(&[("provider-a", "codex")]);
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            db.clone(),
            verifier,
        ));
        let run = coordinator.start(target()).await.unwrap();
        let generation = coordinator
            .state
            .lock()
            .unwrap()
            .active
            .get(&target())
            .unwrap()
            .generation;
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let persist_coordinator = coordinator.clone();
        let persist_db = db.clone();
        let persist_target = target();
        let persist_run_id = run.run_id.clone();
        let persist = tokio::task::spawn_blocking(move || {
            let report = completed_report(persist_target.clone());
            persist_coordinator.persist_if_current_with(
                &persist_target,
                &persist_run_id,
                generation,
                || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    crate::relay::model_verification::store::upsert_active(&persist_db, &report)
                        .map_err(|_| RunFailureKind::InvalidResponse)
                },
            )
        });
        tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
            .await
            .unwrap();

        let clear_coordinator = coordinator.clone();
        let mut clear = tokio::task::spawn_blocking(move || {
            clear_coordinator.clear_scope(&TargetScope::new("provider-a", "codex"))
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut clear)
                .await
                .is_err()
        );

        release_tx.send(()).unwrap();
        assert_eq!(persist.await.unwrap(), Ok(true));
        clear.await.unwrap().unwrap();
        assert!(coordinator
            .list_results(&["provider-a".into()])
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn old_completion_cannot_remove_or_overwrite_a_newer_run() {
        let verifier = Arc::new(BlockedVerifier::new());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_verifier(
            database_with_providers(&[("provider-a", "codex")]),
            verifier.clone(),
        ));
        let old = coordinator.start(target()).await.unwrap();
        let old_generation = coordinator
            .state
            .lock()
            .unwrap()
            .active
            .get(&target())
            .unwrap()
            .generation;
        coordinator.cancel(&old.run_id).unwrap();
        let newer = coordinator.start(target()).await.unwrap();

        coordinator.finish(
            target(),
            old.run_id,
            old_generation,
            Ok(Ok(VerificationReport {
                verdict: Verdict::Anomaly,
                ..completed_report(target())
            })),
        );

        let duplicate = coordinator.start(target()).await.unwrap();
        assert_eq!(duplicate.run_id, newer.run_id);
        assert_eq!(verifier.calls.load(Ordering::SeqCst), 2);
        assert!(coordinator
            .list_results(&["provider-a".into()])
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn progress_and_changed_events_are_finite_and_follow_successful_persistence() {
        let verifier = Arc::new(BlockedVerifier::new());
        let sink = Arc::new(RecordingSink::default());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
            database_with_providers(&[("provider-a", "codex")]),
            verifier.clone(),
            sink.clone(),
        ));

        let run = coordinator.start(target()).await.unwrap();
        assert!(verifier.complete(&target(), Ok(completed_report(target()))));
        wait_until(|| sink.changed.lock().unwrap().len() == 1).await;

        let progress = sink.progress.lock().unwrap().clone();
        assert_eq!(progress.len(), 5);
        assert_eq!(progress[0].run_id, run.run_id);
        assert_eq!(progress[0].completed_checks, 0);
        assert_eq!(progress[0].total_checks, 3);
        assert_eq!(
            progress[0].state,
            crate::relay::model_verification::types::RunState::Running
        );
        assert_eq!(
            progress[4].state,
            crate::relay::model_verification::types::RunState::Completed
        );
        assert_eq!(progress[4].completed_checks, 3);
        assert_eq!(progress[4].total_checks, 3);
        assert_eq!(
            progress
                .iter()
                .map(|event| event.completed_checks)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 3]
        );
        assert_eq!(
            sink.changed.lock().unwrap().as_slice(),
            [TargetScope::new("provider-a", "codex")]
        );

        let serialized = serde_json::to_string(&progress).unwrap();
        for sentinel in [
            "SENTINEL_URL",
            "SENTINEL_KEY",
            "SENTINEL_PROMPT",
            "SENTINEL_OUTPUT",
            "SENTINEL_THINKING",
            "SENTINEL_SIGNATURE",
        ] {
            assert!(!serialized.contains(sentinel));
        }
    }

    #[tokio::test]
    async fn failure_and_cancellation_emit_finite_terminal_events_without_changed() {
        let verifier = Arc::new(BlockedVerifier::new());
        let sink = Arc::new(RecordingSink::default());
        let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
            database_with_providers(&[("provider-a", "codex"), ("provider-b", "codex")]),
            verifier.clone(),
            sink.clone(),
        ));
        let failed_target = target();
        let cancelled_target = TargetKey::new("provider-b", "codex", "gpt-5.4");

        coordinator.start(failed_target.clone()).await.unwrap();
        let cancelled = coordinator.start(cancelled_target.clone()).await.unwrap();
        assert!(verifier.advance(&failed_target, 1));
        assert!(verifier.advance(&cancelled_target, 2));
        assert!(verifier.complete(&failed_target, Err(RunFailureKind::Authentication)));
        coordinator.cancel(&cancelled.run_id).unwrap();
        wait_until(|| {
            sink.progress
                .lock()
                .unwrap()
                .iter()
                .filter(|event| {
                    matches!(
                        event.state,
                        crate::relay::model_verification::types::RunState::Failed
                            | crate::relay::model_verification::types::RunState::Cancelled
                    )
                })
                .count()
                == 2
        })
        .await;

        let terminal: Vec<_> = sink
            .progress
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(
                    event.state,
                    crate::relay::model_verification::types::RunState::Failed
                        | crate::relay::model_verification::types::RunState::Cancelled
                )
            })
            .cloned()
            .collect();
        assert_eq!(terminal.len(), 2);
        assert!(terminal.iter().any(|event| {
            event.state == crate::relay::model_verification::types::RunState::Failed
                && event.failure == Some(RunFailureKind::Authentication)
                && event.completed_checks == 1
                && event.total_checks == 3
        }));
        assert!(terminal.iter().any(|event| {
            event.state == crate::relay::model_verification::types::RunState::Cancelled
                && event.failure == Some(RunFailureKind::Cancelled)
                && event.completed_checks == 2
                && event.total_checks == 3
        }));
        assert!(sink.changed.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn restart_waits_until_the_cancel_transition_is_published() {
        let verifier = Arc::new(BlockedVerifier::new());
        let (sink, entered, release) =
            blocking_sink(crate::relay::model_verification::types::RunState::Cancelled);
        let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
            Arc::new(Database::memory().unwrap()),
            verifier,
            sink,
        ));
        let run = coordinator.start(target()).await.unwrap();

        let cancel_coordinator = coordinator.clone();
        let cancel = tokio::task::spawn_blocking(move || cancel_coordinator.cancel(&run.run_id));
        tokio::task::spawn_blocking(move || entered.recv().unwrap())
            .await
            .unwrap();

        let restart_coordinator = coordinator.clone();
        let mut restart = tokio::spawn(async move { restart_coordinator.start(target()).await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut restart)
                .await
                .is_err()
        );

        release.send(()).unwrap();
        cancel.await.unwrap().unwrap();
        restart.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_cannot_overtake_running_progress() {
        let verifier = Arc::new(BlockedVerifier::new());
        let (sink, entered, release) =
            blocking_sink(crate::relay::model_verification::types::RunState::Running);
        let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
            Arc::new(Database::memory().unwrap()),
            verifier,
            sink,
        ));

        let start_coordinator = coordinator.clone();
        let start = tokio::spawn(async move { start_coordinator.start(target()).await });
        tokio::task::spawn_blocking(move || entered.recv().unwrap())
            .await
            .unwrap();

        let clear_coordinator = coordinator.clone();
        let mut clear = tokio::task::spawn_blocking(move || {
            clear_coordinator.clear_scope(&TargetScope::new("provider-a", "codex"))
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut clear)
                .await
                .is_err()
        );

        release.send(()).unwrap();
        start.await.unwrap().unwrap();
        clear.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_cannot_overtake_completed_progress() {
        let verifier = Arc::new(BlockedVerifier::new());
        let (sink, entered, release) =
            blocking_sink(crate::relay::model_verification::types::RunState::Completed);
        let coordinator = Arc::new(ModelVerificationCoordinator::with_dependencies(
            database_with_providers(&[("provider-a", "codex")]),
            verifier.clone(),
            sink,
        ));

        coordinator.start(target()).await.unwrap();
        assert!(verifier.complete(&target(), Ok(completed_report(target()))));
        tokio::task::spawn_blocking(move || entered.recv().unwrap())
            .await
            .unwrap();

        let clear_coordinator = coordinator.clone();
        let mut clear = tokio::task::spawn_blocking(move || {
            clear_coordinator.clear_scope(&TargetScope::new("provider-a", "codex"))
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut clear)
                .await
                .is_err()
        );

        release.send(()).unwrap();
        clear.await.unwrap().unwrap();
    }

    #[allow(dead_code)]
    fn completed_report(target: TargetKey) -> VerificationReport {
        VerificationReport {
            target,
            verdict: Verdict::Trusted,
            evidence_level: EvidenceLevel::ProtocolBehavior,
            facts: Vec::new(),
            rules_version: RULES_VERSION,
            checked_at: 1_700_000_000,
        }
    }
}
