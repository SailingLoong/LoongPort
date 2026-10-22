//! Fixed-host TRAE inference using a selected original pool account only.
use super::{
    protocol::{self, Decoder, Part},
    store::{self, Failure, Snapshot},
};
use crate::{
    proxy::{hyper_client::ProxyResponse, ProxyError},
    secrets::{session::SecretSession, stream_guard::KnownSecretGuard},
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, future::Future, pin::Pin, sync::Arc, time::Duration};
pub(super) struct Request {
    pub url: String,
    pub headers: http::HeaderMap,
    pub body: Value,
    pub timeout: Duration,
}
pub(super) trait Transport: Send + Sync {
    fn send(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>>;
}
pub(super) struct OfficialHttp;
impl Transport for OfficialHttp {
    fn send(
        &self,
        r: Request,
    ) -> Pin<Box<dyn Future<Output = Result<ProxyResponse, ProxyError>> + Send + '_>> {
        Box::pin(async move {
            #[cfg(test)]
            {
                let _ = r;
                Err(ProxyError::ConfigError(
                    "trae.fixture_transport_required".into(),
                ))
            }
            #[cfg(not(test))]
            {
                let client = crate::proxy::http_client::get_budgeted_authenticated()
                    .map_err(|_| network_error())?;
                let response = client
                    .post(r.url)
                    .headers(r.headers)
                    .timeout(r.timeout)
                    .json(&r.body)
                    .send()
                    .await
                    .map_err(|_| network_error())?;
                Ok(ProxyResponse::Reqwest(response))
            }
        })
    }
}
pub(crate) struct Lease {
    pub(super) session: Arc<SecretSession>,
    pub(super) snapshot: Snapshot,
    request_session: String,
}
#[derive(Clone)]
pub(crate) struct ResponseFence {
    session: Arc<SecretSession>,
    snapshot: Snapshot,
}
pub(crate) struct DispatchResponse {
    pub response: ProxyResponse,
    pub fence: ResponseFence,
}
impl ResponseFence {
    pub(crate) async fn check(&self) -> Result<(), ProxyError> {
        store::access(&self.session, Some(&self.snapshot.binding), |d, _| {
            let a = store::matched(d, &self.snapshot)?;
            if a.version != self.snapshot.account.version || a.refresh_pending {
                return Err(Failure::Retired);
            }
            Ok(((), false))
        })
        .await
        .map_err(|_| ProxyError::Internal("trae.account_no_longer_available".into()))
    }
}
fn auth_error() -> ProxyError {
    ProxyError::AuthError("trae.authorization_unavailable".into())
}
fn network_error() -> ProxyError {
    ProxyError::ForwardFailed("trae.request_failed".into())
}
fn stream_error() -> std::io::Error {
    std::io::Error::other("trae.stream_interrupted")
}
pub(super) fn failure(status: u16) -> ProxyResponse {
    ProxyResponse::buffered(
        http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::BAD_GATEWAY),
        headers(false),
        Bytes::from_static(if status == 403 {
            b"{\"error\":{\"type\":\"access_denied\",\"message\":\"TRAE request unavailable\"}}"
        } else {
            b"{\"error\":{\"type\":\"api_error\",\"message\":\"TRAE request unavailable\"}}"
        }),
    )
}
fn headers(stream: bool) -> http::HeaderMap {
    let mut h = http::HeaderMap::new();
    h.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static(if stream {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    h
}
pub(super) fn header(
    h: &mut http::HeaderMap,
    name: &'static str,
    value: &str,
    sensitive: bool,
) -> Result<(), ProxyError> {
    let mut value = http::HeaderValue::from_str(value).map_err(|_| auth_error())?;
    value.set_sensitive(sensitive);
    h.insert(name, value);
    Ok(())
}
impl Lease {
    pub(crate) async fn acquire(
        session: Arc<SecretSession>,
        id: &str,
        request_session: &str,
    ) -> Result<Self, Failure> {
        let snapshot = store::snapshot(&session, id).await?;
        // Legacy CN rows lack validated native product provenance. A newly
        // selected CN Code installation is isolated from Global SG/US identity.
        if (snapshot.account.realm == store::Realm::Cn
            && snapshot.account.device.native_package.as_deref() != Some("TRAE_CN"))
            || snapshot.account.device.auth_host != snapshot.account.realm.auth()
            || snapshot.account.device.client != "ono9krqynydwx5"
        {
            return Err(Failure::Unsupported);
        }
        if !store::text(request_session, 4096) {
            return Err(Failure::Unsupported);
        }
        Ok(Self {
            session,
            snapshot,
            request_session: request_session.into(),
        })
    }
    pub(crate) fn base_url(&self) -> &'static str {
        self.snapshot.account.realm.api()
    }
    pub(crate) async fn dispatch(
        self,
        body: Value,
        timeout: Duration,
    ) -> Result<DispatchResponse, ProxyError> {
        self.dispatch_with(body, timeout, &OfficialHttp).await
    }
    pub(super) fn fence(&self) -> ResponseFence {
        ResponseFence {
            session: self.session.clone(),
            snapshot: self.snapshot.clone(),
        }
    }
    pub(super) fn wire(&self, body: Value, timeout: Duration) -> Result<Request, ProxyError> {
        let a = &self.snapshot.account;
        let g = a.grant.as_ref().ok_or_else(auth_error)?;
        let d = &a.device;
        let mut h = http::HeaderMap::new();
        header(
            &mut h,
            "authorization",
            &format!("Cloud-IDE-JWT {}", g.access),
            true,
        )?;
        header(&mut h, "x-cloudide-token", &g.access, true)?;
        header(&mut h, "x-ide-token", &g.access, true)?;
        for (k, v) in [
            ("x-uid", a.uid.as_str()),
            ("x-device-id", d.id.as_str()),
            ("x-machine-id", d.machine.as_str()),
            ("x-app-version", d.version.as_str()),
            ("x-ide-version", d.version.as_str()),
            ("x-app-version-code", d.version_code.as_str()),
            ("x-ide-version-code", d.version_code.as_str()),
            ("x-device-type", d.os.as_str()),
            ("x-os-version", d.os_version.as_str()),
        ] {
            if !v.is_empty() {
                header(&mut h, k, v, false)?;
            }
        }
        header(
            &mut h,
            "x-app-id",
            "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8",
            false,
        )?;
        header(&mut h, "content-type", "application/json", false)?;
        header(&mut h, "accept", "text/event-stream", false)?;
        header(&mut h, "accept-encoding", "identity", false)?;
        Ok(Request {
            url: format!("{}/api/agent/v3/llm_utils_chat", a.realm.api()),
            headers: h,
            body,
            timeout,
        })
    }
    pub(super) async fn dispatch_with(
        mut self,
        body: Value,
        timeout: Duration,
        transport: &impl Transport,
    ) -> Result<DispatchResponse, ProxyError> {
        let a = &self.snapshot.account;
        let correlation = hex::encode(Sha256::digest(
            serde_json::to_vec(&(
                &self.snapshot.binding.vault.vault_id,
                &a.id,
                a.generation,
                &self.request_session,
            ))
            .map_err(|_| network_error())?,
        ));
        let session = format!(
            "{}-{}-4{}-a{}-{}",
            &correlation[..8],
            &correlation[8..12],
            &correlation[13..16],
            &correlation[17..20],
            &correlation[20..32]
        );
        let want_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        let model = body["model"].as_str().unwrap_or("").to_string();
        let wire = protocol::request(body, &session)
            .map_err(|_| ProxyError::ConfigError("trae.text_only_request_required".into()))?;
        super::auth::synchronize(&self.session, &mut self.snapshot).await?;
        let mut refreshed = false;
        if self
            .snapshot
            .account
            .grant
            .as_ref()
            .ok_or_else(auth_error)?
            .expires
            <= store::now() + 120000
        {
            super::auth::refresh(&self.session, &mut self.snapshot, timeout, transport).await?;
            refreshed = true;
        }
        let mut response = admit(
            &self.session,
            &self.snapshot,
            transport,
            self.wire(wire.clone(), timeout)?,
            false,
        )
        .await?;
        self.fence().check().await?;
        if !response.status().is_success() {
            let status = classified_failure(response).await;
            self.fence().check().await?;
            if status != 401 || refreshed {
                return Ok(DispatchResponse {
                    response: failure(status),
                    fence: self.fence(),
                });
            }
            super::auth::refresh(&self.session, &mut self.snapshot, timeout, transport).await?;
            response = admit(
                &self.session,
                &self.snapshot,
                transport,
                self.wire(wire, timeout)?,
                false,
            )
            .await?;
        }
        let fence = self.fence();
        fence.check().await?;
        let g = self
            .snapshot
            .account
            .grant
            .as_ref()
            .ok_or_else(auth_error)?;
        if !response.status().is_success() || !response.is_sse() {
            let status = classified_failure(response).await;
            fence.check().await?;
            return Ok(DispatchResponse {
                response: failure(status),
                fence,
            });
        }
        if response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
        {
            return Ok(DispatchResponse {
                response: failure(502),
                fence,
            });
        }
        let guard = KnownSecretGuard::new(&[g.access.as_bytes(), g.refresh.as_bytes()])
            .map_err(|_| network_error())?;
        // Inspect a bounded prefix only to classify an initial structured denial.
        // Denials become our fixed response; no original bytes leave this branch.
        // Every normal response is then screened in its ORIGINAL byte encoding.
        let mut source = Box::pin(response.bytes_stream());
        let mut held = Vec::new();
        let mut size = 0usize;
        let mut peek = Decoder::default();
        loop {
            fence.check().await?;
            let next = source.next().await;
            fence.check().await?;
            let bytes = match next {
                Some(Ok(bytes)) => bytes,
                _ => {
                    return Ok(DispatchResponse {
                        response: failure(502),
                        fence,
                    })
                }
            };
            size = size.saturating_add(bytes.len());
            if size > 1024 * 1024 {
                return Ok(DispatchResponse {
                    response: failure(502),
                    fence,
                });
            }
            let semantic = match peek.feed(&bytes) {
                Ok(parts) => parts.into_iter().next(),
                Err(_) => {
                    return Ok(DispatchResponse {
                        response: failure(502),
                        fence,
                    })
                }
            };
            held.push(Ok(bytes));
            match semantic {
                Some(Part::Error(status)) => {
                    return Ok(DispatchResponse {
                        response: failure(status),
                        fence,
                    })
                }
                Some(_) => break,
                None => continue,
            }
        }
        let raw = futures::stream::iter(held).chain(source);
        let content_guard = KnownSecretGuard::new(&[g.access.as_bytes(), g.refresh.as_bytes()])
            .map_err(|_| network_error())?;
        let reasoning_guard = KnownSecretGuard::new(&[g.access.as_bytes(), g.refresh.as_bytes()])
            .map_err(|_| network_error())?;
        let mut parts = Parts {
            content_guard: Some(content_guard),
            reasoning_guard: Some(reasoning_guard),
            source: Box::pin(guard.wrap_stream(raw)),
            decoder: Decoder::default(),
            queued: VecDeque::new(),
            fence: fence.clone(),
            ended: false,
        };
        let first = parts.next().await;
        let response = match first {
            Part::Error(status) => failure(status),
            part if want_stream => stream_response(parts, part, model),
            part => buffered_response(parts, part, model).await,
        };
        fence.check().await?;
        Ok(DispatchResponse { response, fence })
    }
}
/// Only fixed sanitized errors are returned. Read a bounded error body before
/// deciding whether an explicit HTTP 401 may use the single refresh budget.
async fn classified_failure(response: ProxyResponse) -> u16 {
    let status = response.status().as_u16();
    if response
        .headers()
        .get(http::header::CONTENT_ENCODING)
        .is_some_and(|v| v != "identity")
    {
        return 502;
    }
    let bytes = match response.bytes_with_limit(1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return 502,
    };
    if let Ok(body) = serde_json::from_slice::<Value>(&bytes) {
        if let Some(status) = protocol::denial(&body) {
            return status;
        }
    }
    if let Ok(parts) = Decoder::default().feed(&bytes) {
        // Permission/risk denials take priority over any authentication code.
        if parts.iter().any(|p| matches!(p, Part::Error(403))) {
            return 403;
        }
        if let Some(status) = parts.into_iter().find_map(|p| match p {
            Part::Error(s) => Some(s),
            _ => None,
        }) {
            return status;
        }
    }
    if (200..300).contains(&status) {
        502
    } else {
        status
    }
}
/// Classification is conservative: only a failure before transport creation and
/// first poll may withdraw an exact durable intent. Never infer "unsent" from IO.
pub(super) enum DispatchFailure {
    NotDispatched(ProxyError),
    OutcomeUnknown(ProxyError),
}
impl DispatchFailure {
    pub(super) fn error(self) -> ProxyError {
        match self {
            Self::NotDispatched(e) | Self::OutcomeUnknown(e) => e,
        }
    }
}
/// Transport creation and first poll share the owner's mutation admission fence.
pub(super) async fn admit(
    session: &SecretSession,
    s: &Snapshot,
    transport: &impl Transport,
    request: Request,
    pending_refresh: bool,
) -> Result<ProxyResponse, ProxyError> {
    admit_checked(session, s, transport, request, pending_refresh, |_| Ok(()))
        .await
        .map_err(DispatchFailure::error)
}
pub(super) async fn admit_checked(
    session: &SecretSession,
    s: &Snapshot,
    transport: &impl Transport,
    request: Request,
    pending_refresh: bool,
    check: impl FnOnce(&store::Account) -> Result<(), Failure>,
) -> Result<ProxyResponse, DispatchFailure> {
    let waker = std::future::poll_fn(|cx| std::task::Poll::Ready(cx.waker().clone())).await;
    let mut pending = None;
    let mut ready = None;
    let mut entered = false;
    let admission = store::access(session, Some(&s.binding), |d, _| {
        let a = store::matched(d, s)?;
        if a.version != s.account.version || a.refresh_pending != pending_refresh {
            return Err(Failure::Retired);
        }
        check(a)?;
        entered = true;
        let mut future = transport.send(request);
        match future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            std::task::Poll::Ready(r) => ready = Some(r),
            std::task::Poll::Pending => pending = Some(future),
        }
        Ok(((), false))
    })
    .await;
    if admission.is_err() {
        return Err(if entered {
            DispatchFailure::OutcomeUnknown(auth_error())
        } else {
            DispatchFailure::NotDispatched(auth_error())
        });
    }
    if let Some(r) = ready {
        r
    } else {
        pending
            .ok_or_else(|| DispatchFailure::OutcomeUnknown(auth_error()))?
            .await
    }
    .map_err(DispatchFailure::OutcomeUnknown)
}
struct Parts {
    content_guard: Option<KnownSecretGuard>,
    reasoning_guard: Option<KnownSecretGuard>,
    source: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
    decoder: Decoder,
    queued: VecDeque<Part>,
    fence: ResponseFence,
    ended: bool,
}
impl Parts {
    /// Preserve two separate logical text streams after JSON unescaping. This
    /// reuses the original bounded matcher so frame boundaries cannot hide a
    /// known credential, and a failed stream never flushes its held suffix.
    fn project(&mut self, part: Part) -> Result<Option<Part>, ()> {
        match part {
            Part::Delta(delta) => {
                let mut screened = json!({});
                for (field, guard) in [
                    ("content", &mut self.content_guard),
                    ("reasoning_content", &mut self.reasoning_guard),
                ] {
                    if let Some(text) = delta.get(field).and_then(Value::as_str) {
                        let emit = guard
                            .as_mut()
                            .ok_or(())?
                            .feed(Bytes::copy_from_slice(text.as_bytes()))
                            .map_err(|_| ())?;
                        if let Some(bytes) = emit {
                            screened[field] = json!(std::str::from_utf8(&bytes).map_err(|_| ())?);
                        }
                    }
                }
                Ok((!screened.as_object().unwrap().is_empty()).then_some(Part::Delta(screened)))
            }
            Part::Done => {
                let mut tail = json!({});
                for (field, guard) in [
                    ("content", &mut self.content_guard),
                    ("reasoning_content", &mut self.reasoning_guard),
                ] {
                    if let Some(bytes) = guard.take().and_then(KnownSecretGuard::finish) {
                        tail[field] = json!(std::str::from_utf8(&bytes).map_err(|_| ())?);
                    }
                }
                self.ended = true;
                if tail.as_object().unwrap().is_empty() {
                    Ok(Some(Part::Done))
                } else {
                    self.queued.push_front(Part::Done);
                    Ok(Some(Part::Delta(tail)))
                }
            }
            Part::Error(code) => {
                self.content_guard.take();
                self.reasoning_guard.take();
                self.queued.clear();
                self.ended = true;
                Ok(Some(Part::Error(code)))
            }
        }
    }
    async fn next(&mut self) -> Part {
        loop {
            if self.fence.check().await.is_err() {
                self.ended = true;
                return Part::Error(401);
            }
            if let Some(p) = self.queued.pop_front() {
                match self.project(p) {
                    Ok(Some(p)) => return p,
                    Ok(None) => continue,
                    Err(_) => {
                        self.content_guard.take();
                        self.reasoning_guard.take();
                        self.queued.clear();
                        self.ended = true;
                        return Part::Error(502);
                    }
                }
            }
            if self.ended {
                return Part::Error(502);
            }
            let next = self.source.next().await;
            if self.fence.check().await.is_err() {
                self.ended = true;
                return Part::Error(401);
            }
            match next {
                Some(Ok(b)) => match self.decoder.feed(&b) {
                    Ok(parts) => self.queued.extend(parts),
                    Err(_) => {
                        self.ended = true;
                        return Part::Error(502);
                    }
                },
                Some(Err(_)) => {
                    self.ended = true;
                    return Part::Error(502);
                }
                None => {
                    self.ended = true;
                    return if self.decoder.finished() {
                        Part::Done
                    } else {
                        Part::Error(502)
                    };
                }
            }
        }
    }
}
fn chunk(id: &str, model: &str, delta: Value, finish: Option<&str>) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"id":id,"object":"chat.completion.chunk","created":store::now()/1000,"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    ))
}
fn stream_response(parts: Parts, first: Part, model: String) -> ProxyResponse {
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let stream = futures::stream::unfold(
        (parts, Some(first), model, id, false),
        |(mut parts, first, model, id, stopped)| async move {
            if stopped {
                return None;
            }
            let part = match first {
                Some(p) => p,
                None => parts.next().await,
            };
            let (done, item) = match part {
                Part::Delta(d) => (false, Ok(chunk(&id, &model, d, None))),
                Part::Done => {
                    let mut bytes = chunk(&id, &model, json!({}), Some("stop")).to_vec();
                    bytes.extend_from_slice(b"data: [DONE]\n\n");
                    (true, Ok(Bytes::from(bytes)))
                }
                Part::Error(_) => (true, Err(stream_error())),
            };
            if parts.fence.check().await.is_err() {
                return Some((Err(stream_error()), (parts, None, model, id, true)));
            }
            Some((item, (parts, None, model, id, done)))
        },
    );
    ProxyResponse::streamed(http::StatusCode::OK, headers(true), stream)
}
async fn buffered_response(mut parts: Parts, first: Part, model: String) -> ProxyResponse {
    let mut part = first;
    let mut content = String::new();
    let mut reasoning = String::new();
    loop {
        match part {
            Part::Error(s) => return failure(s),
            Part::Done => break,
            Part::Delta(d) => {
                if let Some(s) = d["content"].as_str() {
                    content.push_str(s)
                }
                if let Some(s) = d["reasoning_content"].as_str() {
                    reasoning.push_str(s)
                }
                if content.len() + reasoning.len() > 4 * 1024 * 1024 {
                    return failure(502);
                }
            }
        }
        part = parts.next().await;
    }
    let mut message = json!({"role":"assistant","content":content});
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning)
    }
    let body = json!({"id":format!("chatcmpl-{}",uuid::Uuid::new_v4()),"object":"chat.completion","created":store::now()/1000,"model":model,"choices":[{"index":0,"message":message,"finish_reason":"stop"}]});
    ProxyResponse::buffered(
        http::StatusCode::OK,
        headers(false),
        Bytes::from(serde_json::to_vec(&body).unwrap_or_default()),
    )
}

