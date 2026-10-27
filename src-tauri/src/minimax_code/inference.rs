//! Managed MiniMax Code inference, using the existing encrypted account owner.
//!
//! Wire facts: MiniMax-AI/minimax-code 9a935605956c4500e9cb415f29258e2887e97817,
//! packages/config/src/config.ts and packages/local-runtime/src/runtime/
//! model-resolver{,-helpers}.ts. MIT notice: licenses/minimax-code-MIT.txt.
//! `sk-xxx` in that source is an SDK placeholder, never an account credential.
#[cfg(not(test))]
use super::http::OfficialHttp;
use super::{
    auth,
    catalog::{self, Failure, Snapshot},
    http::Transport,
};
use crate::{
    proxy::{hyper_client::ProxyResponse, ProxyError},
    secrets::{session::SecretSession, stream_guard::KnownSecretGuard},
};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
#[path = "inference_sse.rs"]
mod inference_sse;

/// An in-memory, non-serializable capability for one selected account incarnation.
/// The caller must resolve account_id from its trusted selected pool row, never
/// from request JSON. request_session_id provides correlation, not authorization.
/// No credential, URL override, provider settings, or global current account is
/// exposed through this API.
pub(crate) struct Lease {
    session: Arc<SecretSession>,
    snapshot: Snapshot,
    upstream_session_id: String,
}

/// The retry owner must retain `fence` through any priming, buffering and
/// protocol conversion, then apply it to the FINAL downstream body stream.
/// An upstream-only wrapper cannot invalidate bytes another layer buffered.
pub(crate) struct DispatchResponse {
    pub response: ProxyResponse,
    pub fence: ResponseFence,
}

/// Cloneable, non-serializable account-generation fence. This is a read-only
/// capability over the original owner, not another credential/state store.
#[derive(Clone)]
pub(crate) struct ResponseFence {
    lease: Arc<Lease>,
    // Shared with the upstream observer and retained through protocol conversion.
    incomplete: Arc<AtomicBool>,
}
impl ResponseFence {
    pub(crate) async fn check(&self) -> Result<(), ProxyError> {
        if self.incomplete.load(Ordering::Acquire) {
            return Err(ProxyError::ForwardFailed(
                "minimax_code.incomplete_stream".into(),
            ));
        }
        self.lease.check().await.map_err(owner_error)
    }

    pub(crate) fn wrap_stream<S, E>(
        &self,
        stream: S,
    ) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static
    where
        S: futures::Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Send + 'static,
    {
        let fence = self.clone();
        let stream = Box::pin(stream);
        futures::stream::unfold(
            (fence, stream, false),
            |(fence, mut stream, stopped)| async move {
                if stopped {
                    return None;
                }
                if fence.check().await.is_err() {
                    return Some((
                        Err(std::io::Error::other(
                            "minimax_code.stream_authorization_changed",
                        )),
                        (fence, stream, true),
                    ));
                }
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        if fence.check().await.is_err() {
                            Some((
                                Err(std::io::Error::other(
                                    "minimax_code.stream_authorization_changed",
                                )),
                                (fence, stream, true),
                            ))
                        } else {
                            Some((Ok(bytes), (fence, stream, false)))
                        }
                    }
                    Some(Err(_)) => Some((
                        Err(std::io::Error::other("minimax_code.stream_interrupted")),
                        (fence, stream, true),
                    )),
                    None => None,
                }
            },
        )
    }
}

pub(super) struct StreamRequest {
    pub url: &'static str,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
    pub streaming: bool,
    pub timeout: Duration,
}

/// Kept inside the account module. Tests replace only the network boundary,
/// while exercising the production encrypted owner, refresh CAS and fences.
pub(super) trait StreamTransport: Send + Sync {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>>;
}

