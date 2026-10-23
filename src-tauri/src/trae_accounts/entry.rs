//! Explicit selected-file authorization. Begin never reads installation secrets.
use super::{
    auth, import,
    inference::{self, Transport},
    store::{self, AccountView, Binding, Failure, Realm},
};
use crate::{proxy::ProxyError, secrets::session::SecretSession};
use serde::Serialize;
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Error {
    Cancelled,
    Busy,
    InvalidSelection,
    Unsupported,
    StorageUnavailable,
    AuthorizationUnavailable,
    PublicationUnknown,
    Expired,
}
fn owner_error(e: Failure) -> Error {
    match e {
        Failure::PublicationUnknown => Error::PublicationUnknown,
        Failure::Busy => Error::Busy,
        Failure::Unsupported => Error::Unsupported,
        _ => Error::StorageUnavailable,
    }
}
fn auth_error(e: ProxyError) -> Error {
    match e {
        ProxyError::AuthError(ref c) if c == "trae.publication_unknown" => {
            Error::PublicationUnknown
        }
        _ => Error::AuthorizationUnavailable,
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FlowView {
    pub flow_id: String,
    pub realm: Realm,
    pub auth_host: &'static str,
}
#[derive(Clone)]
pub(super) struct Selection {
    pub storage: PathBuf,
    pub product: PathBuf,
    pub edition: import::Edition,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Running,
    Saved,
    Cancelled,
    Failed,
}
struct Flow {
    root: PathBuf,
    binding: Binding,
    realm: Realm,
    selection: Selection,
    expires: i64,
    admission: Admission,
}
// Ephemeral flow admission only: no credential copies or second persistent owner.
// Mutations that can admit/cancel publication run inside store::access's original
// session/file/runtime barrier. Target generation prevents cancelling a newer login.
struct AdmissionState {
    phase: Phase,
    target: Option<(String, u64)>,
}
pub(super) struct Admission {
    expires: i64,
    state: Mutex<AdmissionState>,
}
impl Admission {
    pub(super) fn check(&self) -> Result<(), Failure> {
        let state = self.state.lock().map_err(|_| Failure::Busy)?;
        if state.phase != Phase::Running || self.expires <= store::now() {
            return Err(Failure::Retired);
        }
        Ok(())
    }
    pub(super) fn staged(&self, row: &store::Account) -> Result<(), Failure> {
        let mut state = self.state.lock().map_err(|_| Failure::Busy)?;
        if state.phase != Phase::Running || self.expires <= store::now() {
            return Err(Failure::Retired);
        }
        state.target = Some((row.id.clone(), row.generation));
        Ok(())
    }
}
/// Transport futures are first-polled by inference::admit under store::access.
/// Thus this check shares cancellation's admission barrier; no check/send gap.
struct FlowTransport<'a, T> {
    inner: &'a T,
    admission: &'a Admission,
}
impl<T: Transport> Transport for FlowTransport<'_, T> {
    fn send(
        &self,
        r: inference::Request,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Result<crate::proxy::hyper_client::ProxyResponse, ProxyError>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.admission
                .check()
                .map_err(|_| ProxyError::AuthError("trae.authorization_cancelled".into()))?;
            self.inner.send(r).await
        })
    }
}
fn flows() -> &'static Mutex<HashMap<String, Arc<Flow>>> {
    static F: OnceLock<Mutex<HashMap<String, Arc<Flow>>>> = OnceLock::new();
    F.get_or_init(Default::default)
}
fn selected_path(s: &str, name: &str) -> Result<PathBuf, Error> {
    let p = PathBuf::from(s);
    if !store::text(s, 8192)
        || !p.is_absolute()
        || p.file_name().and_then(|s| s.to_str()) != Some(name)
    {
        return Err(Error::InvalidSelection);
    }
    Ok(p)
}
/// Metadata only. The separate confirm command is the explicit read/grant action.
pub(crate) async fn begin(
    session: &SecretSession,
    storage_path: String,
    product_path: String,
    realm: Realm,
) -> Result<FlowView, Error> {
    let selection = Selection {
        storage: selected_path(&storage_path, "storage.json")?,
        product: selected_path(&product_path, "product.json")?,
        edition: if realm == Realm::Cn {
            import::Edition::Cn
        } else {
            import::Edition::Global
        },
    };
    let id = uuid::Uuid::new_v4().to_string();
    store::access(session, None, |_, binding| {
        let mut map = flows().lock().map_err(|_| Failure::Busy)?;
        map.retain(|_, f| f.expires > store::now());
        if map.len() >= 64 {
            return Err(Failure::Busy);
        }
        map.insert(
            id.clone(),
            Arc::new(Flow {
                root: session.root().into(),
                binding: binding.clone(),
                realm,
                selection,
                expires: store::now() + 600000,
                admission: Admission {
                    expires: store::now() + 600000,
                    state: Mutex::new(AdmissionState {
                        phase: Phase::Pending,
                        target: None,
                    }),
                },
            }),
        );
        Ok(((), false))
    })
    .await
    .map_err(owner_error)?;
    Ok(FlowView {
        flow_id: id,
        realm,
        auth_host: realm.auth(),
    })
}
fn flow(session: &SecretSession, id: &str) -> Result<Arc<Flow>, Error> {
    let f = flows()
        .lock()
        .map_err(|_| Error::Busy)?
        .get(id)
        .cloned()
        .ok_or(Error::Expired)?;
    if f.root != session.root() {
        return Err(Error::InvalidSelection);
    }
    Ok(f)
}
pub(crate) async fn confirm(session: Arc<SecretSession>, id: &str) -> Result<AccountView, Error> {
    confirm_with(
        session,
        id,
        &inference::OfficialHttp,
        |selection| async move {
            tokio::task::spawn_blocking(move || {
                import::read_selected(&selection.storage, &selection.product, selection.edition)
            })
            .await
            .map_err(|_| Failure::Storage)?
        },
    )
    .await
}
pub(super) async fn confirm_with<R, F>(
    session: Arc<SecretSession>,
    id: &str,
    transport: &impl Transport,
    read: R,
) -> Result<AccountView, Error>
where
    R: FnOnce(Selection) -> F,
    F: Future<Output = Result<import::Material, Failure>>,
{
    let f = flow(&session, id)?;
    let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
    let mut pending = None;
    let mut ready = None;
    store::access(&session, Some(&f.binding), |_, _| {
        let mut state = f.admission.state.lock().map_err(|_| Failure::Busy)?;
        if state.phase != Phase::Pending || f.expires <= store::now() {
            return Err(Failure::Retired);
        }
        state.phase = Phase::Running;
        drop(state);
        // Read admission shares the original owner barrier, just like network
        // admission. Cancel that wins this barrier cannot start a new file read.
        let mut future = Box::pin(read(f.selection.clone()));
        match future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            std::task::Poll::Ready(result) => ready = Some(result),
            std::task::Poll::Pending => pending = Some(future),
        }
        Ok(((), false))
    })
    .await
    .map_err(|_| Error::Cancelled)?;
    let result = async {
        let material = if let Some(result) = ready {
            result
        } else {
            pending.ok_or(Error::Cancelled)?.await
        }
        .map_err(|_| Error::InvalidSelection)?;
        if material.account.realm != f.realm {
            return Err(Error::InvalidSelection);
        }
        f.admission.check().map_err(|_| Error::Cancelled)?;
        let guarded = FlowTransport {
            inner: transport,
            admission: &f.admission,
        };
        let account_id = auth::activate_guarded(
            session.clone(),
            material,
            Duration::from_secs(60),
            &guarded,
            &f.binding,
            &f.admission,
        )
        .await
        .map_err(auth_error)?;
        store::access(&session, Some(&f.binding), |d, _| {
            let mut state = f.admission.state.lock().map_err(|_| Failure::Busy)?;
            if state.phase != Phase::Running || f.expires <= store::now() {
                return Err(Failure::Retired);
            }
            let a = d
                .accounts
                .iter()
                .find(|a| {
                    a.id == account_id
                        && a.realm == f.realm
                        && a.grant.is_some()
                        && !a.refresh_pending
                        && state.target.as_ref().is_some_and(|(id, generation)| {
                            id == &a.id && *generation == a.generation
                        })
                })
                .ok_or(Failure::Retired)?;
            state.phase = Phase::Saved;
            Ok((a.view(), false))
        })
        .await
        .map_err(|_| Error::Cancelled)
    }
    .await;
    if result.is_err() {
        if let Ok(mut state) = f.admission.state.lock() {
            if state.phase == Phase::Running {
                state.phase = Phase::Failed
            }
        }
    }
    result
}
pub(crate) async fn cancel(session: &SecretSession, id: &str) -> Result<(), Error> {
    let f = flow(session, id)?;
    store::access(session, Some(&f.binding), |d, _| {
        let mut state = f.admission.state.lock().map_err(|_| Failure::Busy)?;
        state.phase = Phase::Cancelled;
        let mut dirty = false;
        if let Some((id, generation)) = &state.target {
            if let Some(row) = d
                .accounts
                .iter_mut()
                .find(|a| &a.id == id && a.generation == *generation)
            {
                row.generation = row.generation.checked_add(1).ok_or(Failure::Storage)?;
                row.auto_checkin_enabled = false;
                row.auto_checkin_revision = row
                    .auto_checkin_revision
                    .checked_add(1)
                    .ok_or(Failure::Storage)?;
                row.grant = None;
                row.refresh_pending = false;
                dirty = true;
            }
        }
        Ok(((), dirty))
    })
    .await
    .map_err(owner_error)
}
pub(crate) async fn list(session: &SecretSession) -> Result<Vec<AccountView>, Error> {
    store::list(session).await.map_err(owner_error)
}
pub(crate) async fn retire(session: &SecretSession, id: &str) -> Result<(), Error> {
    store::retire(session, id).await.map_err(owner_error)
}
#[cfg(test)]
mod tests;
