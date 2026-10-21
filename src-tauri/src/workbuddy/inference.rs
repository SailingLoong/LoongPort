//! Account-bound WorkBuddy HTTP inference; never reads native sessions.
//!
//! Protocol facts adapted from magpie-community/plugins 8cdf2d20206dab5071ba15252d3f83fce3d9c666,
//! packages/workbuddy/index.mjs (MIT; licenses/magpie-workbuddy-MIT.txt).
//! Only ordinary authorization, refresh, model discovery and Chat/SSE transport
//! are used. No prompt rewriting, refusal retry, usage pricing or ACP session.
use super::{
    model::{Domain, Failure, SavedAccount},
    store,
};
use crate::{
    proxy::{hyper_client::ProxyResponse, ProxyError},
    secrets::{session::SecretSession, VaultMetadata},
};
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

#[path = "inference_sse.rs"]
mod inference_sse;

const ORIGIN: &str = "https://copilot.tencent.com";
const PLATFORM: &str = "workbuddy";
const MAX_JSON: usize = 4 * 1024 * 1024;
const EARLY_MS: i64 = 60_000;

/// Backend-only second grant on the SAME encrypted account row. No Debug and
/// no UI DTO exposure. The legacy check-in access token remains untouched.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Authorization {
    platform: String,
    domain: Domain,
    uid: String,
    grant_id: String,
    credential_version: u64,
    access: String,
    refresh: String,
    expires_ms: i64,
    refresh_expires_ms: Option<i64>,
}
impl Drop for Authorization {
    fn drop(&mut self) {
        self.access.zeroize();
        self.refresh.zeroize();
    }
}

#[derive(Clone)]
struct Owner {
    binding: VaultMetadata,
    epoch: u64,
    account_id: String,
    uid: String,
    domain: Domain,
    // Detect replacement of the legacy row without sending its token anywhere.
    legacy_fingerprint: [u8; 32],
    original_grant_id: Option<String>,
}
impl Owner {
    fn from_row(session: &SecretSession, binding: VaultMetadata, row: &SavedAccount) -> Self {
        Self {
            epoch: session.lifecycle_epoch(),
            account_id: row.account.id(&binding.vault_id),
            uid: row.account.uid.clone(),
            domain: row.account.domain,
            legacy_fingerprint: Sha256::digest(row.account.token.as_bytes()).into(),
            original_grant_id: row.inference.as_ref().map(|g| g.grant_id.clone()),
            binding,
        }
    }
    fn rows(&self, session: &SecretSession) -> Result<(Vec<SavedAccount>, usize), Failure> {
        if session.lifecycle_epoch() != self.epoch || store::binding(session)? != self.binding {
            return Err(Failure::ExpiredAuthorization);
        }
        let rows = store::load(session, &self.binding)?;
        let index = rows
            .iter()
            .position(|r| r.account.id(&self.binding.vault_id) == self.account_id)
            .ok_or(Failure::ExpiredAuthorization)?;
        let row = &rows[index];
        let fingerprint: [u8; 32] = Sha256::digest(row.account.token.as_bytes()).into();
        if row.account.uid != self.uid
            || row.account.domain != self.domain
            || fingerprint != self.legacy_fingerprint
            || row.inference.as_ref().map(|g| g.grant_id.as_str())
                != self.original_grant_id.as_deref()
        {
            return Err(Failure::ExpiredAuthorization);
        }
        Ok((rows, index))
    }
}

