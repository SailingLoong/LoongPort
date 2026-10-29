//! Explicit local native transaction and original-journal recovery commands.
pub(crate) use super::native_source::SelectionKey;
pub(crate) use super::native_transaction::{Failure, Outcome};

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
                    controller.activate_installed(&target, &bundle).await
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
                    controller
                        .recover_installed_bound(&bundle, realm, &stamp)
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
