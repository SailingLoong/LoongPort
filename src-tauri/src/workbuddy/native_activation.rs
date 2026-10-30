//! Explicit local native transaction and original-journal recovery commands.
pub(crate) use super::native_source::SelectionKey;
pub(crate) use super::native_transaction::{Failure, Outcome};

#[cfg(any(test, feature = "gui"))]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwitchReport {
    transaction: Outcome,
    restart: &'static str,
}

#[cfg(any(test, feature = "gui"))]
#[derive(serde::Serialize)]
#[serde(tag = "stage", content = "reason", rename_all = "camelCase")]
pub(crate) enum SwitchError {
    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    Busy,
    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    QuitRefused,
    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    QuitTimeout,
    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    WritersRemaining,
    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    SourceChanged,
    #[cfg(any(
        test,
        all(feature = "gui", target_os = "macos", target_arch = "aarch64")
    ))]
    OutcomeUnknown,
    Transaction(Failure),
}

#[cfg(test)]
mod switch_wire_tests {
    use super::*;
    #[test]
    fn switch_results_only_expose_local_transaction_and_restart_stage() {
        let result = SwitchReport {
            transaction: Outcome::Applied,
            restart: "requested",
        };
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({"transaction":"applied","restart":"requested"})
        );
        for (error, stage) in [
            (SwitchError::Busy, "busy"),
            (SwitchError::QuitRefused, "quitRefused"),
            (SwitchError::QuitTimeout, "quitTimeout"),
            (SwitchError::WritersRemaining, "writersRemaining"),
            (SwitchError::SourceChanged, "sourceChanged"),
            (SwitchError::OutcomeUnknown, "outcomeUnknown"),
        ] {
            assert_eq!(
                serde_json::to_value(error).unwrap(),
                serde_json::json!({"stage":stage})
            );
        }
        assert_eq!(
            serde_json::to_value(SwitchError::Transaction(Failure::SourceChanged)).unwrap(),
            serde_json::json!({"stage":"transaction","reason":"sourceChanged"})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lost_worker_result_is_unknown_and_not_a_retry_instruction() {
        let worker: tokio::task::JoinHandle<
            Result<
                super::super::native_switch_lifecycle::Report,
                super::super::native_switch_lifecycle::Error,
            >,
        > = tokio::spawn(async { panic!("synthetic worker failure") });
        let result = finish_switch(worker.await).err().unwrap();
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::json!({"stage":"outcomeUnknown"})
        );
    }
}

#[cfg(any(
    all(test, unix),
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
fn finish_switch(
    joined: Result<
        Result<super::native_switch_lifecycle::Report, super::native_switch_lifecycle::Error>,
        tokio::task::JoinError,
    >,
) -> Result<SwitchReport, SwitchError> {
    use super::native_switch_lifecycle::{Error, Restart};
    let report = joined
        .map_err(|_| SwitchError::OutcomeUnknown)?
        .map_err(|e| match e {
            Error::Busy => SwitchError::Busy,
            Error::QuitRefused => SwitchError::QuitRefused,
            Error::QuitTimeout => SwitchError::QuitTimeout,
            Error::WritersRemaining => SwitchError::WritersRemaining,
            Error::SourceChanged => SwitchError::SourceChanged,
            Error::Transaction(f) => SwitchError::Transaction(f),
        })?;
    Ok(SwitchReport {
        transaction: report.transaction,
        restart: match report.restart {
            Restart::NotNeeded => "notNeeded",
            Restart::Requested => "requested",
            Restart::Failed => "failed",
            Restart::Unknown => "unknown",
        },
    })
}

#[cfg(all(feature = "gui", target_os = "macos", target_arch = "aarch64"))]
pub(crate) async fn activate_with_restart(
    db: std::sync::Arc<crate::database::Database>,
    target: SelectionKey,
) -> Result<SwitchReport, SwitchError> {
    use super::native_switch_lifecycle::Error;
    let runtime = tokio::runtime::Handle::current();
    let worker_db = db.clone();
    let handle = super::native_capture::schedule_capture(
        db.secret_session(),
        || {
            crate::secrets::protection::generation::capture_current_selection()
                .map_err(|_| Failure::SourceChanged)
        },
        move |identity, epoch, selection| {
            runtime.block_on(async move {
                let session = worker_db.secret_session();
                let controller = super::native_controller::Controller {
                    catalog: super::native_source::Catalog {
                        session,
                        identity: &identity,
                        epoch,
                        selection: &selection,
                    },
                };
                let bundle =
                    super::native_capture::installed_bundle().map_err(Error::Transaction)?;
                controller.activate_with_restart(&target, &bundle).await
            })
        },
    )
    .map_err(SwitchError::Transaction)?;
    finish_switch(handle.await)
}