/// Opaque capability; account ID must originate in the selected trusted pool row.
pub(crate) struct Lease {
    session: Arc<SecretSession>,
    owner: Owner,
    grant: Authorization,
    response_blocked: Arc<AtomicBool>,
    response_denial: Arc<AtomicU32>,
}
#[derive(Clone)]
pub(crate) struct ResponseFence {
    lease: Arc<Lease>,
}
pub(crate) struct DispatchResponse {
    pub response: ProxyResponse,
    pub fence: ResponseFence,
}
impl ResponseFence {
    pub(crate) async fn check(&self) -> Result<(), ProxyError> {
        if self.lease.response_blocked.load(Ordering::Acquire) {
            return Err(ProxyError::Internal(
                "workbuddy.unsafe_response_body".into(),
            ));
        }
        let code = self.lease.response_denial.load(Ordering::Acquire);
        if matches!(code, 11004 | 11140) {
            return Err(ProxyError::UpstreamError {
                status: 403,
                body: Some(safe_error_value(200, Some(i64::from(code)), true).to_string()),
            });
        }
        self.lease
            .check()
            .await
            .map_err(|_| ProxyError::Internal("workbuddy.response_authorization_changed".into()))
    }
    fn terminal_stream_error(&self) -> std::io::Error {
        let code = self.lease.response_denial.load(Ordering::Acquire);
        if !self.lease.response_blocked.load(Ordering::Acquire) && matches!(code, 11004 | 11140) {
            std::io::Error::other(safe_error_value(200, Some(i64::from(code)), true).to_string())
        } else {
            stream_error(true)
        }
    }
    /// MUST be placed after original priming, buffering and protocol conversion,
    /// immediately around the final downstream body, including buffered bodies.
    pub(crate) fn wrap_stream<S, E>(
        &self,
        stream: S,
    ) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static
    where
        S: futures::Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Send + 'static,
    {
        let fence = self.clone();
        futures::stream::unfold(
            (fence, Box::pin(stream), false),
            |(fence, mut stream, stopped)| async move {
                if stopped {
                    return None;
                }
                if fence.check().await.is_err() {
                    return Some((Err(fence.terminal_stream_error()), (fence, stream, true)));
                }
                let next = stream.next().await;
                // Check EOF too: buffered collectors must not accept a successful empty
                // result after authorization changed during the final pending read.
                if fence.check().await.is_err() {
                    return Some((Err(fence.terminal_stream_error()), (fence, stream, true)));
                }
                match next {
                    Some(Ok(bytes)) => Some((Ok(bytes), (fence, stream, false))),
                    Some(Err(_)) => Some((Err(stream_error(false)), (fence, stream, true))),
                    None => None,
                }
            },
        )
    }
}
fn stream_error(auth: bool) -> std::io::Error {
    std::io::Error::other(if auth {
        "workbuddy.stream_authorization_changed"
    } else {
        "workbuddy.stream_interrupted"
    })
}
fn owner_error(_: Failure) -> ProxyError {
    ProxyError::AuthError("workbuddy.authorization_unavailable".into())
}
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn valid_text(v: &str, max: usize) -> bool {
    !v.trim().is_empty() && v.len() <= max && !v.chars().any(char::is_control)
}
fn token(v: &Value, key: &str) -> Result<String, Failure> {
    let s = v
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| valid_text(s, 16384))
        .ok_or(Failure::NeedsVerification)?;
    http::HeaderValue::from_str(s).map_err(|_| Failure::NeedsVerification)?;
    Ok(s.into())
}
fn data(v: &Value) -> Result<&Value, Failure> {
    if code(v) != Some(0) {
        return Err(Failure::NeedsVerification);
    }
    v.get("data")
        .filter(|v| v.is_object())
        .ok_or(Failure::Unconfirmed)
}
fn code(v: &Value) -> Option<i64> {
    v.get("code")
        .and_then(|c| c.as_i64().or_else(|| c.as_str()?.parse().ok()))
}
// The documented error envelopes use all four of these structured paths.
// A denial anywhere takes precedence over another envelope's technical code;
// never infer denial by searching arbitrary human/model message text.
fn upstream_error_code(v: &Value) -> Option<i64> {
    let found: Vec<i64> = [
        v.pointer("/error/data/code"),
        v.pointer("/error/code"),
        v.get("code"),
        v.pointer("/data/code"),
    ]
    .into_iter()
    .flatten()
    .filter_map(|c| c.as_i64().or_else(|| c.as_str()?.parse().ok()))
    .collect();
    found
        .iter()
        .copied()
        .find(|c| matches!(c, 11004 | 11140))
        .or_else(|| found.first().copied())
}
fn expiry(v: &Value, at: &str, seconds: &str, now: i64) -> Result<Option<i64>, Failure> {
    if let Some(n) = v.get(at) {
        let n = n
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or(Failure::NeedsVerification)?;
        return Ok(Some(n));
    }
    if let Some(n) = v.get(seconds) {
        let n = n
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or(Failure::NeedsVerification)?;
        return now
            .checked_add(n.checked_mul(1000).ok_or(Failure::NeedsVerification)?)
            .map(Some)
            .ok_or(Failure::NeedsVerification);
    }
    Ok(None)
}
fn personal(v: &Value) -> bool {
    ["enterpriseId", "enterprise_id", "tenantId"]
        .iter()
        .all(|key| {
            v.get(key)
                .is_none_or(|x| x.is_null() || x.as_str() == Some(""))
        })
        && v.get("type")
            .is_none_or(|x| matches!(x.as_str(), Some("personal" | "individual")))
}
fn new_grant(
    owner: &Owner,
    token_env: &Value,
    profile_env: &Value,
    now: i64,
) -> Result<Authorization, Failure> {
    let t = data(token_env)?;
    let p = data(profile_env)?;
    if !personal(t) || !personal(p) {
        return Err(Failure::UnsupportedContext);
    }
    let uid = p
        .get("uid")
        .and_then(Value::as_str)
        .filter(|s| valid_text(s, 256))
        .ok_or(Failure::UnsupportedContext)?;
    let domain = Domain::parse(
        t.get("domain")
            .and_then(Value::as_str)
            .ok_or(Failure::UnsupportedContext)?,
    )?;
    // No domain aliasing, UID-only cross-realm binding or audience migration.
    if uid != owner.uid
        || domain != owner.domain
        || t.get("uid").is_some_and(|v| v.as_str() != Some(uid))
        || p.get("domain")
            .is_some_and(|v| v.as_str() != Some(domain.host()))
        || t.get("platform")
            .is_some_and(|v| v.as_str() != Some(PLATFORM))
        || p.get("platform")
            .is_some_and(|v| v.as_str() != Some(PLATFORM))
        || t.get("tokenType")
            .is_some_and(|v| !v.as_str().is_some_and(|v| v.eq_ignore_ascii_case("bearer")))
    {
        return Err(Failure::UnsupportedContext);
    }
    let expires_ms = expiry(t, "expiresAt", "expiresIn", now)?
        .filter(|v| *v > now)
        .ok_or(Failure::NeedsVerification)?;
    let refresh_expires_ms = expiry(t, "refreshExpiresAt", "refreshExpiresIn", now)?;
    Ok(Authorization {
        platform: PLATFORM.into(),
        domain,
        uid: uid.into(),
        grant_id: uuid::Uuid::new_v4().to_string(),
        credential_version: 1,
        access: token(t, "accessToken")?,
        refresh: token(t, "refreshToken")?,
        expires_ms,
        refresh_expires_ms,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Endpoint {
    State,
    Token,
    Profile,
    Refresh,
    Models,
    Chat,
}
impl Endpoint {
    fn path(self) -> &'static str {
        match self {
            Self::State => "/v2/plugin/auth/state",
            Self::Token => "/v2/plugin/auth/token",
            Self::Profile => "/v2/plugin/login/account",
            Self::Refresh => "/v2/plugin/auth/token/refresh",
            Self::Models => "/v3/config",
            Self::Chat => "/v2/chat/completions",
        }
    }
    fn method(self) -> http::Method {
        if matches!(self, Self::Token | Self::Profile | Self::Models) {
            http::Method::GET
        } else {
            http::Method::POST
        }
    }
}
pub(super) struct Request {
    endpoint: Endpoint,
    query: Option<(String, String)>,
    headers: http::HeaderMap,
    body: Vec<u8>,
    timeout: Duration,
}
pub(super) trait Transport: Send + Sync {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>>;
}
struct Official;
impl Transport for Official {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            // Reuse original proxy/TLS/no-redirect/no-protocol-retry policy. This
            // profile name does NOT add a pricing or billing path.
            let client = crate::proxy::http_client::get_budgeted_authenticated()
                .map_err(|_| ProxyError::ConfigError("workbuddy.transport_unavailable".into()))?;
            let mut url = reqwest::Url::parse(&format!("{ORIGIN}{}", request.endpoint.path()))
                .map_err(|_| ProxyError::ConfigError("workbuddy.invalid_endpoint".into()))?;
            if let Some((k, v)) = request.query {
                url.query_pairs_mut().append_pair(&k, &v);
            }
            let streaming = request.endpoint == Endpoint::Chat;
            let mut builder = client
                .request(request.endpoint.method(), url)
                .headers(request.headers);
            if !request.body.is_empty() {
                builder = builder.body(request.body);
            }
            let timeout = if request.timeout.is_zero() {
                Duration::from_secs(30)
            } else {
                request.timeout
            };
            builder = builder.timeout(if streaming {
                Duration::from_secs(24 * 60 * 60)
            } else {
                timeout
            });
            let response = tokio::time::timeout(timeout, builder.send())
                .await
                .map_err(|_| ProxyError::Timeout("workbuddy.response_headers_timeout".into()))?
                .map_err(|_| ProxyError::ForwardFailed("workbuddy.transport_failed".into()))?;
            Ok(ProxyResponse::Reqwest(response))
        })
    }
}
fn headers() -> http::HeaderMap {
    let mut h = http::HeaderMap::new();
    for (k, v) in [
        ("content-type", "application/json"),
        ("accept", "application/json"),
        ("accept-encoding", "identity"),
        ("user-agent", "WorkBuddy/5.5.6"),
        ("x-requested-with", "XMLHttpRequest"),
    ] {
        h.insert(k, http::HeaderValue::from_static(v));
    }
    h
}
fn set(h: &mut http::HeaderMap, k: &'static str, v: &str) -> Result<(), Failure> {
    let mut value = http::HeaderValue::from_str(v).map_err(|_| Failure::NeedsVerification)?;
    if matches!(k, "authorization" | "x-refresh-token" | "x-user-id") {
        value.set_sensitive(true);
    }
    h.insert(k, value);
    Ok(())
}
fn signed(grant: &Authorization) -> Result<http::HeaderMap, Failure> {
    let mut h = headers();
    set(&mut h, "authorization", &format!("Bearer {}", grant.access))?;
    set(&mut h, "x-domain", grant.domain.host())?;
    set(&mut h, "x-user-id", &grant.uid)?;
    for (k, v) in [
        ("x-product", "SaaS"),
        ("x-ide-type", "WorkBuddy"),
        ("x-agent-intent", "craft"),
        ("x-agent-purpose", "conversation"),
        ("x-ide-name", "WorkBuddy"),
        ("x-ide-version", "5.5.6"),
    ] {
        h.insert(k, http::HeaderValue::from_static(v));
    }
    Ok(h)
}
async fn json_response(response: ProxyResponse) -> Result<Value, Failure> {
    if !response.status().is_success() {
        return Err(Failure::NeedsVerification);
    }
    if !response
        .headers()
        .get_all(http::header::CONTENT_ENCODING)
        .iter()
        .all(|value| {
            value
                .to_str()
                .is_ok_and(|value| value.trim().eq_ignore_ascii_case("identity"))
        })
    {
        return Err(Failure::Unconfirmed);
    }
    let bytes = response
        .bytes_with_limit(MAX_JSON)
        .await
        .map_err(|_| Failure::Unconfirmed)?;
    serde_json::from_slice(&bytes).map_err(|_| Failure::Unconfirmed)
}
async fn send_owner<T: Transport>(
    session: &SecretSession,
    owner: &Owner,
    grant: Option<&Authorization>,
    transport: &T,
    request: Request,
) -> Result<ProxyResponse, ProxyError> {
    send_cancellable(session, owner, grant, transport, request, None).await
}
async fn send_cancellable<T: Transport>(
    session: &SecretSession,
    owner: &Owner,
    grant: Option<&Authorization>,
    transport: &T,
    request: Request,
    cancellation: Option<&CancellationHandle>,
) -> Result<ProxyResponse, ProxyError> {
    let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
    let mut pending = None;
    let mut ready = None;
    {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        if let Some(cancellation) = cancellation {
            cancellation.check().map_err(owner_error)?;
        }
        let (rows, index) = owner.rows(session).map_err(owner_error)?;
        if let Some(expected) = grant {
            match rows[index].inference.as_ref() {
                Some(actual) if same_grant(actual, expected, true) => (),
                _ => return Err(owner_error(Failure::ExpiredAuthorization)),
            }
        }
        // Create AND first-poll under the original owner admission fence. No
        // owner/vault guard is retained across a pending network operation.
        let mut future = transport.request(request);
        match future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            std::task::Poll::Ready(r) => ready = Some(r),
            std::task::Poll::Pending => pending = Some(future),
        }
    }
    if let Some(r) = ready {
        r
    } else {
        pending
            .ok_or_else(|| owner_error(Failure::Unconfirmed))?
            .await
    }
}
fn same_grant(a: &Authorization, b: &Authorization, version: bool) -> bool {
    a.platform == PLATFORM
        && a.platform == b.platform
        && a.domain == b.domain
        && a.uid == b.uid
        && a.grant_id == b.grant_id
        && (!version || a.credential_version == b.credential_version)
}