#[cfg(not(test))]
struct OfficialStreamTransport;
#[cfg(not(test))]
impl StreamTransport for OfficialStreamTransport {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            // Reuse the existing no-redirect/no-protocol-retry profile, including
            // configured HTTP/SOCKS proxies and TLS. The ordinary authenticated
            // profile permits protocol retries. Hyper's raw-write fallback can
            // issue a second send. Neither is appropriate for this POST owner.
            // This profile name does not introduce pricing/billing admission.
            let client = crate::proxy::http_client::get_budgeted_authenticated().map_err(|_| {
                ProxyError::ConfigError("minimax_code.transport_unavailable".into())
            })?;
            let mut builder = client
                .post(request.url)
                .headers(request.headers)
                .body(request.body);
            if request.streaming {
                // The original response processor owns first-byte/idle timeouts
                // after headers. reqwest's total timeout must not cut long SSE.
                builder = builder.timeout(Duration::from_secs(24 * 60 * 60));
            } else if !request.timeout.is_zero() {
                builder = builder.timeout(request.timeout);
            }
            let send = builder.send();
            let response = if request.timeout.is_zero() {
                send.await
            } else {
                tokio::time::timeout(request.timeout, send)
                    .await
                    .map_err(|_| {
                        ProxyError::Timeout("minimax_code.response_headers_timeout".into())
                    })?
            }
            .map_err(|_| ProxyError::ForwardFailed("minimax_code.transport_failed".into()))?;
            Ok(ProxyResponse::Reqwest(response))
        })
    }
}

fn owner_error(_: Failure) -> ProxyError {
    // Do not format credentials, HTTP bodies, filesystem paths or transport
    // errors into diagnostics. The account UI can read its existing safe DTO.
    ProxyError::AuthError("minimax_code.authorization_unavailable".into())
}

fn refresh_error(error: Failure) -> ProxyError {
    if error == Failure::NetworkUnavailable {
        // Refresh may already have rotated an upstream credential. Preserve the
        // original managed-account unknown-outcome policy: no account-health
        // penalty, no failover/replay, and no misleading downstream HTTP 401.
        ProxyError::ForwardFailed("minimax_code.refresh_outcome_unknown".into())
    } else {
        owner_error(error)
    }
}

fn post_dispatch_owner_error(_: Failure) -> ProxyError {
    // The upstream request has already completed its send. An invalidated
    // original owner cannot prove non-acceptance; do not classify this as a
    // recoverable pre-send authentication failure or select another account.
    ProxyError::Internal("minimax_code.authorization_changed_after_dispatch".into())
}

fn check_snapshot(document: &mut catalog::Document, snapshot: &Snapshot) -> Result<(), Failure> {
    let account = catalog::matched(document, snapshot)?;
    account.usable()?;
    if account.credential_version != snapshot.account.credential_version {
        return Err(Failure::StaleIdentity);
    }
    Ok(())
}

impl Lease {
    pub(crate) async fn acquire(
        session: Arc<SecretSession>,
        account_id: &str,
        request_session_id: &str,
    ) -> Result<Self, Failure> {
        if request_session_id.trim().is_empty()
            || request_session_id.len() > 1024
            || request_session_id.chars().any(char::is_control)
        {
            return Err(Failure::UnsupportedContext);
        }
        let snapshot = catalog::snapshot(&session, account_id).await?;
        let credentials = snapshot
            .account
            .credentials
            .as_ref()
            .ok_or(Failure::NeedsLogin)?;
        if credentials.access_token.trim() == "sk-xxx" {
            return Err(Failure::NeedsLogin);
        }
        // Account-bound session correlation is a LoongPort adaptation. Never
        // forward an untrusted session value directly or share the same upstream
        // session across two selected accounts or authorization incarnations.
        let context = serde_json::to_vec(&(
            "loongport.minimax.inference.v1",
            &snapshot.account.id,
            snapshot.account.auth_generation,
            request_session_id,
        ))
        .map_err(|_| Failure::InvalidResponse)?;
        let upstream_session_id = format!("loongport-{}", hex::encode(Sha256::digest(context)));
        Ok(Self {
            session,
            snapshot,
            upstream_session_id,
        })
    }