#[cfg(all(
    feature = "gui",
    not(all(target_os = "macos", target_arch = "aarch64"))
))]
pub(crate) async fn activate_with_restart(
    _db: std::sync::Arc<crate::database::Database>,
    _target: SelectionKey,
) -> Result<SwitchReport, SwitchError> {
    Err(SwitchError::Transaction(Failure::UnsafePath))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryState {
    phase: &'static str,
    request_outcome: &'static str,
    can_request_activation: bool,
}

#[cfg(any(
    all(test, unix),
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
pub(super) async fn schedule_recovery<R: Send + 'static>(
    session: &crate::secrets::session::SecretSession,
    select: impl FnOnce() -> Result<crate::app_store::isolated_scope::SelectionSnapshot, Failure>,
    worker: impl FnOnce(
            crate::secrets::storage::StorageIdentity,
            u64,
            crate::app_store::isolated_scope::SelectionSnapshot,
            super::model::Domain,
            super::native_transaction::Stamp,
        ) -> R
        + Send
        + 'static,
) -> Result<tokio::task::JoinHandle<R>, Failure> {
    let identity = super::store::binding(session).map_err(|_| Failure::Journal)?;
    let epoch = session.lifecycle_epoch();
    let selection = select()?;
    let controller = super::native_controller::Controller {
        catalog: super::native_source::Catalog {
            session,
            identity: &identity,
            epoch,
            selection: &selection,
        },
    };
    let (realm, stamp) = controller.journal_binding().await?;
    Ok(tokio::task::spawn_blocking(move || {
        worker(identity, epoch, selection, realm, stamp)
    }))
}

#[cfg(any(
    all(test, unix),
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
pub(super) async fn state(
    catalog: &super::native_source::Catalog<'_>,
) -> Result<RecoveryState, Failure> {
    super::native_owner::with_selected_owner(
        catalog.session,
        catalog.identity,
        catalog.epoch,
        catalog.selection,
        |owner| {
            let disposition = owner.disposition()?;
            let phase = match disposition {
                super::native_owner::Disposition::Absent => "absent",
                super::native_owner::Disposition::Clear => "clear",
                super::native_owner::Disposition::Pending => "pending",
            };
            Ok(RecoveryState {
                phase,
                request_outcome: "unknown",
                can_request_activation: disposition != super::native_owner::Disposition::Pending,
            })
        },
    )
    .await
}

#[cfg(feature = "gui")]
pub(crate) async fn activate(
    db: std::sync::Arc<crate::database::Database>,
    target: SelectionKey,
) -> Result<Outcome, Failure> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let runtime = tokio::runtime::Handle::current();
        let worker_db = db.clone();
        super::native_capture::schedule_capture(
            db.secret_session(),
            || {
                crate::secrets::protection::generation::capture_current_selection()
                    .map_err(|_| Failure::SourceChanged)
            },
            move |identity, epoch, selection| {
                runtime.block_on(async move {
                    let session = worker_db.secret_session();
                    let controller = super::native_controller::Controller {
                        catalog: super::native_source::Catalog {
                            session,
                            identity: &identity,
                            epoch,
                            selection: &selection,
                        },
                    };
                    let bundle = super::native_capture::installed_bundle()?;
                    super::native_switch_lifecycle::run_existing_transaction(|| async {
                        controller.activate_installed(&target, &bundle).await
                    })
                    .await
                })
            },
        )?
        .await
        .map_err(|_| Failure::Io)?
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (db, target);
        Err(Failure::UnsafePath)
    }
}

#[cfg(feature = "gui")]
pub(crate) async fn recover(
    db: std::sync::Arc<crate::database::Database>,
) -> Result<Outcome, Failure> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let runtime = tokio::runtime::Handle::current();
        let worker_db = db.clone();
        schedule_recovery(
            db.secret_session(),
            || {
                crate::secrets::protection::generation::capture_current_selection()
                    .map_err(|_| Failure::SourceChanged)
            },
            move |identity, epoch, selection, realm, stamp| {
                runtime.block_on(async move {
                    let session = worker_db.secret_session();
                    let controller = super::native_controller::Controller {
                        catalog: super::native_source::Catalog {
                            session,
                            identity: &identity,
                            epoch,
                            selection: &selection,
                        },
                    };
                    let bundle = super::native_capture::installed_bundle()?;
                    super::native_switch_lifecycle::run_existing_transaction(|| async {
                        controller
                            .recover_installed_bound(&bundle, realm, &stamp)
                            .await
                    })
                    .await
                })
            },
        )
        .await?
        .await
        .map_err(|_| Failure::Io)?
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = db;
        Err(Failure::UnsafePath)
    }
}

#[cfg(feature = "gui")]
pub(crate) async fn recovery_state(
    session: &crate::secrets::session::SecretSession,
) -> Result<RecoveryState, Failure> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let identity = super::store::binding(session).map_err(|_| Failure::Journal)?;
        let epoch = session.lifecycle_epoch();
        let selection = crate::secrets::protection::generation::capture_current_selection()
            .map_err(|_| Failure::SourceChanged)?;
        state(&super::native_source::Catalog {
            session,
            identity: &identity,
            epoch,
            selection: &selection,
        })
        .await
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = session;
        Err(Failure::UnsafePath)
    }
}
