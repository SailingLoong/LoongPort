//! Command-facing native capture input and stored-row result.
use super::native_source::CapturedAccount;
use super::native_source::SelectionKey;
pub(crate) use super::native_transaction::Failure;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    pub(super) label: String,
    pub(super) expected: Option<SelectionKey>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Saved {
    state: &'static str,
    account: super::native_source::CapturedAccount,
    activation_available: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CurrentCaptures {
    accounts: Vec<CapturedAccount>,
    request_outcome: &'static str,
    activation_available: bool,
}
impl CurrentCaptures {
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    pub(super) fn unknown(accounts: Vec<CapturedAccount>) -> Self {
        Self {
            accounts,
            request_outcome: "unknown",
            activation_available: false,
        }
    }
}

// This is the concrete capture scheduling boundary: the selector and vault
// binding belong to the submitted action, before the blocking worker queues.
#[cfg(any(
    test,
    all(feature = "gui", target_os = "macos", target_arch = "aarch64")
))]
pub(super) fn schedule_capture<R: Send + 'static>(
    session: &crate::secrets::session::SecretSession,
    select: impl FnOnce() -> Result<crate::app_store::isolated_scope::SelectionSnapshot, Failure>,
    worker: impl FnOnce(
            crate::secrets::storage::StorageIdentity,
            u64,
            crate::app_store::isolated_scope::SelectionSnapshot,
        ) -> R
        + Send
        + 'static,
) -> Result<tokio::task::JoinHandle<R>, Failure> {
    let identity = super::store::binding(session).map_err(|_| Failure::Journal)?;
    let epoch = session.lifecycle_epoch();
    let selection = select()?;
    Ok(tokio::task::spawn_blocking(move || {
        worker(identity, epoch, selection)
    }))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "gui"))]
fn installed_bundle() -> Result<std::path::PathBuf, Failure> {
    use objc2::rc::Retained;
    use objc2_app_kit::NSWorkspace;
    // SAFETY: NSString copies the static NUL-terminated UTF-8 bundle identifier.
    let identifier: Retained<_> = unsafe {
        objc2::msg_send![objc2::class!(NSString), stringWithUTF8String: c"com.tencent.workbuddy.mac".as_ptr()]
    };
    let url = NSWorkspace::sharedWorkspace()
        .URLForApplicationWithBundleIdentifier(&identifier)
        .ok_or(Failure::UnsafePath)?;
    let path = url.path().ok_or(Failure::UnsafePath)?;
    std::fs::canonicalize(std::path::PathBuf::from(path.to_string()))
        .map_err(|_| Failure::UnsafePath)
}

#[cfg(feature = "gui")]
pub(crate) async fn capture(
    db: std::sync::Arc<crate::database::Database>,
    input: Input,
) -> Result<Saved, Failure> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let runtime = tokio::runtime::Handle::current();
        let worker_db = db.clone();
        return schedule_capture(
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
                    let bundle = installed_bundle()?;
                    let receipt = controller
                        .capture_installed(&bundle, input.expected.as_ref(), &input.label)
                        .await?;
                    Ok(Saved {
                        state: "saved",
                        account: receipt.view(&input.label)?,
                        activation_available: false,
                    })
                })
            },
        )?
        .await
        .map_err(|_| Failure::Io)?;
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let Input { label, expected } = input;
        let _ = (db, label, expected);
        Err(Failure::UnsafePath)
    }
}

#[cfg(feature = "gui")]
pub(crate) async fn list(
    session: &crate::secrets::session::SecretSession,
) -> Result<CurrentCaptures, Failure> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let identity = super::store::binding(session).map_err(|_| Failure::Journal)?;
        let epoch = session.lifecycle_epoch();
        let selection = crate::secrets::protection::generation::capture_current_selection()
            .map_err(|_| Failure::SourceChanged)?;
        let accounts = super::native_source::Catalog {
            session,
            identity: &identity,
            epoch,
            selection: &selection,
        }
        .list_captures()
        .await?;
        return Ok(CurrentCaptures::unknown(accounts));
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = session;
        Err(Failure::UnsafePath)
    }
}