/// Pure capability admission on original Chat, Claude or Responses bodies.
/// Full wire/schema validation remains protocol::validate_request after conversion.
#[cfg(any(feature = "gui", test))]
pub(crate) fn validate_capabilities(body: &Value) -> Result<(), &'static str> {
    fn text_content(v: &Value) -> bool {
        v.is_string()
            || v.as_array().is_some_and(|a| {
                a.iter().all(|part| {
                    part.as_object().is_some_and(|o| {
                        matches!(
                            o.get("type").and_then(Value::as_str),
                            Some("text" | "input_text" | "output_text")
                        ) && o.get("text").is_some_and(Value::is_string)
                            && o.keys().all(|k| matches!(k.as_str(), "type" | "text"))
                    })
                })
            })
    }
    fn message(v: &Value) -> bool {
        v.as_object().is_some_and(|o| {
            o.get("type").is_none_or(|t| t.as_str() == Some("message"))
                && matches!(
                    o.get("role").and_then(Value::as_str),
                    Some("system" | "developer" | "user" | "assistant")
                )
                && !o.contains_key("tool_calls")
                && !o.contains_key("function_call")
                && !o.contains_key("tool_call_id")
                && !o.contains_key("audio")
                && o.get("content").is_some_and(text_content)
        })
    }
    let o = body.as_object().ok_or("trae.text_only_request_required")?;
    let incompatible = body
        .pointer("/stream_options/include_usage")
        .is_some_and(|v| v.as_bool() != Some(false))
        || ["tools", "functions"].iter().any(|k| {
            o.get(*k)
                .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
        })
        || ["tool_choice", "function_call"]
            .iter()
            .any(|k| o.get(*k).is_some_and(|v| v.as_str() != Some("none")))
        || o.get("parallel_tool_calls")
            .is_some_and(|v| v.as_bool() != Some(false))
        || o.get("modalities").is_some_and(|v| {
            v.as_array()
                .is_none_or(|a| a.iter().any(|m| m.as_str() != Some("text")))
        })
        || ["audio", "image", "image_url", "input_audio"]
            .iter()
            .any(|k| o.contains_key(*k))
        || o.get("system").is_some_and(|v| !text_content(v))
        || o.get("messages")
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.iter().all(message)))
        || o.get("input")
            .is_some_and(|v| !v.is_string() && v.as_array().is_none_or(|a| !a.iter().all(message)));
    if incompatible {
        Err("trae.text_only_request_required")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
pub(crate) use fixture::{FixtureOwner, FixtureTransport};
#[cfg(test)]
mod tests;