    /// Bind an explicit shared-route snapshot to this original owner capability.
    /// Normal same-account credential rotation remains owned by the refresh path;
    /// it is not an account replacement and does not change this source identity.
    pub(crate) fn verify_routing_stamp(
        &self,
        expected: &crate::services::service_source_identity::MiniMaxStamp,
    ) -> Result<(), ProxyError> {
        let account = &self.snapshot.account;
        let binding = &self.snapshot.binding;
        let region = match account.region {
            catalog::Region::Cn => "cn",
            catalog::Region::Global => "global",
        };
        if expected.account_id != account.id
            || expected.region != region
            || expected.auth_generation != account.auth_generation
            || expected.real_user_id != account.real_user_id
            || expected.build_env != account.build_env
            || expected.vault_id != binding.vault.store_id()
            || expected.storage != binding.vault
            || expected.session_epoch != binding.session_epoch
            || expected.process_epoch != binding.process_epoch
        {
            return Err(ProxyError::AuthError(
                "minimax_code.routing_identity_changed".into(),
            ));
        }
        Ok(())
    }

    /// Non-secret, fixed routing metadata for the original forwarder's URL
    /// construction/conversion. This cannot override the dispatch destination.
    pub(crate) fn base_url(&self) -> &'static str {
        match self.snapshot.account.region {
            catalog::Region::Cn => "https://agent.minimax.cn/mavis/api/v1/llm/v1",
            catalog::Region::Global => "https://agent.minimax.io/mavis/api/v1/llm/v1",
        }
    }

    /// Accept an already-converted Anthropic Messages body from the original
    /// forwarder. Preserve its complete messages/tools/thinking payload. This
    /// adapter performs no parallel protocol conversion and no pool selection.
    #[cfg(not(test))]
    pub(crate) async fn dispatch(
        self,
        body: Value,
        timeout: Duration,
    ) -> Result<DispatchResponse, ProxyError> {
        self.dispatch_with(body, timeout, &OfficialHttp, &OfficialStreamTransport)
            .await
    }

    pub(super) async fn dispatch_with<T: Transport, S: StreamTransport>(
        mut self,
        body: Value,
        timeout: Duration,
        auth_transport: &T,
        stream_transport: &S,
    ) -> Result<DispatchResponse, ProxyError> {
        if !body.is_object()
            || !body.get("messages").is_some_and(Value::is_array)
            || body
                .get("model")
                .and_then(Value::as_str)
                .is_none_or(|m| m.trim().is_empty())
            || body.get("stream").is_some_and(|v| !v.is_boolean())
        {
            return Err(ProxyError::InvalidRequest(
                "minimax_code.messages_required".into(),
            ));
        }
        let mut refreshed = false;
        if self.snapshot.account.credentials.as_ref().is_none_or(|c| {
            c.kind == catalog::CredentialKind::DeviceOAuth
                && c.expires_at <= catalog::now_ms().saturating_add(60_000)
        }) {
            self.snapshot = auth::refresh_snapshot(&self.session, auth_transport, &self.snapshot)
                .await
                .map_err(refresh_error)?;
            refreshed = true;
        }
        loop {
            let request = self.build_request(&body, timeout).map_err(owner_error)?;
            let response = self.send_once(stream_transport, request).await?;
            let (response, denied) = if response.status().is_success() {
                (response, false)
            } else {
                safe_error_response(response, timeout).await?
            };
            self.check().await.map_err(post_dispatch_owner_error)?;
            // Only an explicit HTTP 401 before any downstream output allows one
            // same-account refresh/replay, after inspecting its bounded error
            // envelope. No downstream output has been emitted. Expiry refresh
            // consumes the same one-refresh budget.
            if response.status() == http::StatusCode::UNAUTHORIZED && !refreshed && !denied {
                drop(response);
                self.snapshot =
                    auth::refresh_snapshot(&self.session, auth_transport, &self.snapshot)
                        .await
                        .map_err(refresh_error)?;
                refreshed = true;
                continue;
            }
            self.check().await.map_err(post_dispatch_owner_error)?;
            if !response.status().is_success() {
                // Preserve the status for the ORIGINAL retry owner. 429 and a
                // second 401 must never be treated as successful completions.
                // The bounded classification above discarded raw upstream text;
                // only its safe status and allowlisted denial category remain.
                let fence = ResponseFence {
                    lease: Arc::new(self),
                    incomplete: Arc::default(),
                };
                return Ok(DispatchResponse { response, fence });
            }
            let fence = ResponseFence {
                lease: Arc::new(self),
                incomplete: Arc::default(),
            };
            let response = fence.lease.clone().fence_response(
                response,
                body.get("stream").and_then(Value::as_bool).unwrap_or(false),
                &fence,
            )?;
            return Ok(DispatchResponse { response, fence });
        }
    }

    fn build_request(&self, body: &Value, timeout: Duration) -> Result<StreamRequest, Failure> {
        let credential = self
            .snapshot
            .account
            .credentials
            .as_ref()
            .ok_or(Failure::NeedsLogin)?;
        if credential.access_token.trim() == "sk-xxx" {
            return Err(Failure::NeedsLogin);
        }
        let mut authorization =
            http::HeaderValue::from_str(&format!("Bearer {}", credential.access_token))
                .map_err(|_| Failure::InvalidResponse)?;
        authorization.set_sensitive(true);
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::AUTHORIZATION, authorization);
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            http::header::ACCEPT,
            http::HeaderValue::from_static("application/json, text/event-stream"),
        );
        // Exact-secret screening operates on identity bytes, never compressed
        // content. The shared client disables automatic decompression.
        headers.insert(
            http::header::ACCEPT_ENCODING,
            http::HeaderValue::from_static("identity"),
        );
        headers.insert(
            http::header::USER_AGENT,
            http::HeaderValue::from_static("MiniMaxAgent"),
        );
        headers.insert(
            "anthropic-version",
            http::HeaderValue::from_static("2023-06-01"),
        );
        headers.insert(
            "x-mavis-session-id",
            http::HeaderValue::from_str(&self.upstream_session_id)
                .map_err(|_| Failure::InvalidResponse)?,
        );
        // Official readAgentHeaderId fallback for a main-agent invocation.
        headers.insert("x-mavis-agent-id", http::HeaderValue::from_static("main"));
        // Official JS getTimezoneOffset() * -60: seconds east of UTC, not minutes.
        headers.insert(
            "x-mavis-timezone-offset",
            http::HeaderValue::from_str(
                &chrono::Local::now().offset().local_minus_utc().to_string(),
            )
            .map_err(|_| Failure::InvalidResponse)?,
        );
        let url = match self.snapshot.account.region {
            catalog::Region::Cn => "https://agent.minimax.cn/mavis/api/v1/llm/v1/messages",
            catalog::Region::Global => "https://agent.minimax.io/mavis/api/v1/llm/v1/messages",
        };
        Ok(StreamRequest {
            url,
            headers,
            body: serde_json::to_vec(body).map_err(|_| Failure::InvalidResponse)?,
            streaming: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
            timeout,
        })
    }

    async fn check(&self) -> Result<(), Failure> {
        catalog::access(
            &self.session,
            Some(&self.snapshot.binding),
            |document, _| {
                check_snapshot(document, &self.snapshot)?;
                Ok(((), false))
            },
        )
        .await
    }

    async fn send_once<S: StreamTransport>(
        &self,
        transport: &S,
        request: StreamRequest,
    ) -> Result<ProxyResponse, ProxyError> {
        // Match catalog::dispatch's existing admission linearization: create AND
        // first-poll the network future under the existing owner/file/session/
        // Runtime fence. Await only after all guards have been released. This
        // guarantees admission ordering, not that bytes are already on the wire.
        let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
        let mut pending = None;
        let mut ready = None;
        catalog::access(
            &self.session,
            Some(&self.snapshot.binding),
            |document, _| {
                check_snapshot(document, &self.snapshot)?;
                let mut future = transport.request(request);
                match future
                    .as_mut()
                    .poll(&mut std::task::Context::from_waker(&waker))
                {
                    std::task::Poll::Ready(response) => ready = Some(response),
                    std::task::Poll::Pending => pending = Some(future),
                }
                Ok(((), false))
            },
        )
        .await
        .map_err(owner_error)?;
        if let Some(response) = ready {
            response
        } else {
            pending
                .ok_or_else(|| owner_error(Failure::StorageUnavailable))?
                .await
        }
    }

    fn fence_response(
        self: Arc<Self>,
        response: ProxyResponse,
        streaming: bool,
        fence: &ResponseFence,
    ) -> Result<ProxyResponse, ProxyError> {
        if response
            .headers()
            .get_all(http::header::CONTENT_ENCODING)
            .iter()
            .any(|value| {
                !value
                    .to_str()
                    .is_ok_and(|value| value.trim().eq_ignore_ascii_case("identity"))
            })
        {
            return Err(ProxyError::ForwardFailed(
                "minimax_code.unsupported_content_encoding".into(),
            ));
        }
        let credentials = self
            .snapshot
            .account
            .credentials
            .as_ref()
            .ok_or_else(|| owner_error(Failure::NeedsLogin))?;
        let mut secrets = vec![credentials.access_token.as_bytes()];
        if let Some(refresh) = &credentials.refresh_token {
            secrets.push(refresh.as_bytes());
        }
        let guard =
            KnownSecretGuard::new(&secrets).map_err(|_| owner_error(Failure::InvalidResponse))?;
        let status = response.status();
        let headers = safe_success_headers(response.headers());
        let observed = inference_sse::observe_completion(
            guard.wrap_stream(response.bytes_stream()),
            streaming,
            fence.incomplete.clone(),
        );
        let fenced = fence.wrap_stream(observed);
        Ok(ProxyResponse::streamed(status, headers, fenced))
    }
}

