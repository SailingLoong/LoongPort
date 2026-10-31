use super::{slot::Entry, AccountSummary, Phase, Reason, Saved, State, View};
use crate::workbuddy::{
    native_controller::{Controller, InstalledAttachPlan},
    native_source::Catalog,
    native_transaction::Failure,
};
use std::{sync::Arc, time::Instant};

pub(super) struct Prepared {
    plan: InstalledAttachPlan,
    identity: crate::secrets::storage::StorageIdentity,
    epoch: u64,
    selection: crate::app_store::isolated_scope::SelectionSnapshot,
}
fn reason(error: Failure) -> Reason {
    match error {
        Failure::UnsafePath => Reason::UnsafePath,
        Failure::Io => Reason::Io,
        Failure::Journal => Reason::Journal,
        Failure::SourceChanged => Reason::SourceChanged,
        Failure::RecoveryRequired => Reason::RecoveryRequired,
        Failure::RequestConflict => Reason::RequestConflict,
        Failure::RolledBack => Reason::RolledBack,
    }
}
fn snapshot_for(
    entry: &Entry<Prepared>,
    epoch: u64,
    selection: Option<&crate::app_store::isolated_scope::SelectionSnapshot>,
) -> View {
    if entry.epoch != epoch || selection.is_none() || entry.selection.as_ref() != selection {
        super::unknown(
            entry.view.flow_id.clone(),
            true,
            Some(Reason::SourceChanged),
        )
    } else {
        entry.snapshot(Instant::now())
    }
}
fn lock(state: &State) -> std::sync::MutexGuard<'_, Option<Entry<Prepared>>> {
    state
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
// Lives IN the worker. Panic or early return leaves a queryable unknown outcome,
// even when the renderer or awaiting IPC future has already disappeared.
struct WorkerGuard {
    state: Arc<State>,
    flow: String,
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if let Some(entry) = lock(&self.state)
            .as_mut()
            .filter(|entry| entry.matches(&self.flow))
        {
            if matches!(entry.view.state, Phase::Preparing | Phase::Applying) {
                entry.finish(Err(Reason::Io));
            }
        }
    }
}
fn schedule_expiry(
    runtime: &tokio::runtime::Handle,
    state: &Arc<State>,
    flow: String,
    deadline: Instant,
) -> tokio::task::JoinHandle<()> {
    let weak = Arc::downgrade(state);
    runtime.spawn(async move {
        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        if let Some(state) = weak.upgrade() {
            let mut slot = lock(&state);
            if let Some(entry) = slot.as_mut().filter(|entry| entry.matches(&flow)) {
                entry.expire(Instant::now(), entry.epoch);
            }
        }
    })
}
pub(super) async fn prepare(
    state: Arc<State>,
    db: Arc<crate::database::Database>,
    id: String,
) -> Result<View, Reason> {
    let flow = uuid::Uuid::new_v4().to_string();
    {
        let mut slot = lock(&state);
        if slot
            .as_ref()
            .is_some_and(|entry| entry.view.state == Phase::Applying)
        {
            return Err(Reason::RequestConflict);
        }
        *slot = Some(Entry::preparing(
            flow.clone(),
            db.secret_session().lifecycle_epoch(),
        ));
    }
    let worker_state = state.clone();
    let worker_flow = flow.clone();
    let worker_db = db.clone();
    let runtime = tokio::runtime::Handle::current();
    let task = crate::workbuddy::native_capture::schedule_capture(
        db.secret_session(),
        || {
            crate::secrets::protection::generation::capture_current_selection()
                .map_err(|_| Failure::SourceChanged)
        },
        move |identity, epoch, selection| {
            let _guard = WorkerGuard {
                state: worker_state.clone(),
                flow: worker_flow.clone(),
            };
            let result = runtime.block_on(async {
                let controller = Controller {
                    catalog: Catalog {
                        session: worker_db.secret_session(),
                        identity: &identity,
                        epoch,
                        selection: &selection,
                    },
                };
                let bundle = crate::workbuddy::native_capture::installed_bundle()?;
                controller.prepare_attach_installed(&id, &bundle).await
            });
            let mut slot = lock(&worker_state);
            if let Some(entry) = slot.as_mut().filter(|entry| entry.matches(&worker_flow)) {
                if entry.view.state == Phase::Preparing {
                    entry.selection = Some(selection.clone());
                    match result {
                        Ok(plan) => {
                            let (id, label, realm, deadline) = plan.review();
                            let account = AccountSummary {
                                id: id.into(),
                                label: label.into(),
                                realm: realm.into(),
                            };
                            entry.ready(
                                Prepared {
                                    plan,
                                    identity,
                                    epoch,
                                    selection,
                                },
                                account,
                                deadline,
                            );
                            let expiry_task = schedule_expiry(
                                &runtime,
                                &worker_state,
                                worker_flow.clone(),
                                deadline,
                            );
                            entry.set_expiry_task(expiry_task.abort_handle());
                            drop(expiry_task);
                            entry.expire(
                                Instant::now(),
                                worker_db.secret_session().lifecycle_epoch(),
                            );
                        }
                        Err(error) => entry.finish(Err(reason(error))),
                    }
                }
                snapshot_for(
                    entry,
                    worker_db.secret_session().lifecycle_epoch(),
                    crate::secrets::protection::generation::capture_current_selection()
                        .ok()
                        .as_ref(),
                )
            } else {
                super::unknown(
                    Some(worker_flow.clone()),
                    true,
                    Some(Reason::RequestConflict),
                )
            }
        },
    );
    match task {
        Ok(task) => task.await.map_err(|_| Reason::Io),
        Err(error) => {
            let error = reason(error);
            if let Some(entry) = lock(&state).as_mut().filter(|entry| entry.matches(&flow)) {
                entry.finish(Err(error));
            }
            Err(error)
        }
    }
}
pub(super) async fn confirm(
    state: Arc<State>,
    db: Arc<crate::database::Database>,
    flow: String,
) -> Result<View, Reason> {
    let (prepared, label) = {
        let mut slot = lock(&state);
        let entry = slot
            .as_mut()
            .filter(|entry| entry.matches(&flow))
            .ok_or(Reason::RequestConflict)?;
        entry.expire(Instant::now(), db.secret_session().lifecycle_epoch());
        let label = entry
            .view
            .account
            .as_ref()
            .ok_or(Reason::RequestConflict)?
            .label
            .clone();
        (entry.take()?, label)
    };
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _guard = WorkerGuard {
            state: state.clone(),
            flow: flow.clone(),
        };
        let result = runtime
            .block_on(async {
                let Prepared {
                    plan,
                    identity,
                    epoch,
                    selection,
                } = prepared;
                let controller = Controller {
                    catalog: Catalog {
                        session: db.secret_session(),
                        identity: &identity,
                        epoch,
                        selection: &selection,
                    },
                };
                let receipt = controller.confirm_attach_installed(plan).await?;
                Ok::<_, Failure>(Saved {
                    selection: receipt.selection_key()?,
                    label,
                    realm: receipt.realm,
                })
            })
            .map_err(reason);
        let mut slot = lock(&state);
        if let Some(entry) = slot.as_mut().filter(|entry| entry.matches(&flow)) {
            entry.finish(result);
            snapshot_for(
                entry,
                db.secret_session().lifecycle_epoch(),
                crate::secrets::protection::generation::capture_current_selection()
                    .ok()
                    .as_ref(),
            )
        } else {
            super::unknown(Some(flow.clone()), true, Some(Reason::RequestConflict))
        }
    })
    .await
    .map_err(|_| Reason::Io)
}
pub(super) fn inspect(
    state: &State,
    session: &crate::secrets::session::SecretSession,
    flow: Option<&str>,
) -> View {
    let mut slot = lock(state);
    if let Some(entry) = slot
        .as_mut()
        .filter(|entry| flow.is_none_or(|flow| entry.matches(flow)))
    {
        entry.expire(Instant::now(), session.lifecycle_epoch());
        snapshot_for(
            entry,
            session.lifecycle_epoch(),
            crate::secrets::protection::generation::capture_current_selection()
                .ok()
                .as_ref(),
        )
    } else {
        super::unknown(flow.map(str::to_owned), true, None)
    }
}
pub(super) fn cancel(
    state: &State,
    session: &crate::secrets::session::SecretSession,
    flow: &str,
) -> View {
    let mut slot = lock(state);
    if let Some(entry) = slot.as_mut().filter(|entry| entry.matches(flow)) {
        entry.expire(Instant::now(), session.lifecycle_epoch());
        entry.cancel();
        snapshot_for(
            entry,
            session.lifecycle_epoch(),
            crate::secrets::protection::generation::capture_current_selection()
                .ok()
                .as_ref(),
        )
    } else {
        super::unknown(Some(flow.into()), true, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unwinding_worker_records_unknown_without_awaiting_ipc() {
        let state = Arc::new(State::default());
        *lock(&state) = Some(Entry::preparing("flow".into(), 7));
        let worker_state = state.clone();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = WorkerGuard {
                state: worker_state,
                flow: "flow".into(),
            };
            panic!("fixture worker failed");
        }));
        assert!(outcome.is_err());
        let slot = lock(&state);
        let entry = slot.as_ref().unwrap();
        assert_eq!(entry.view.state, Phase::Unknown);
        assert_eq!(entry.view.reason, Some(Reason::Io));
    }
    #[tokio::test]
    async fn detached_worker_completion_survives_lost_ipc_waiter() {
        let state = Arc::new(State::default());
        let mut entry = Entry::preparing("flow".into(), 7);
        entry.view.state = Phase::Applying;
        *lock(&state) = Some(entry);
        let (release, wait) = std::sync::mpsc::channel();
        let (done, completed) = tokio::sync::oneshot::channel();
        let worker_state = state.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _guard = WorkerGuard {
                state: worker_state.clone(),
                flow: "flow".into(),
            };
            wait.recv().unwrap();
            lock(&worker_state).as_mut().unwrap().finish(Ok(Saved {
                selection: crate::workbuddy::native_source::SelectionKey {
                    id: "id".into(),
                    generation: 1,
                    incarnation: uuid::Uuid::new_v4(),
                },
                label: "reviewed".into(),
                realm: "www.workbuddy.cn".into(),
            }));
            let _ = done.send(());
        });
        drop(task);
        release.send(()).unwrap();
        completed.await.unwrap();
        let slot = lock(&state);
        let entry = slot.as_ref().unwrap();
        assert_eq!(entry.view.state, Phase::Saved);
        let hidden = snapshot_for(entry, 8, None);
        assert_eq!(hidden.state, Phase::Unknown);
        assert!(hidden.account.is_none());
        assert!(hidden.saved.is_none());
    }
    #[tokio::test]
    async fn applying_rejects_new_prepare_before_native_access() {
        let state = Arc::new(State::default());
        let mut entry = Entry::preparing("executing".into(), 0);
        entry.view.state = Phase::Applying;
        *lock(&state) = Some(entry);
        let db = Arc::new(crate::database::Database::memory().unwrap());
        assert!(matches!(
            prepare(state.clone(), db, "different".into()).await,
            Err(Reason::RequestConflict)
        ));
        assert!(lock(&state).as_ref().unwrap().matches("executing"));
    }
    #[tokio::test]
    async fn expiry_timer_runs_and_cancel_reclaims_queued_timer() {
        let state = Arc::new(State::default());
        let deadline = Instant::now();
        let mut entry = Entry::preparing("expired".into(), 7);
        entry.view.state = Phase::Ready;
        entry.deadline = Some(deadline);
        *lock(&state) = Some(entry);
        let task = schedule_expiry(
            &tokio::runtime::Handle::current(),
            &state,
            "expired".into(),
            deadline,
        );
        task.await.unwrap();
        assert_eq!(lock(&state).as_ref().unwrap().view.state, Phase::Unknown);
        let task = schedule_expiry(
            &tokio::runtime::Handle::current(),
            &state,
            "cancelled".into(),
            Instant::now() + std::time::Duration::from_secs(300),
        );
        let mut entry: Entry<Prepared> = Entry::preparing("cancelled".into(), 7);
        entry.set_expiry_task(task.abort_handle());
        entry.cancel();
        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn changed_durable_selector_with_same_session_epoch_redacts_saved_view() {
        use crate::app_store::isolated_scope::capture_selection_at;
        let root = crate::secrets::testing::tempdir().unwrap();
        let control = root.path().join("control");
        let data = root.path().join("data");
        let other = root.path().join("other");
        let device = root.path().join("device");
        for path in [&control, &data, &other, &device] {
            std::fs::create_dir(path).unwrap();
        }
        let selector = control.join("app_paths.json");
        let reviewed = capture_selection_at(&selector, &data, &device).unwrap();
        let session = crate::secrets::session::SecretSession::ephemeral().unwrap();
        let epoch = session.lifecycle_epoch();
        let mut entry: Entry<Prepared> = Entry::preparing("original-flow".into(), epoch);
        entry.selection = Some(reviewed.clone());
        entry.view.account = Some(AccountSummary {
            id: "id".into(),
            label: "reviewed".into(),
            realm: "www.workbuddy.cn".into(),
        });
        entry.finish(Ok(Saved {
            selection: crate::workbuddy::native_source::SelectionKey {
                id: "id".into(),
                generation: 1,
                incarnation: uuid::Uuid::new_v4(),
            },
            label: "reviewed".into(),
            realm: "www.workbuddy.cn".into(),
        }));
        assert_eq!(
            snapshot_for(&entry, epoch, Some(&reviewed)).state,
            Phase::Saved
        );
        std::fs::write(
            &selector,
            serde_json::to_vec(&serde_json::json!({ "app_config_dir_override": other })).unwrap(),
        )
        .unwrap();
        let changed = capture_selection_at(&selector, &data, &device).unwrap();
        assert!(reviewed != changed);
        assert_eq!(session.lifecycle_epoch(), epoch);
        let view = snapshot_for(&entry, epoch, Some(&changed));
        assert_eq!(view.state, Phase::Unknown);
        assert_eq!(view.reason, Some(Reason::SourceChanged));
        assert!(view.account.is_none());
        assert!(view.saved.is_none());
        // View validation never renews the old plan's selector or its authority.
        assert!(entry.selection.as_ref() == Some(&reviewed));
    }
}
