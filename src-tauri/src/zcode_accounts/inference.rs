//! Start Plan inference from the existing encrypted ZCode catalog.
//!
//! Protocol: zai-org/ZCode 29628c9acdb81b703bbd4080c207a0e7ce5e276e,
//! accountProviderRequestAuthService.ts, zcode-builtin.json, model-execution.ts;
//! @ai-sdk/anthropic 3.0.81 anthropic-provider.ts. The Start JWT supplies BOTH
//! x-api-key and Bearer Authorization. No refresh exchange or CodingKey creation.
//! Preserve the upstream Apache-2.0 LICENSE and NOTICE with distribution.
use super::{
    core::AccountSnapshot,
    library_context::LibraryContext,
    native::NativeCipher,
    oauth_account::saved_session_binding,
    session_checks::{claim_jwt, SessionCheckReport},
    transaction::VaultAccountStore,
};
use crate::{
    proxy::{hyper_client::ProxyResponse, ProxyError},
    secrets::{session::SecretSession, stream_guard::KnownSecretGuard},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;
#[cfg(not(test))]
use std::path::Path;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

const START_MESSAGES_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    NeedsLogin,
    StaleIdentity,
    UnsupportedContext,
    StorageUnavailable,
}

/// Non-secret, non-serializable request capability. Dispatch consumes this
/// authority and returns a separate read-only response fence; an acquired lease
/// is not automatically advanced by a response-owned catalog transition.
#[derive(Clone)]
pub(crate) struct Lease {
    session: Arc<SecretSession>,
    context: LibraryContext,
    binding: Binding,
}
/// Retain this owner-issued fence through final buffering, priming and conversion.
pub(crate) struct DispatchResponse {
    pub response: ProxyResponse,
    pub fence: ResponseFence,
}

/// Read-only authority for this response. Only a verified rejection performed
/// by this dispatch may replace its catalog revision; request leases stay stale.
#[derive(Clone)]
pub(crate) struct ResponseFence {
    lease: Lease,
}
impl ResponseFence {
    pub(crate) async fn check(&self) -> Result<(), Failure> {
        self.lease.check().await
    }
}

#[derive(Clone)]
struct Binding {
    vault_id: String,
    key_id: String,
    vault_revision: u64,
    epoch: u64,
    catalog_revision: String,
    account_id: String,
    session_binding: [u8; 32],
}

struct StreamRequest {
    url: &'static str,
    headers: http::HeaderMap,
    body: Vec<u8>,
    streaming: bool,
    timeout: Duration,
}
trait StreamTransport: Send + Sync {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>>;
}
#[cfg(not(test))]
struct OfficialTransport;
#[cfg(not(test))]
impl StreamTransport for OfficialTransport {
    fn request(
        &self,
        request: StreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            // One authenticated POST, no redirects or protocol replay. Reuse
            // LoongPort's configured proxy/TLS profile, not a second client stack.
            let client = crate::proxy::http_client::get_budgeted_authenticated().map_err(|_| {
                ProxyError::ConfigError("zcode.inference.transport_unavailable".into())
            })?;
            let mut builder = client
                .post(request.url)
                .headers(request.headers)
                .body(request.body);
            if request.streaming {
                builder = builder.timeout(Duration::from_secs(24 * 60 * 60));
            } else if !request.timeout.is_zero() {
                builder = builder.timeout(request.timeout);
            }
            let send = builder.send();
            let result = if request.timeout.is_zero() {
                send.await
            } else {
                tokio::time::timeout(request.timeout, send)
                    .await
                    .map_err(|_| ProxyError::Timeout("zcode.inference.headers_timeout".into()))?
            };
            result
                .map(ProxyResponse::Reqwest)
                .map_err(|_| ProxyError::ForwardFailed("zcode.inference.transport_failed".into()))
        })
    }
}
fn now_seconds() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}
fn owner_error(error: Failure) -> ProxyError {
    ProxyError::AuthError(
        match error {
            Failure::NeedsLogin => "zcode.inference.needs_login",
            Failure::StaleIdentity => "zcode.inference.authorization_changed",
            Failure::UnsupportedContext => "zcode.inference.unsupported_context",
            Failure::StorageUnavailable => "zcode.inference.owner_unavailable",
        }
        .into(),
    )
}