/// Transient flow cancellation only; no token or second account owner.
#[derive(Clone, Default)]
pub(crate) struct CancellationHandle(Arc<AtomicBool>);
impl CancellationHandle {
    /// Linearizes with the original owner's final publish. A grant already
    /// published before cancellation is not revoked by closing a login window.
    pub(crate) async fn cancel(&self) {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        self.0.store(true, Ordering::Release);
    }
    fn check(&self) -> Result<(), Failure> {
        if self.0.load(Ordering::Acquire) {
            Err(Failure::ExpiredAuthorization)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum InferenceState {
    NeedsAuthorization,
    Ready,
    RefreshRequired,
    Expired,
}
#[derive(Serialize)]
pub(crate) struct InferenceAccount {
    pub id: String,
    pub state: InferenceState,
}
/// Safe local snapshot only; Ready does not claim a successful upstream probe.
pub(crate) async fn list_inference_accounts(
    session: &SecretSession,
) -> Result<Vec<InferenceAccount>, Failure> {
    let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
    let binding = store::binding(session)?;
    let now = now_ms();
    Ok(store::load(session, &binding)?
        .iter()
        .map(|row| {
            let state = match row.inference.as_ref() {
                Some(grant) if valid_grant(grant, row) => {
                    if grant.expires_ms > now.saturating_add(EARLY_MS) {
                        InferenceState::Ready
                    } else if grant.refresh_expires_ms.is_some_and(|expiry| expiry <= now) {
                        InferenceState::Expired
                    } else {
                        InferenceState::RefreshRequired
                    }
                }
                _ => InferenceState::NeedsAuthorization,
            };
            InferenceAccount {
                id: row.account.id(&binding.vault_id),
                state,
            }
        })
        .collect())
}
fn valid_grant(grant: &Authorization, row: &SavedAccount) -> bool {
    grant.platform == PLATFORM
        && grant.uid == row.account.uid
        && grant.domain == row.account.domain
        && valid_text(&grant.grant_id, 128)
        && valid_text(&grant.access, 16384)
        && valid_text(&grant.refresh, 16384)
        && grant.credential_version != 0
}

/// Returned only by explicit authorization for a selected ORIGINAL account.
/// Keep this opaque in the original flow owner; never accept a client-created
/// serialized flow/grant. No new account or credential registry is created here.
pub(crate) struct PendingAuthorization {
    session: Arc<SecretSession>,
    owner: Owner,
    state: Zeroizing<String>,
    expires_ms: i64,
    pub verification_uri: String,
    cancellation: CancellationHandle,
}
pub(crate) enum AuthorizationProgress {
    Waiting,
    Saved,
}
impl PendingAuthorization {
    pub(crate) fn cancel_handle(&self) -> CancellationHandle {
        self.cancellation.clone()
    }
    pub(crate) async fn begin(
        session: Arc<SecretSession>,
        account_id: &str,
    ) -> Result<Self, Failure> {
        Self::begin_with(session, account_id, &Official, now_ms()).await
    }
    async fn begin_with<T: Transport>(
        session: Arc<SecretSession>,
        account_id: &str,
        t: &T,
        now: i64,
    ) -> Result<Self, Failure> {
        let owner = {
            let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
            let b = store::binding(&session)?;
            let rows = store::load(&session, &b)?;
            let row = rows
                .iter()
                .find(|r| r.account.id(&b.vault_id) == account_id)
                .ok_or(Failure::UnsupportedContext)?;
            Owner::from_row(&session, b, row)
        };
        let mut h = headers();
        for k in [
            "x-no-authorization",
            "x-no-user-id",
            "x-no-enterprise-id",
            "x-no-department-info",
        ] {
            h.insert(k, http::HeaderValue::from_static("true"));
        }
        let response = send_owner(
            &session,
            &owner,
            None,
            t,
            Request {
                endpoint: Endpoint::State,
                query: Some(("platform".into(), PLATFORM.into())),
                headers: h,
                body: b"{}".to_vec(),
                timeout: Duration::from_secs(30),
            },
        )
        .await
        .map_err(|_| Failure::Unconfirmed)?;
        let env = json_response(response).await?;
        let d = data(&env)?;
        let state = d
            .get("state")
            .and_then(Value::as_str)
            .filter(|s| valid_text(s, 4096))
            .ok_or(Failure::Unconfirmed)?;
        let mut url = reqwest::Url::parse(
            d.get("authUrl")
                .and_then(Value::as_str)
                .ok_or(Failure::Unconfirmed)?,
        )
        .map_err(|_| Failure::UnsupportedContext)?;
        if url.scheme() != "https"
            || url.host_str() != Some("copilot.tencent.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some_and(|p| p != 443)
            || url.fragment().is_some()
        {
            return Err(Failure::UnsupportedContext);
        }
        let states: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| k == "state")
            .map(|(_, v)| v.into_owned())
            .collect();
        if states != [state]
            || url.query_pairs().any(|(k, _)| {
                k.to_ascii_lowercase().contains("token")
                    || k.to_ascii_lowercase().contains("password")
            })
        {
            return Err(Failure::UnsupportedContext);
        }
        url.query_pairs_mut()
            .append_pair("version", "2.0.0")
            .append_pair("loginSessionId", &uuid::Uuid::new_v4().simple().to_string());
        {
            let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
            owner.rows(&session)?;
        }
        Ok(Self {
            session,
            owner,
            state: Zeroizing::new(state.into()),
            expires_ms: now.saturating_add(300_000),
            verification_uri: url.to_string(),
            cancellation: CancellationHandle::default(),
        })
    }
    pub(crate) async fn poll(&mut self) -> Result<AuthorizationProgress, Failure> {
        self.poll_with(&Official, now_ms()).await
    }
    async fn poll_with<T: Transport>(
        &mut self,
        t: &T,
        now: i64,
    ) -> Result<AuthorizationProgress, Failure> {
        self.cancellation.check()?;
        if now >= self.expires_ms {
            return Err(Failure::ExpiredAuthorization);
        }
        let response = send_cancellable(
            &self.session,
            &self.owner,
            None,
            t,
            Request {
                endpoint: Endpoint::Token,
                query: Some(("state".into(), self.state.to_string())),
                headers: headers(),
                body: vec![],
                timeout: Duration::from_secs(30),
            },
            Some(&self.cancellation),
        )
        .await
        .map_err(|_| Failure::Unconfirmed)?;
        let token_env = json_response(response).await?;
        {
            let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
            self.cancellation.check()?;
            self.owner.rows(&self.session)?;
        }
        if code(&token_env) == Some(11217)
            || (code(&token_env) == Some(0)
                && token_env
                    .get("data")
                    .is_some_and(|v| v.is_null() || v.as_object().is_some_and(|m| m.is_empty())))
        {
            return Ok(AuthorizationProgress::Waiting);
        }
        let d = data(&token_env)?;
        let access = Zeroizing::new(token(d, "accessToken")?);
        let domain = Domain::parse(
            d.get("domain")
                .and_then(Value::as_str)
                .ok_or(Failure::UnsupportedContext)?,
        )?;
        if domain != self.owner.domain {
            return Err(Failure::UnsupportedContext);
        }
        let mut h = headers();
        set(
            &mut h,
            "authorization",
            &format!("Bearer {}", access.as_str()),
        )?;
        set(&mut h, "x-domain", domain.host())?;
        for k in ["x-no-user-id", "x-no-enterprise-id"] {
            h.insert(k, http::HeaderValue::from_static("true"));
        }
        let response = send_cancellable(
            &self.session,
            &self.owner,
            None,
            t,
            Request {
                endpoint: Endpoint::Profile,
                query: Some(("state".into(), self.state.to_string())),
                headers: h,
                body: vec![],
                timeout: Duration::from_secs(30),
            },
            Some(&self.cancellation),
        )
        .await
        .map_err(|_| Failure::Unconfirmed)?;
        let profile = json_response(response).await?;
        {
            let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
            self.cancellation.check()?;
            self.owner.rows(&self.session)?;
        }
        if code(&profile) == Some(12151) {
            return Ok(AuthorizationProgress::Waiting);
        }
        let grant = new_grant(&self.owner, &token_env, &profile, now)?;
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        self.cancellation.check()?;
        let (mut rows, index) = self.owner.rows(&self.session)?;
        rows[index].inference = Some(grant);
        store::save(&self.session, &self.owner.binding, rows)?;
        self.state.zeroize();
        self.expires_ms = 0;
        Ok(AuthorizationProgress::Saved)
    }
}

fn refresh_lock(
    session: &SecretSession,
    owner: &Owner,
    grant: &Authorization,
) -> Result<Arc<tokio::sync::Mutex<()>>, Failure> {
    // Coordination only; no tokens, account records, or results stored here.
    static LOCKS: OnceLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let key = format!(
        "{}:{}:{}",
        session.root().display(),
        owner.account_id,
        grant.grant_id
    );
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| Failure::Busy)?;
    locks.retain(|_, v| v.strong_count() > 0);
    let lock = locks
        .get(&key)
        .and_then(Weak::upgrade)
        .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}
impl Lease {
    /// Public protocol metadata only; no account/grant data or editable URL.
    /// Dispatch itself appends its fixed /v2/chat/completions endpoint.
    pub(crate) fn base_url(&self) -> &'static str {
        ORIGIN
    }

    pub(crate) async fn acquire(
        session: Arc<SecretSession>,
        account_id: &str,
    ) -> Result<Self, Failure> {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        let binding = store::binding(&session)?;
        let rows = store::load(&session, &binding)?;
        let row = rows
            .iter()
            .find(|r| r.account.id(&binding.vault_id) == account_id)
            .ok_or(Failure::UnsupportedContext)?;
        let grant = row.inference.clone().ok_or(Failure::NeedsVerification)?;
        if !valid_grant(&grant, row) {
            return Err(Failure::NeedsVerification);
        }
        let owner = Owner::from_row(&session, binding, row);
        Ok(Self {
            session,
            owner,
            grant,
            response_blocked: Arc::new(AtomicBool::new(false)),
            response_denial: Arc::new(AtomicU32::new(0)),
        })
    }
    async fn check(&self) -> Result<(), Failure> {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        let (rows, index) = self.owner.rows(&self.session)?;
        if rows[index]
            .inference
            .as_ref()
            .is_some_and(|g| same_grant(g, &self.grant, true))
        {
            Ok(())
        } else {
            Err(Failure::ExpiredAuthorization)
        }
    }
    async fn refresh<T: Transport>(&mut self, t: &T, now: i64) -> Result<(), Failure> {
        let lock = refresh_lock(&self.session, &self.owner, &self.grant)?;
        let _refresh = lock.lock().await;
        {
            let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
            let (rows, index) = self.owner.rows(&self.session)?;
            let current = rows[index]
                .inference
                .as_ref()
                .ok_or(Failure::ExpiredAuthorization)?;
            if !same_grant(current, &self.grant, false) {
                return Err(Failure::ExpiredAuthorization);
            }
            if current.credential_version > self.grant.credential_version {
                self.grant = current.clone();
                return Ok(());
            }
            if current.credential_version != self.grant.credential_version {
                return Err(Failure::ExpiredAuthorization);
            }
        }
        if self.grant.refresh_expires_ms.is_some_and(|n| n <= now) {
            return Err(Failure::ExpiredAuthorization);
        }
        let mut h = headers();
        set(&mut h, "x-refresh-token", &self.grant.refresh)?;
        set(&mut h, "x-domain", self.grant.domain.host())?;
        h.insert(
            "x-auth-refresh-source",
            http::HeaderValue::from_static("plugin"),
        );
        let response = send_owner(
            &self.session,
            &self.owner,
            Some(&self.grant),
            t,
            Request {
                endpoint: Endpoint::Refresh,
                query: None,
                headers: h,
                body: b"{}".to_vec(),
                timeout: Duration::from_secs(30),
            },
        )
        .await
        .map_err(|_| Failure::Unconfirmed)?;
        let env = json_response(response).await?;
        let d = data(&env)?;
        if !personal(d)
            || d.get("domain")
                .is_some_and(|v| v.as_str() != Some(self.grant.domain.host()))
            || d.get("uid")
                .is_some_and(|v| v.as_str() != Some(self.grant.uid.as_str()))
            || d.get("platform")
                .is_some_and(|v| v.as_str() != Some(PLATFORM))
        {
            return Err(Failure::UnsupportedContext);
        }
        let mut next = self.grant.clone();
        next.access = token(d, "accessToken")?;
        if d.get("refreshToken").is_some() {
            next.refresh = token(d, "refreshToken")?;
        }
        next.expires_ms = expiry(d, "expiresAt", "expiresIn", now)?
            .filter(|n| *n > now)
            .ok_or(Failure::NeedsVerification)?;
        if let Some(n) = expiry(d, "refreshExpiresAt", "refreshExpiresIn", now)? {
            next.refresh_expires_ms = Some(n);
        }
        next.credential_version = next
            .credential_version
            .checked_add(1)
            .ok_or(Failure::ExpiredAuthorization)?;
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        let (mut rows, index) = self.owner.rows(&self.session)?;
        if !rows[index]
            .inference
            .as_ref()
            .is_some_and(|g| same_grant(g, &self.grant, true))
        {
            return Err(Failure::ExpiredAuthorization);
        }
        rows[index].inference = Some(next.clone());
        store::save(&self.session, &self.owner.binding, rows)?;
        self.grant = next;
        Ok(())
    }
    pub(crate) async fn discover_models(mut self) -> Result<Vec<Model>, ProxyError> {
        self.models_with(&Official, now_ms()).await
    }
    async fn models_with<T: Transport>(
        &mut self,
        t: &T,
        now: i64,
    ) -> Result<Vec<Model>, ProxyError> {
        if self.grant.expires_ms <= now.saturating_add(EARLY_MS) {
            self.refresh(t, now).await.map_err(owner_error)?;
        }
        let mut h = signed(&self.grant).map_err(owner_error)?;
        h.insert(
            "user-agent",
            http::HeaderValue::from_static("CLI/2.0.0 WorkBuddy/5.5.6"),
        );
        let response = send_owner(
            &self.session,
            &self.owner,
            Some(&self.grant),
            t,
            Request {
                endpoint: Endpoint::Models,
                query: None,
                headers: h,
                body: vec![],
                timeout: Duration::from_secs(15),
            },
        )
        .await?;
        let env = json_response(response).await.map_err(owner_error)?;
        self.check().await.map_err(owner_error)?;
        let models = parse_models(&env).map_err(owner_error)?;
        let guard = crate::secrets::stream_guard::KnownSecretGuard::new(&[
            self.grant.access.as_bytes(),
            self.grant.refresh.as_bytes(),
        ])
        .map_err(|_| owner_error(Failure::Unconfirmed))?;
        // Screen decoded fields (not JSON-escaped bytes) before ANY DTO is
        // exposed. Reuse the sole exact-known-secret algorithm; never rewrite.
        let fields: Vec<Result<Bytes, std::io::Error>> = models
            .iter()
            .flat_map(|model| {
                [
                    Ok(Bytes::copy_from_slice(model.id.as_bytes())),
                    Ok(Bytes::copy_from_slice(model.name.as_bytes())),
                ]
            })
            .collect();
        let mut screened = Box::pin(guard.wrap_stream(futures::stream::iter(fields)));
        while let Some(result) = screened.next().await {
            result.map_err(|_| owner_error(Failure::Unconfirmed))?;
        }
        Ok(models)
    }
    #[cfg(not(test))]
    pub(crate) async fn dispatch(
        self,
        body: Value,
        timeout: Duration,
    ) -> Result<DispatchResponse, ProxyError> {
        self.dispatch_with(body, timeout, &Official, now_ms()).await
    }
    async fn dispatch_with<T: Transport>(
        mut self,
        body: Value,
        timeout: Duration,
        t: &T,
        now: i64,
    ) -> Result<DispatchResponse, ProxyError> {
        // Upstream is streaming-only. The original caller may aggregate SSE for a
        // nonstreaming client, but must explicitly perform that conversion there.
        if !body.is_object()
            || body.get("n").is_some_and(|value| value.as_u64() != Some(1))
            || body.get("stream").and_then(Value::as_bool) != Some(true)
            || !body.get("messages").is_some_and(Value::is_array)
            || !body
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|s| valid_text(s, 256))
        {
            return Err(ProxyError::InvalidRequest(
                "workbuddy.chat_stream_required".into(),
            ));
        }
        let payload = serde_json::to_vec(&body)
            .map_err(|_| ProxyError::InvalidRequest("workbuddy.invalid_chat".into()))?;
        let mut refreshed = false;
        if self.grant.expires_ms <= now.saturating_add(EARLY_MS) {
            self.refresh(t, now).await.map_err(owner_error)?;
            refreshed = true;
        }
        loop {
            let mut h = signed(&self.grant).map_err(owner_error)?;
            h.insert(
                "accept",
                http::HeaderValue::from_static("text/event-stream"),
            );
            let response = send_owner(
                &self.session,
                &self.owner,
                Some(&self.grant),
                t,
                Request {
                    endpoint: Endpoint::Chat,
                    query: None,
                    headers: h,
                    body: payload.clone(),
                    timeout,
                },
            )
            .await?;
            self.check().await.map_err(owner_error)?;
            // Never classify an opaque compressed 401 as refreshable: it may
            // contain an explicit denial. Identity was requested. Reject this
            // protocol response terminally (415) rather than retrying elsewhere.
            if !response
                .headers()
                .get_all(http::header::CONTENT_ENCODING)
                .iter()
                .all(|v| {
                    v.to_str()
                        .is_ok_and(|s| s.trim().eq_ignore_ascii_case("identity"))
                })
            {
                let upstream_status = response.status().as_u16();
                return Ok(DispatchResponse {
                    response:ProxyResponse::buffered(http::StatusCode::UNSUPPORTED_MEDIA_TYPE,safe_headers(&http::HeaderMap::new(),false),Bytes::from(json!({"error":{"type":"upstream_protocol_error","code":"workbuddy_unsupported_response_encoding","upstream_status":upstream_status,"message":"WorkBuddy returned an unsupported response encoding"}}).to_string())),
                    fence:ResponseFence{lease:Arc::new(self)},
                });
            }
            if !response.status().is_success() || !response.is_sse() {
                let status = response.status();
                let retry = safe_retry_after(response.headers());
                let body_timeout = if timeout.is_zero() {
                    Duration::from_secs(30)
                } else {
                    timeout
                };
                // Headers have arrived, but SSE's long transport timeout must not
                // leave an error/JSON body read unbounded. An incomplete error may
                // conceal a denial, so fail terminally, never refresh or fail over.
                let bytes = tokio::time::timeout(body_timeout, response.bytes_with_limit(MAX_JSON))
                    .await
                    .map_err(|_| ProxyError::Internal("workbuddy.error_body_timeout".into()))?
                    .map_err(|_| ProxyError::Internal("workbuddy.invalid_error_response".into()))?;
                self.check().await.map_err(owner_error)?;
                let env: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let upstream_code = upstream_error_code(&env);
                let refusal = matches!(upstream_code, Some(11004 | 11140));
                if status == http::StatusCode::UNAUTHORIZED && !refreshed && !refusal {
                    self.refresh(t, now).await.map_err(owner_error)?;
                    refreshed = true;
                    continue;
                }
                let response = safe_error(status, upstream_code, retry, refusal);
                return Ok(DispatchResponse {
                    response,
                    fence: ResponseFence {
                        lease: Arc::new(self),
                    },
                });
            }
            // Only this private account module can supply known credential bytes.
            // The shared pure guard holds back split prefixes and TERMINATES on
            // an echo; it does not rewrite ordinary model content or refusals.
            let guard = inference_sse::ChatGuard::new(
                &[self.grant.access.as_bytes(), self.grant.refresh.as_bytes()],
                self.response_blocked.clone(),
                self.response_denial.clone(),
            )
            .map_err(|_| ProxyError::Internal("workbuddy.response_guard_unavailable".into()))?;
            let fence = ResponseFence {
                lease: Arc::new(self),
            };
            let headers = safe_headers(response.headers(), true);
            let status = response.status();
            let response = ProxyResponse::streamed(
                status,
                headers,
                fence.wrap_stream(guard.wrap_stream(response.bytes_stream())),
            );
            return Ok(DispatchResponse { response, fence });
        }
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Model {
    pub id: String,
    pub name: String,
}
fn parse_models(env: &Value) -> Result<Vec<Model>, Failure> {
    let d = data(env)?;
    let agents = d
        .get("agents")
        .and_then(Value::as_array)
        .ok_or(Failure::Unconfirmed)?;
    let mut out = vec![];
    for agent in agents
        .iter()
        .filter(|a| a.get("name").and_then(Value::as_str) == Some("cli"))
    {
        for id in agent
            .get("models")
            .and_then(Value::as_array)
            .ok_or(Failure::Unconfirmed)?
        {
            let id = id
                .as_str()
                .filter(|s| valid_text(s, 256))
                .ok_or(Failure::Unconfirmed)?;
            if out.iter().any(|m: &Model| m.id == id) {
                continue;
            }
            let name = d
                .get("models")
                .and_then(Value::as_array)
                .and_then(|ms| {
                    ms.iter()
                        .find(|m| m.get("id").and_then(Value::as_str) == Some(id))
                })
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .filter(|s| valid_text(s, 256))
                .unwrap_or(id);
            out.push(Model {
                id: id.into(),
                name: name.into(),
            });
            if out.len() > 512 {
                return Err(Failure::Unconfirmed);
            }
        }
    }
    Ok(out)
}
fn safe_retry_after(h: &http::HeaderMap) -> Option<http::HeaderValue> {
    h.get(http::header::RETRY_AFTER)
        .filter(|v| {
            v.to_str()
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .is_some_and(|n| n <= 86400)
        })
        .cloned()
}
fn safe_headers(h: &http::HeaderMap, sse: bool) -> http::HeaderMap {
    let mut out = http::HeaderMap::new();
    out.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static(if sse {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    if let Some(v) = safe_retry_after(h) {
        out.insert(http::header::RETRY_AFTER, v);
    }
    out
}
fn safe_error(
    status: http::StatusCode,
    code: Option<i64>,
    retry: Option<http::HeaderValue>,
    refusal: bool,
) -> ProxyResponse {
    let upstream_status = status.as_u16();
    let status = if status.is_success() {
        if refusal {
            http::StatusCode::FORBIDDEN
        } else {
            http::StatusCode::BAD_GATEWAY
        }
    } else {
        status
    };
    let mut headers = safe_headers(&http::HeaderMap::new(), false);
    if let Some(v) = retry {
        headers.insert(http::header::RETRY_AFTER, v);
    }
    let body = safe_error_value(upstream_status, code, refusal);
    ProxyResponse::buffered(status, headers, Bytes::from(body.to_string()))
}

fn safe_error_value(upstream_status: u16, code: Option<i64>, refusal: bool) -> Value {
    json!({"error":{"type":if refusal{"access_denied"}else{"upstream_error"},"code":code.map(|c|format!("workbuddy_{c}")),"upstream_status":upstream_status,"message":match code{Some(11004)=>"WorkBuddy refused invocation from an unapproved channel",Some(11140)=>"WorkBuddy restricted this account for unusual activity",_=>"WorkBuddy returned an upstream error"}}})
}

#[cfg(test)]
#[path = "inference_tests.rs"]
mod tests;

/// Original-router fixture seam only. A trusted extension supplies this object;
/// no client JSON/header or production settings can override upstream origins.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct FixtureTransport {
    root: std::path::PathBuf,
    binding: VaultMetadata,
    epoch: u64,
    origin: reqwest::Url,
    synthetic_grants: Vec<(String, String)>,
    client: reqwest::Client,
    attempts: Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(test)]
impl FixtureTransport {
    pub(crate) fn loopback(session: &SecretSession, origin: &str) -> Result<Self, Failure> {
        let url = reqwest::Url::parse(origin).map_err(|_| Failure::UnsupportedContext)?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Failure::UnsupportedContext);
        }
        let binding = store::binding(session)?;
        let rows = store::load(session, &binding)?;
        if rows.is_empty()
            || rows.iter().any(|row| {
                let uid = &row.account.uid;
                !uid.starts_with("synthetic-")
                    || row.account.token != format!("fixture-checkin-{uid}")
                    || !row.inference.as_ref().is_some_and(|grant| {
                        grant.uid == *uid
                            && grant.access == format!("fixture-access-{uid}")
                            && grant.refresh == format!("fixture-refresh-{uid}")
                            && grant.credential_version == 1
                    })
            })
        {
            return Err(Failure::UnsupportedContext);
        }
        let synthetic_grants = rows
            .iter()
            .map(|row| {
                (
                    row.account.id(&binding.vault_id),
                    row.inference.as_ref().unwrap().grant_id.clone(),
                )
            })
            .collect();
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| Failure::Unconfirmed)?;
        Ok(Self {
            root: session.root().into(),
            binding: store::binding(session)?,
            epoch: session.lifecycle_epoch(),
            origin: url,
            synthetic_grants,
            client,
            attempts: Default::default(),
        })
    }
    pub(crate) fn attempts(&self) -> usize {
        self.attempts.load(std::sync::atomic::Ordering::Acquire)
    }
    /// Generated synthetic credentials only, and ONLY an empty ephemeral owner.
    /// Existing accounts are never replaced by this test helper.
    pub(crate) async fn seed_accounts(
        session: &SecretSession,
        uids: &[&str],
    ) -> Result<Vec<String>, Failure> {
        let _guard = crate::services::sync_protocol::sync_mutex().lock().await;
        let binding = store::binding(session)?;
        if !store::load(session, &binding)?.is_empty() {
            return Err(Failure::UnsupportedContext);
        }
        if uids.is_empty() || uids.len() > 8 {
            return Err(Failure::UnsupportedContext);
        }
        let mut rows = vec![];
        let mut ids = vec![];
        for uid in uids {
            if !uid.starts_with("synthetic-") || !valid_text(uid, 256) {
                return Err(Failure::UnsupportedContext);
            }
            let mut row = SavedAccount::new(super::model::Account {
                uid: (*uid).into(),
                domain: Domain::WorkBuddy,
                label: (*uid).into(),
                token: format!("fixture-checkin-{uid}"),
            });
            row.inference = Some(Authorization {
                platform: PLATFORM.into(),
                domain: Domain::WorkBuddy,
                uid: (*uid).into(),
                grant_id: uuid::Uuid::new_v4().to_string(),
                credential_version: 1,
                access: format!("fixture-access-{uid}"),
                refresh: format!("fixture-refresh-{uid}"),
                expires_ms: i64::MAX,
                refresh_expires_ms: None,
            });
            let id = row.account.id(&binding.vault_id);
            if ids.contains(&id) {
                return Err(Failure::UnsupportedContext);
            }
            ids.push(id);
            rows.push(row);
        }
        store::save(session, &binding, rows)?;
        Ok(ids)
    }
}
#[cfg(test)]
impl Transport for FixtureTransport {
    fn request(
        &self,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let mut url = self.origin.clone();
            url.set_path(request.endpoint.path());
            if let Some((k, v)) = request.query {
                url.query_pairs_mut().append_pair(&k, &v);
            }
            let mut request_builder = self
                .client
                .request(request.endpoint.method(), url)
                .headers(request.headers)
                .timeout(Duration::from_secs(30));
            if !request.body.is_empty() {
                request_builder = request_builder.body(request.body);
            }
            request_builder
                .send()
                .await
                .map(ProxyResponse::Reqwest)
                .map_err(|_| {
                    ProxyError::ForwardFailed("workbuddy.synthetic_transport_failed".into())
                })
        })
    }
}
#[cfg(test)]
impl Lease {
    pub(crate) async fn dispatch_fixture(
        self,
        body: Value,
        timeout: Duration,
        fixture: &FixtureTransport,
    ) -> Result<DispatchResponse, ProxyError> {
        if self.session.root() != fixture.root
            || self.owner.binding != fixture.binding
            || self.owner.epoch != fixture.epoch
            || !fixture
                .synthetic_grants
                .iter()
                .any(|(id, grant)| id == &self.owner.account_id && grant == &self.grant.grant_id)
        {
            return Err(owner_error(Failure::UnsupportedContext));
        }
        self.dispatch_with(body, timeout, fixture, now_ms()).await
    }
}