fn safe_success_headers(original: &http::HeaderMap) -> http::HeaderMap {
    // Never copy arbitrary upstream headers: even a 200 response can echo an
    // Authorization value, a session identifier, or a credential cookie.
    let mut headers = http::HeaderMap::new();
    let content_type = original
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim);
    if let Some(value) = content_type.and_then(|value| {
        if value.eq_ignore_ascii_case("text/event-stream") {
            Some("text/event-stream")
        } else if value.eq_ignore_ascii_case("application/json")
            || value.to_ascii_lowercase().ends_with("+json")
        {
            Some("application/json")
        } else if value.eq_ignore_ascii_case("application/octet-stream") {
            Some("application/octet-stream")
        } else {
            None
        }
    }) {
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static(value),
        );
    }
    headers
}

async fn safe_error_response(
    response: ProxyResponse,
    timeout: Duration,
) -> Result<(ProxyResponse, bool), ProxyError> {
    let status = response.status();
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    if let Some(value) = response
        .headers()
        .get(http::header::RETRY_AFTER)
        .filter(|v| {
            v.to_str()
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .is_some_and(|n| n <= 86_400)
        })
    {
        headers.insert(http::header::RETRY_AFTER, value.clone());
    }
    // Inspect only a bounded error envelope, never return its raw text. This
    // runs before refresh/rejection so an explicit denial cannot mutate auth
    // state or be replayed under another account by the existing router.
    if !response
        .headers()
        .get_all(http::header::CONTENT_ENCODING)
        .iter()
        .all(|v| {
            v.to_str()
                .is_ok_and(|s| s.trim().eq_ignore_ascii_case("identity"))
        })
    {
        return Err(ProxyError::Internal(
            "minimax_code.error_encoding_unsupported".into(),
        ));
    }
    let declared_json = response.is_json();
    let deadline = if timeout.is_zero() {
        Duration::from_secs(30)
    } else {
        timeout.min(Duration::from_secs(30))
    };
    let bytes = tokio::time::timeout(deadline, response.bytes_with_limit(64 * 1024))
        .await
        .map_err(|_| ProxyError::Internal("minimax_code.error_body_timeout".into()))?
        .map_err(|_| ProxyError::Internal("minimax_code.error_body_unreadable".into()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ProxyError::Internal("minimax_code.error_body_undecodable".into()))?;
    let envelope: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_)
            if !text.trim().is_empty()
                && (declared_json || text.trim_start().starts_with(['{', '['])) =>
        {
            return Err(ProxyError::Internal(
                "minimax_code.error_json_invalid".into(),
            ));
        }
        // Legacy empty or plain-text/HTML gateway errors retain status semantics.
        // Their prose is never interpreted as permission or safety evidence.
        Err(_) => serde_json::Value::Null,
    };
    // Same exact structured categories recognized by the original forwarder.
    // Arbitrary message text, status alone and quota/rate codes are not denials.
    let denied = envelope
        .get("error")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|error| {
            ["type", "code"].into_iter().any(|field| {
                matches!(
                    error.get(field).and_then(serde_json::Value::as_str),
                    Some(
                        "permission_error"
                            | "permission_denied"
                            | "access_denied"
                            | "safety_error"
                            | "content_policy_violation"
                            | "content_filter"
                    )
                )
            })
        });
    let error_type = if denied {
        "access_denied"
    } else if status == http::StatusCode::TOO_MANY_REQUESTS {
        "rate_limit_error"
    } else if status == http::StatusCode::UNAUTHORIZED {
        "authentication_error"
    } else {
        "api_error"
    };
    let body = serde_json::json!({"type":"error","error":{"type":error_type,"message":"MiniMax Code upstream returned an error"}});
    Ok((
        ProxyResponse::buffered(status, headers, Bytes::from(body.to_string())),
        denied,
    ))
}

