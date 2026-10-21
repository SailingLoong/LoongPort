//! Explicit chat_v3 catalog discovery using the existing selected account owner.
//! Wire facts: magpie-community/plugins@5a836745, index.mjs:2317-2327,2611-2619.
use super::{
    auth,
    inference::{self, Lease, Transport},
    protocol,
    store::{self, Binding, Failure, Realm},
};
use crate::{
    proxy::{hyper_client::ProxyResponse, ProxyError},
    secrets::{session::SecretSession, stream_guard::KnownSecretGuard},
};
use bytes::Bytes;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
const PATH: &str = "/api/ide/v1/get_detail_param";
const TTL_MS: i64 = 120_000;
const LIMIT: usize = 64;
const MAX_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ModelView {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestView {
    pub request_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Error {
    Cancelled,
    Expired,
    Busy,
    Unsupported,
    AuthorizationUnavailable,
    PermissionDenied,
    RateLimited,
    ResponseInvalid,
    Unavailable,
    StorageUnavailable,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Running,
    Done,
    Cancelled,
    Failed,
}
struct Operation {
    root: PathBuf,
    binding: Binding,
    account_id: String,
    realm: Realm,
    identity: String,
    expires: i64,
    phase: Mutex<Phase>,
}
fn operations() -> &'static Mutex<HashMap<String, Arc<Operation>>> {
    static OPS: OnceLock<Mutex<HashMap<String, Arc<Operation>>>> = OnceLock::new();
    OPS.get_or_init(Default::default)
}
fn owner_error(e: Failure) -> Error {
    match e {
        Failure::Unsupported => Error::Unsupported,
        Failure::Busy => Error::Busy,
        _ => Error::StorageUnavailable,
    }
}
fn identity(a: &store::Account) -> Result<String, Failure> {
    // Store only a digest, never a credential or a directory listing.
    let bytes = zeroize::Zeroizing::new(
        serde_json::to_vec(&(&a.id, &a.uid, a.realm, a.generation, &a.device))
            .map_err(|_| Failure::Invalid)?,
    );
    Ok(hex::encode(Sha256::digest(bytes.as_slice())))
}
impl Operation {
    fn check(&self) -> Result<(), Failure> {
        if self.expires <= store::now()
            || *self.phase.lock().map_err(|_| Failure::Busy)? != Phase::Running
        {
            return Err(Failure::Retired);
        }
        Ok(())
    }
    fn matches(&self, lease: &Lease) -> bool {
        self.root == lease.session.root()
            && self.binding == lease.snapshot.binding
            && self.account_id == lease.snapshot.account.id
            && self.realm == lease.snapshot.account.realm
            && identity(&lease.snapshot.account).is_ok_and(|id| id == self.identity)
    }
}
/// Metadata only. No HTTP, refresh, native file selection or model list cache.
pub(crate) async fn begin(
    session: Arc<SecretSession>,
    account_id: &str,
) -> Result<RequestView, Error> {
    let id = uuid::Uuid::new_v4().to_string();
    let lease = Lease::acquire(session.clone(), account_id, "model-discovery")
        .await
        .map_err(owner_error)?;
    store::access(&session, Some(&lease.snapshot.binding), |d, b| {
        let a = store::matched(d, &lease.snapshot)?;
        if a.refresh_pending || a.version != lease.snapshot.account.version {
            return Err(Failure::Retired);
        }
        let mut ops = operations().lock().map_err(|_| Failure::Busy)?;
        ops.retain(|_, op| op.expires > store::now());
        if ops.len() >= LIMIT {
            return Err(Failure::Busy);
        }
        ops.insert(
            id.clone(),
            Arc::new(Operation {
                root: session.root().into(),
                binding: b.clone(),
                account_id: account_id.into(),
                realm: a.realm,
                identity: identity(a)?,
                expires: store::now() + TTL_MS,
                phase: Mutex::new(Phase::Pending),
            }),
        );
        Ok(((), false))
    })
    .await
    .map_err(owner_error)?;
    Ok(RequestView { request_id: id })
}
fn operation(session: &SecretSession, id: &str) -> Result<Arc<Operation>, Error> {
    let op = operations()
        .lock()
        .map_err(|_| Error::Busy)?
        .get(id)
        .cloned()
        .ok_or(Error::Expired)?;
    if op.root != session.root() {
        return Err(Error::Cancelled);
    }
    if op.expires <= store::now() {
        return Err(Error::Expired);
    }
    Ok(op)
}
pub(crate) async fn cancel(session: &SecretSession, request_id: &str) -> Result<(), Error> {
    let op = operation(session, request_id)?;
    store::access(session, Some(&op.binding), |_, _| {
        *op.phase.lock().map_err(|_| Failure::Busy)? = Phase::Cancelled;
        Ok(((), false))
    })
    .await
    .map_err(owner_error)
}
struct Guarded<'a, T> {
    inner: &'a T,
    op: &'a Operation,
}
impl<T: Transport> Transport for Guarded<'_, T> {
    fn send(
        &self,
        r: inference::Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.op
                .check()
                .map_err(|_| ProxyError::Internal("trae.model_discovery_cancelled".into()))?;
            self.inner.send(r).await
        })
    }
}
pub(crate) async fn list(
    session: Arc<SecretSession>,
    request_id: &str,
) -> Result<Vec<ModelView>, Error> {
    list_with(session, request_id, &inference::OfficialHttp).await
}
pub(super) async fn list_with(
    session: Arc<SecretSession>,
    request_id: &str,
    transport: &impl Transport,
) -> Result<Vec<ModelView>, Error> {
    let op = operation(&session, request_id)?;
    let mut lease = Lease::acquire(session.clone(), &op.account_id, "model-discovery")
        .await
        .map_err(owner_error)?;
    if !op.matches(&lease) {
        return Err(Error::Cancelled);
    }
    store::access(&session, Some(&op.binding), |d, _| {
        let a = store::matched(d, &lease.snapshot)?;
        let mut phase = op.phase.lock().map_err(|_| Failure::Busy)?;
        if *phase != Phase::Pending
            || op.expires <= store::now()
            || a.version != lease.snapshot.account.version
            || a.refresh_pending
        {
            return Err(Failure::Retired);
        }
        *phase = Phase::Running;
        Ok(((), false))
    })
    .await
    .map_err(|_| Error::Cancelled)?;
    let result=async {
        let guarded=Guarded{inner:transport,op:&op};
        auth::synchronize(&session,&mut lease.snapshot).await.map_err(|_|Error::AuthorizationUnavailable)?;
        op.check().map_err(|_|Error::Cancelled)?;
        if lease.snapshot.account.grant.as_ref().ok_or(Error::AuthorizationUnavailable)?.expires<=store::now()+120000 {
            auth::refresh(&session,&mut lease.snapshot,Duration::from_secs(20),&guarded).await.map_err(|_|Error::AuthorizationUnavailable)?;
        }
        op.check().map_err(|_|Error::Cancelled)?;
        if !op.matches(&lease){return Err(Error::Cancelled)}
        let mut request=lease.wire(json!({"function":"chat_v3","config_names":null,"need_prompt":false,"current_config_info":null,"poly_prompt":true,"mode_type":null,"agent_type":null}),Duration::from_secs(20)).map_err(|_|Error::AuthorizationUnavailable)?;
        request.url=format!("{}{PATH}",lease.base_url());
        request.headers.insert("accept",http::HeaderValue::from_static("application/json"));
        let response=inference::admit(&session,&lease.snapshot,&guarded,request,false).await.map_err(|_|Error::Unavailable)?;
        lease.fence().check().await.map_err(|_|Error::Cancelled)?;op.check().map_err(|_|Error::Cancelled)?;
        let status=response.status().as_u16();
        if response.headers().get(http::header::CONTENT_ENCODING).is_some_and(|h|h!="identity"){return Err(Error::ResponseInvalid)}
        let bytes=response.bytes_with_limit(MAX_BYTES).await.map_err(|_|Error::ResponseInvalid)?;
        lease.fence().check().await.map_err(|_|Error::Cancelled)?;
        op.check().map_err(|_|Error::Cancelled)?;
        let decoded=serde_json::from_slice::<Value>(&bytes);
        // A recognized denial returns only this fixed enum. Its raw body is
        // discarded, including any credential echo; it is never a catalog.
        if let Ok(body)=&decoded {
            if let Some(code)=protocol::denial(body){return Err(match code {403=>Error::PermissionDenied,401=>Error::AuthorizationUnavailable,429=>Error::RateLimited,_=>Error::Unavailable})}
        }
        if status!=200{return Err(match status {401=>Error::AuthorizationUnavailable,403=>Error::PermissionDenied,429=>Error::RateLimited,_=>Error::Unavailable})}
        let body=decoded.map_err(|_|Error::ResponseInvalid)?;
        let grant=lease.snapshot.account.grant.as_ref().ok_or(Error::AuthorizationUnavailable)?;
        screen(&[&bytes],grant)?;
        let models=parse(&body)?;
        let semantic:Vec<&[u8]>=models.iter().flat_map(|m|[m.id.as_bytes(),m.name.as_bytes()]).collect();screen(&semantic,grant)?;
        // No result leaves without the same owner, key epoch, account identity,
        // grant version and cancellation phase being checked under one barrier.
        store::access(&session,Some(&op.binding),|d,_|{
            let a=store::matched(d,&lease.snapshot)?;
            op.check()?;
            if a.version!=lease.snapshot.account.version || a.refresh_pending {return Err(Failure::Retired)}
            *op.phase.lock().map_err(|_|Failure::Busy)?=Phase::Done;Ok((models,false))
        }).await.map_err(|_|Error::Cancelled)
    }.await;
    if result.is_err() {
        if let Ok(mut p) = op.phase.lock() {
            if *p == Phase::Running {
                *p = Phase::Failed
            }
        }
    }
    result
}
fn screen(parts: &[&[u8]], grant: &store::Grant) -> Result<(), Error> {
    let mut guard = KnownSecretGuard::new(&[grant.access.as_bytes(), grant.refresh.as_bytes()])
        .map_err(|_| Error::ResponseInvalid)?;
    for p in parts {
        guard
            .feed(Bytes::copy_from_slice(p))
            .map_err(|_| Error::ResponseInvalid)?;
    }
    let _ = guard.finish();
    Ok(())
}
fn parse(body: &Value) -> Result<Vec<ModelView>, Error> {
    if ["/error", "/Error", "/ResponseMetadata/Error"]
        .iter()
        .any(|path| body.pointer(path).is_some_and(|e| !e.is_null()))
    {
        return Err(Error::ResponseInvalid);
    }
    let values = body
        .get("config_info_list")
        .or_else(|| body.pointer("/data/config_info_list"))
        .and_then(Value::as_array)
        .ok_or(Error::ResponseInvalid)?;
    if values.len() > 512 {
        return Err(Error::ResponseInvalid);
    }
    let mut models = BTreeMap::new();
    for entry in values {
        let id = entry
            .get("config_name")
            .and_then(Value::as_str)
            .filter(|s| store::text(s, 256))
            .ok_or(Error::ResponseInvalid)?;
        if id.to_ascii_lowercase().starts_with("custom_model") {
            continue;
        }
        if entry
            .get("usage")
            .is_some_and(|v| v.as_str() != Some("chat_completion"))
        {
            continue;
        }
        for key in ["config_switch", "is_custom_model", "is_invisible_to_user"] {
            if entry.get(key).is_some_and(|v| !v.is_boolean()) {
                return Err(Error::ResponseInvalid);
            }
        }
        if entry
            .get("display_config")
            .is_some_and(|v| !v.is_null() && !v.is_object())
            || entry
                .pointer("/display_config/is_custom_model")
                .is_some_and(|v| !v.is_boolean())
        {
            return Err(Error::ResponseInvalid);
        }
        if entry.get("config_switch") == Some(&Value::Bool(false))
            || entry.get("is_custom_model") == Some(&Value::Bool(true))
            || entry.get("is_invisible_to_user") == Some(&Value::Bool(true))
            || entry.pointer("/display_config/is_custom_model") == Some(&Value::Bool(true))
        {
            continue;
        }
        let name = entry
            .pointer("/display_config/display_name")
            .or_else(|| entry.get("display_name"))
            .or_else(|| entry.get("display_model_name"));
        let name = match name {
            None => id,
            Some(v) => v
                .as_str()
                .filter(|s| store::text(s, 256))
                .ok_or(Error::ResponseInvalid)?,
        };
        if models.get(id).is_some_and(|n: &String| n != name) {
            return Err(Error::ResponseInvalid);
        }
        models.insert(id.to_owned(), name.to_owned());
    }
    Ok(models
        .into_iter()
        .map(|(id, name)| ModelView { id, name })
        .collect())
}
#[cfg(test)]
mod tests;