/// Parsing exp can reject an expired credential; it never authenticates a JWT.
/// Opaque tokens/unknown expiry still require an actual upstream response.
fn jwt_is_expired(token: &str, now: u64) -> bool {
    let mut pieces = token.split('.');
    let (Some(_), Some(payload), Some(_)) = (pieces.next(), pieces.next(), pieces.next()) else {
        return false;
    };
    if pieces.next().is_some() || payload.len() > 32 * 1024 {
        return false;
    }
    URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|claims| claims.get("exp").and_then(Value::as_u64))
        .is_some_and(|expiry| expiry <= now)
}

impl Lease {
    /// Resolve the same default library scope used by the existing account UI.
    /// Non-macOS production remains unsupported by that owner; never fabricate
    /// OS identity or silently pick another catalog to make acquisition succeed.
    #[cfg(not(test))]
    pub(crate) async fn acquire(
        session: Arc<SecretSession>,
        account_id: &str,
        request_session_id: &str,
    ) -> Result<Self, Failure> {
        Self::acquire_selected(session, account_id, request_session_id, None).await
    }

    /// selected_data_root may come only from the trusted backend account/library
    /// selection. Never derive it from inference JSON or provider credential fields.
    /// It is a ZCode namespace, distinct from session.root() (LoongPort's vault).
    #[cfg(not(test))]
    pub(crate) async fn acquire_selected(
        session: Arc<SecretSession>,
        account_id: &str,
        request_session_id: &str,
        selected_data_root: Option<&Path>,
    ) -> Result<Self, Failure> {
        let context = super::native_context::library_context(selected_data_root)
            .map_err(|_| Failure::UnsupportedContext)?;
        Self::acquire_in_context(session, context, account_id, request_session_id).await
    }

    async fn acquire_in_context(
        session: Arc<SecretSession>,
        context: LibraryContext,
        account_id: &str,
        request_session_id: &str,
    ) -> Result<Self, Failure> {
        if account_id.len() != 64
            || !account_id.bytes().all(|b| b.is_ascii_hexdigit())
            || request_session_id.trim().is_empty()
            || request_session_id.len() > 1024
            || request_session_id.chars().any(char::is_control)
        {
            return Err(Failure::UnsupportedContext);
        }
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let vault = session.read().map_err(|_| Failure::StorageUnavailable)?;
        let native = context.cipher().map_err(|_| Failure::UnsupportedContext)?;
        let store = VaultAccountStore::new(session.root(), &vault)
            .map_err(|_| Failure::StorageUnavailable)?;
        let status = store
            .catalog_status(&native)
            .map_err(|_| Failure::UnsupportedContext)?;
        if status.pending || status.native_unconfirmed {
            return Err(Failure::StaleIdentity);
        }
        let catalog = store
            .import_catalog(&native, &status.revision)
            .map_err(|_| Failure::StaleIdentity)?;
        let snapshot = catalog
            .profiles()
            .find(|s| s.identity().opaque_id() == account_id)
            .ok_or(Failure::StaleIdentity)?;
        let evidence = catalog.details(snapshot, &native).evidence;
        if evidence
            .as_ref()
            .is_some_and(SessionCheckReport::start_needs_login)
        {
            return Err(Failure::NeedsLogin);
        }
        let token = claim_jwt(&native, snapshot).map_err(|_| Failure::NeedsLogin)?;
        if jwt_is_expired(token.expose(), now_seconds()) {
            let report = SessionCheckReport::start_auth_rejected(
                &native,
                snapshot,
                evidence.as_ref(),
                now_seconds(),
            );
            store
                .update_profile_evidence(&native, &status.revision, account_id, report)
                .map_err(|_| Failure::StorageUnavailable)?;
            return Err(Failure::NeedsLogin);
        }
        let meta = vault.metadata();
        let binding = Binding {
            vault_id: meta.vault_id.clone(),
            key_id: meta.key_id.clone(),
            vault_revision: meta.revision,
            epoch: session.lifecycle_epoch(),
            catalog_revision: status.revision,
            account_id: account_id.into(),
            session_binding: saved_session_binding(&native, snapshot)
                .map_err(|_| Failure::NeedsLogin)?,
        };
        drop(vault);
        Ok(Self {
            session,
            context,
            binding,
        })
    }