/// A test-only capability issued after synthetic seeding. Neither a real vault
/// reference nor an arbitrary request can manufacture this private payload.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) struct FixtureOwner {
    root: std::path::PathBuf,
    binding: catalog::Binding,
    account_ids: Arc<Vec<String>>,
}

/// Request-extension fixture only. Production has no endpoint override or
/// alternate account resolver. The shared router installs this trusted value;
/// no HTTP header or request JSON can select it.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone)]
pub(crate) struct FixtureTransport {
    root: std::path::PathBuf,
    binding: catalog::Binding,
    account_ids: Arc<Vec<String>>,
    origin: String,
    client: reqwest::Client,
    attempts: Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(any(test, feature = "test-hooks"))]
impl FixtureTransport {
    pub(crate) async fn loopback(owner: &FixtureOwner, origin: &str) -> Result<Self, Failure> {
        let url = url::Url::parse(origin).map_err(|_| Failure::UnsupportedContext)?;
        if url.scheme() != "http"
            || !url
                .host_str()
                .and_then(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .is_some_and(|ip| ip.is_loopback())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(Failure::UnsupportedContext);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Failure::NetworkUnavailable)?;
        Ok(Self {
            root: owner.root.clone(),
            binding: owner.binding.clone(),
            account_ids: owner.account_ids.clone(),
            origin: origin.trim_end_matches('/').to_owned(),
            client,
            attempts: Arc::default(),
        })
    }
    pub(crate) fn attempts(&self) -> usize {
        self.attempts.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub(super) fn matches(&self, lease: &Lease) -> bool {
        self.account_ids.contains(&lease.snapshot.account.id)
            && lease.snapshot.account.auth_generation == 1
            && lease
                .snapshot
                .account
                .credentials
                .as_ref()
                .is_some_and(|credentials| {
                    credentials.access_token.starts_with("synthetic-")
                        && (credentials
                            .refresh_token
                            .as_deref()
                            .is_some_and(|r| r.starts_with("synthetic-"))
                            || (lease.snapshot.account.id == "synthetic-native-a"
                                && credentials.access_token == "synthetic-native-a-access"
                                && credentials.kind == catalog::CredentialKind::NativeAccessOnly
                                && credentials.refresh_token.is_none()
                                && credentials.native_source.as_ref().is_some_and(|source| {
                                    source.format == catalog::NativeSourceFormat::PublicCliOAuthV1
                                        && source.generation == 1
                                        && source.fingerprint == "a".repeat(64)
                                        && !source.rejected
                                })))
                })
            && self.binding == lease.snapshot.binding
            && lease
                .session
                .root()
                .canonicalize()
                .is_ok_and(|root| root == self.root)
    }
    fn url(&self, region: catalog::Region, path: &str) -> String {
        let region = match region {
            catalog::Region::Cn => "cn",
            catalog::Region::Global => "global",
        };
        format!("{}/{region}{path}", self.origin)
    }
    /// Seed generated synthetic identities only into an EMPTY fixture owner.
    /// A populated real owner cannot issue the capability required by loopback.
    pub(crate) async fn seed_accounts(
        session: &SecretSession,
        accounts: &[(&str, catalog::Region)],
    ) -> Result<FixtureOwner, Failure> {
        let root = session
            .root()
            .canonicalize()
            .map_err(|_| Failure::StorageUnavailable)?;
        catalog::access(session, None, |document, binding| {
            if !document.accounts.is_empty() || accounts.is_empty() {
                return Err(Failure::Busy);
            }
            for (id, region) in accounts {
                if !id.starts_with("synthetic-") {
                    return Err(Failure::UnsupportedContext);
                }
                document.accounts.push(catalog::Account {
                    auto_claim_enabled: false,
                    auto_claim_revision: 0,
                    auto_claim_failure: None,
                    id: (*id).to_owned(),
                    label: "Synthetic account".into(),
                    region: *region,
                    build_env: "prod".into(),
                    real_user_id: format!("{id}-user"),
                    auth_generation: 1,
                    credential_version: 1,
                    credentials: Some(catalog::Credentials {
                        access_token: format!("{id}-access"),
                        refresh_token: Some(format!("{id}-refresh")),
                        kind: catalog::CredentialKind::DeviceOAuth,
                        native_source: None,
                        expires_at: i64::MAX,
                    }),
                    status: catalog::Status::Claimable,
                    operation: None,
                    last_checked_at: None,
                    last_observed_status: None,
                });
            }
            Ok((
                FixtureOwner {
                    root,
                    binding: binding.clone(),
                    account_ids: Arc::new(
                        accounts.iter().map(|(id, _)| (*id).to_owned()).collect(),
                    ),
                },
                true,
            ))
        })
        .await
    }
}
#[cfg(any(test, feature = "test-hooks"))]
impl StreamTransport for FixtureTransport {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            if !request
                .headers
                .get(http::header::AUTHORIZATION)
                .and_then(|header| header.to_str().ok())
                .is_some_and(|header| header.starts_with("Bearer synthetic-"))
            {
                return Err(owner_error(Failure::UnsupportedContext));
            }
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let region = if request.url.starts_with("https://agent.minimax.cn/") {
                catalog::Region::Cn
            } else {
                catalog::Region::Global
            };
            let mut outbound = self
                .client
                .post(self.url(region, "/mavis/api/v1/llm/v1/messages"))
                .headers(request.headers)
                .body(request.body);
            if !request.timeout.is_zero() {
                outbound = outbound.timeout(request.timeout);
            }
            let response = outbound.send().await.map_err(|_| {
                ProxyError::ForwardFailed("minimax_code.fixture_transport_failed".into())
            })?;
            Ok(ProxyResponse::Reqwest(response))
        })
    }
}
#[cfg(any(test, feature = "test-hooks"))]
impl Transport for FixtureTransport {
    fn request(
        &self,
        request: super::http::Request,
    ) -> Pin<Box<dyn Future<Output = Result<super::http::Response, Failure>> + Send + '_>> {
        Box::pin(async move {
            match request.endpoint {
                super::http::Endpoint::Refresh
                    if request
                        .body
                        .as_ref()
                        .and_then(|body| body.get("refresh_token"))
                        .and_then(Value::as_str)
                        .is_some_and(|token| token.starts_with("synthetic-")) => {}
                super::http::Endpoint::Profile
                    if request
                        .access_token
                        .as_deref()
                        .is_some_and(|token| token.starts_with("synthetic-")) => {}
                _ => return Err(Failure::UnsupportedContext),
            }
            let region = request.region;
            let mut wire = super::http::build_request(
                &self.client,
                request,
                catalog::now_ms(),
                chrono::Local::now().offset().local_minus_utc(),
                std::env::consts::OS,
                env!("CARGO_PKG_VERSION"),
            )?;
            // Preserve original auth method/headers/query/form bytes. Only the
            // destination and region prefix change for this synthetic fixture.
            let mut url = url::Url::parse(&self.url(region, wire.url().path()))
                .map_err(|_| Failure::UnsupportedContext)?;
            url.set_query(wire.url().query());
            *wire.url_mut() = url;
            self.attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            super::http::send_once(&self.client, wire).await
        })
    }
}
#[cfg(any(test, feature = "test-hooks"))]
impl Lease {
    pub(crate) async fn dispatch_fixture(
        self,
        body: Value,
        timeout: Duration,
        fixture: &FixtureTransport,
    ) -> Result<DispatchResponse, ProxyError> {
        if !fixture.matches(&self) {
            return Err(owner_error(Failure::StaleIdentity));
        }
        self.dispatch_with(body, timeout, fixture, fixture).await
    }
}