    /// Only non-secret owner facts escape this operation. The sync mutex and
    /// SecretSession read guard cover catalog open, identity recheck and action.
    async fn with_current<R>(
        &self,
        operation: impl FnOnce(
            &VaultAccountStore<'_>,
            &NativeCipher,
            &AccountSnapshot,
        ) -> Result<R, Failure>,
    ) -> Result<R, Failure> {
        let _owner = crate::services::sync_protocol::sync_mutex().lock().await;
        let vault = self.session.read().map_err(|_| Failure::StaleIdentity)?;
        let meta = vault.metadata();
        if meta.vault_id != self.binding.vault_id
            || meta.key_id != self.binding.key_id
            || meta.revision != self.binding.vault_revision
            || self.session.lifecycle_epoch() != self.binding.epoch
        {
            return Err(Failure::StaleIdentity);
        }
        let native = self
            .context
            .cipher()
            .map_err(|_| Failure::UnsupportedContext)?;
        let store = VaultAccountStore::new(self.session.root(), &vault)
            .map_err(|_| Failure::StorageUnavailable)?;
        let recovery = store.status().map_err(|_| Failure::StorageUnavailable)?;
        if recovery.pending || recovery.native_unconfirmed {
            return Err(Failure::StaleIdentity);
        }
        let catalog = store
            .import_catalog(&native, &self.binding.catalog_revision)
            .map_err(|_| Failure::StaleIdentity)?;
        let snapshot = catalog
            .profiles()
            .find(|s| s.identity().opaque_id() == self.binding.account_id)
            .ok_or(Failure::StaleIdentity)?;
        if saved_session_binding(&native, snapshot).map_err(|_| Failure::StaleIdentity)?
            != self.binding.session_binding
        {
            return Err(Failure::StaleIdentity);
        }
        operation(&store, &native, snapshot)
    }

    /// Shared response owner must call this before final downstream output after
    /// conversion/priming too. It exposes no token, snapshot, or native context.
    pub(crate) async fn check(&self) -> Result<(), Failure> {
        self.with_current(|_, _, _| Ok(())).await
    }

    #[cfg(not(test))]
    pub(crate) async fn dispatch(
        self,
        body: Value,
        timeout: Duration,
    ) -> Result<DispatchResponse, ProxyError> {
        self.dispatch_with(body, timeout, &OfficialTransport).await
    }
    async fn dispatch_with<T: StreamTransport>(
        mut self,
        body: Value,
        timeout: Duration,
        transport: &T,
    ) -> Result<DispatchResponse, ProxyError> {
        if !body.is_object()
            || !body.get("messages").is_some_and(Value::is_array)
            || body
                .get("model")
                .and_then(Value::as_str)
                .is_none_or(|v| v.trim().is_empty())
            || body.get("stream").is_some_and(|v| !v.is_boolean())
        {
            return Err(ProxyError::InvalidRequest(
                "zcode.inference.messages_required".into(),
            ));
        }
        let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
        let mut pending = None;
        let mut ready = None;
        let mut response_guard = None;
        // Construct AND first-poll the sole network future while admitted. This
        // is request-admission ordering, not a claim that bytes already left.
        self.with_current(|store, native, snapshot| {
            let token = claim_jwt(native, snapshot).map_err(|_| Failure::NeedsLogin)?;
            if jwt_is_expired(token.expose(), now_seconds()) {
                self.reject_current(store, native, snapshot)?;
                return Err(Failure::NeedsLogin);
            }
            // Keep the known Start JWT only inside the private owner's bounded,
            // zeroizing screen; no token reaches shared response/fence metadata.
            response_guard = Some(
                KnownSecretGuard::new(&[token.expose().as_bytes()])
                    .map_err(|_| Failure::UnsupportedContext)?,
            );
            let mut headers = http::HeaderMap::new();
            let mut key =
                http::HeaderValue::from_str(token.expose()).map_err(|_| Failure::NeedsLogin)?;
            key.set_sensitive(true);
            let mut bearer = http::HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                .map_err(|_| Failure::NeedsLogin)?;
            bearer.set_sensitive(true);
            headers.insert("x-api-key", key);
            headers.insert(http::header::AUTHORIZATION, bearer);
            headers.insert(
                "anthropic-version",
                http::HeaderValue::from_static("2023-06-01"),
            );
            headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
            headers.insert(
                http::header::ACCEPT,
                http::HeaderValue::from_static("application/json, text/event-stream"),
            );
            headers.insert(
                http::header::ACCEPT_ENCODING,
                http::HeaderValue::from_static("identity"),
            );
            let request = StreamRequest {
                url: START_MESSAGES_URL,
                headers,
                body: serde_json::to_vec(&body).map_err(|_| Failure::StorageUnavailable)?,
                streaming: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
                timeout,
            };
            let mut future = transport.request(request);
            match future
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker))
            {
                std::task::Poll::Ready(response) => ready = Some(response),
                std::task::Poll::Pending => pending = Some(future),
            }
            Ok(())
        })
        .await
        .map_err(owner_error)?;
        let response = match ready {
            Some(result) => result?,
            None => {
                pending
                    .ok_or_else(|| owner_error(Failure::StorageUnavailable))?
                    .await?
            }
        };
        let (response, denied) = if response.status().is_success() {
            (response, false)
        } else {
            safe_error_response(response, timeout).await?
        };
        self.check().await.map_err(owner_error)?;
        if response.status() == http::StatusCode::UNAUTHORIZED && !denied {
            // Never refresh, replay, create a key, or silently select an account.
            // Late 401 after replacement may not contaminate a newer session.
            let committed_revision = self
                .with_current(|store, native, snapshot| {
                    self.reject_current(store, native, snapshot)
                })
                .await
                .map_err(owner_error)?;
            // This exact revision is the verified result of our own rejection
            // CAS, never a lookup of the latest catalog. All identity and epoch
            // facts stay unchanged, and only the response fence receives it.
            self.binding.catalog_revision = committed_revision;
            return Ok(DispatchResponse {
                response,
                fence: ResponseFence { lease: self },
            });
        }
        self.check().await.map_err(owner_error)?;
        if !response.status().is_success() {
            return Ok(DispatchResponse {
                response,
                fence: ResponseFence { lease: self },
            });
        }
        let fence = ResponseFence {
            lease: self.clone(),
        };
        let response = self.fence_response(
            response,
            response_guard.ok_or_else(|| owner_error(Failure::StorageUnavailable))?,
        )?;
        Ok(DispatchResponse { response, fence })
    }

    fn reject_current(
        &self,
        store: &VaultAccountStore<'_>,
        native: &NativeCipher,
        snapshot: &AccountSnapshot,
    ) -> Result<String, Failure> {
        let catalog = store
            .import_catalog(native, &self.binding.catalog_revision)
            .map_err(|_| Failure::StaleIdentity)?;
        let previous = catalog.details(snapshot, native).evidence;
        let report = SessionCheckReport::start_auth_rejected(
            native,
            snapshot,
            previous.as_ref(),
            now_seconds(),
        );
        store
            .update_profile_evidence(
                native,
                &self.binding.catalog_revision,
                &self.binding.account_id,
                report,
            )
            .map(|status| status.revision)
            .map_err(|_| Failure::StaleIdentity)
    }

    fn fence_response(
        self,
        response: ProxyResponse,
        guard: KnownSecretGuard,
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
                "zcode.inference.unsupported_content_encoding".into(),
            ));
        }
        let status = response.status();
        let headers = safe_success_headers(response.headers());
        let stream = Box::pin(guard.wrap_stream(response.bytes_stream()));
        let fenced = futures::stream::unfold(
            (self, stream, false),
            |(lease, mut stream, stopped)| async move {
                if stopped {
                    return None;
                }
                if lease.check().await.is_err() {
                    return Some((
                        Err(std::io::Error::other(
                            "zcode.inference.authorization_changed",
                        )),
                        (lease, stream, true),
                    ));
                }
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        if lease.check().await.is_err() {
                            Some((
                                Err(std::io::Error::other(
                                    "zcode.inference.authorization_changed",
                                )),
                                (lease, stream, true),
                            ))
                        } else {
                            Some((Ok(bytes), (lease, stream, false)))
                        }
                    }
                    Some(Err(_)) => Some((
                        Err(std::io::Error::other("zcode.inference.stream_interrupted")),
                        (lease, stream, true),
                    )),
                    None => None,
                }
            },
        );
        Ok(ProxyResponse::streamed(status, headers, fenced))
    }
}
fn safe_success_headers(original: &http::HeaderMap) -> http::HeaderMap {
    // Header values/parameters may echo a credential even on HTTP 200.
    let content_type = original
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let normalized = match content_type {
        Some(value) if value.eq_ignore_ascii_case("text/event-stream") => Some("text/event-stream"),
        Some(value)
            if value.eq_ignore_ascii_case("application/json")
                || value.to_ascii_lowercase().ends_with("+json") =>
        {
            Some("application/json")
        }
        Some(value) if value.eq_ignore_ascii_case("application/octet-stream") => {
            Some("application/octet-stream")
        }
        _ => None,
    };
    let mut headers = http::HeaderMap::new();
    if let Some(value) = normalized {
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
                .is_some_and(|n| n <= 86400)
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
            "zcode.inference.error_encoding_unsupported".into(),
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
        .map_err(|_| ProxyError::Internal("zcode.inference.error_body_timeout".into()))?
        .map_err(|_| ProxyError::Internal("zcode.inference.error_body_unreadable".into()))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ProxyError::Internal("zcode.inference.error_body_undecodable".into()))?;
    let envelope: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_)
            if !text.trim().is_empty()
                && (declared_json || text.trim_start().starts_with(['{', '['])) =>
        {
            return Err(ProxyError::Internal(
                "zcode.inference.error_json_invalid".into(),
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
    let kind = if denied {
        "access_denied"
    } else if status == http::StatusCode::UNAUTHORIZED {
        "authentication_error"
    } else if status == http::StatusCode::TOO_MANY_REQUESTS {
        "rate_limit_error"
    } else {
        "api_error"
    };
    let message = if status == http::StatusCode::UNAUTHORIZED && !denied {
        "zcode.inference.needs_login"
    } else {
        "ZCode Start Plan request was not accepted"
    };
    Ok((
        ProxyResponse::buffered(
            status,
            headers,
            Bytes::from(
                serde_json::json!({"type":"error","error":{"type":kind,"message":message}})
                    .to_string(),
            ),
        ),
        denied,
    ))
}

#[cfg(all(test, unix))]
#[path = "inference_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "inference_fixture.rs"]
mod fixture;
#[cfg(all(test, unix))]
pub(crate) use fixture::FixtureTransport;
